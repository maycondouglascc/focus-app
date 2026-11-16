//! Direct3D + DirectComposition + Direct2D surface for the main window.
//! Created when the window opens, dropped (all COM objects released) when it
//! closes. Drawing coordinates are DIPs: the device context carries the DPI.

use std::cell::RefCell;
use std::rc::Rc;

use windows::core::{Interface, Result};
use windows::Win32::Foundation::{E_FAIL, HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_ERROR_DEVICE_REMOVED,
    DXGI_ERROR_DEVICE_RESET, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows_numerics::Vector2;

use super::chrome::{Chrome, TITLE_SIZE};
use super::frame::{BUTTON_DIP, TITLE_BAR_DIP};
use super::strings;
use super::text::{TextSystem, Weight};
use super::theme::{self, Color};

/// `D2DERR_RECREATE_TARGET`
const D2DERR_RECREATE_TARGET: i32 = 0x8899000Cu32 as i32;

#[derive(Debug)]
pub enum DrawError {
    /// The graphics device is gone (driver update, GPU switch, RDP): drop the
    /// renderer and build a new one.
    DeviceLost,
    Other(windows::core::Error),
}

impl From<windows::core::Error> for DrawError {
    fn from(e: windows::core::Error) -> Self {
        let c = e.code();
        if c == DXGI_ERROR_DEVICE_REMOVED
            || c == DXGI_ERROR_DEVICE_RESET
            || c.0 == D2DERR_RECREATE_TARGET
        {
            DrawError::DeviceLost
        } else {
            DrawError::Other(e)
        }
    }
}

/// Which caption button the pointer is over.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hover {
    None,
    Minimize,
    Maximize,
    Close,
}

/// Device-level objects shared by every window of this process. They are kept
/// after the window closes (decision 0005): the graphics stack retains its
/// memory anyway, and creating and destroying devices over and over leaks a
/// little each time. Dropped only when the device is lost.
struct Gfx {
    d3d: ID3D11Device,
    factory: IDXGIFactory2,
    d2d_device: ID2D1Device,
    dcomp: IDCompositionDevice,
}

thread_local! {
    /// Text formats are cheap and creating them again every time the window
    /// opens grows memory, so this one lives for the whole process.
    static TEXT: RefCell<Option<Rc<TextSystem>>> = const { RefCell::new(None) };
    static GFX: RefCell<Option<Rc<Gfx>>> = const { RefCell::new(None) };
}

/// Forgets the shared device objects; the next window builds new ones.
pub fn reset_shared() {
    GFX.with(|g| g.borrow_mut().take());
}

fn text_system() -> Result<Rc<TextSystem>> {
    if let Some(t) = TEXT.with(|t| t.borrow().clone()) {
        return Ok(t);
    }
    let t = Rc::new(TextSystem::new()?);
    TEXT.with(|c| *c.borrow_mut() = Some(t.clone()));
    Ok(t)
}

fn shared() -> Result<Rc<Gfx>> {
    if let Some(g) = GFX.with(|g| g.borrow().clone()) {
        return Ok(g);
    }
    // SAFETY: plain COM calls on objects created right here.
    let gfx = unsafe {
        let d3d = d3d_device()?;
        let dxgi: IDXGIDevice = d3d.cast()?;
        let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
        let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let d2d_device = d2d.CreateDevice(&dxgi)?;
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
        Rc::new(Gfx {
            d3d,
            factory,
            d2d_device,
            dcomp,
        })
    };
    GFX.with(|g| *g.borrow_mut() = Some(gfx.clone()));
    Ok(gfx)
}

pub struct Renderer {
    // Keeps the device objects alive while this renderer exists.
    _gfx: Rc<Gfx>,
    text: Rc<TextSystem>,
    chrome: Chrome,
    swap: IDXGISwapChain1,
    ctx: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    // Held so the composition tree stays alive as long as the renderer.
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

fn d3d_device() -> Result<ID3D11Device> {
    let mut device = None;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        // SAFETY: the out pointer is a valid Option slot; no software module.
        let r = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        if r.is_ok() {
            break;
        }
    }
    device.ok_or_else(|| windows::core::Error::from(E_FAIL))
}

impl Renderer {
    pub fn new(hwnd: HWND, w: u32, h: u32) -> Result<Self> {
        // SAFETY: plain COM calls on objects created right here; every pointer
        // passed is a live reference owned by this function.
        unsafe {
            let gfx = shared()?;
            let text = text_system()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w.max(1),
                Height: h.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = gfx
                .factory
                .CreateSwapChainForComposition(&gfx.d3d, &desc, None)?;

            let ctx = gfx
                .d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let brush = ctx.CreateSolidColorBrush(&color(theme::TEXT), None)?;

            let target = gfx.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = gfx.dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            gfx.dcomp.Commit()?;
            Ok(Self {
                _gfx: gfx,
                text,
                chrome: Chrome::new(),
                swap,
                ctx,
                brush,
                _target: target,
                _visual: visual,
            })
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) -> std::result::Result<(), DrawError> {
        // SAFETY: the target bitmap is released first, as ResizeBuffers requires.
        unsafe {
            self.ctx.SetTarget(None);
            self.swap.ResizeBuffers(
                0,
                w.max(1),
                h.max(1),
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        Ok(())
    }

    /// Draws one frame. `w` is the client width in DIPs.
    pub fn draw(&mut self, dpi: u32, w: f32, hover: Hover) -> std::result::Result<(), DrawError> {
        // SAFETY: all objects are owned by `self`; BeginDraw/EndDraw are paired.
        unsafe {
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = self
                .ctx
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.ctx.SetTarget(&bitmap);
            self.ctx.SetDpi(dpi as f32, dpi as f32);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&color(theme::SURFACE)));
            self.fill(0.0, 0.0, w, TITLE_BAR_DIP, theme::TITLE_BAR);
            self.caption_buttons(w, hover);
            self.chrome.layout(w, &self.text);
            let t = self.chrome.title_rect();
            self.draw_text(
                strings::APP_NAME,
                TITLE_SIZE,
                Weight::SemiBold,
                theme::TEXT,
                D2D_RECT_F {
                    left: t.x,
                    top: t.y,
                    right: t.x + t.w,
                    bottom: t.y + t.h,
                },
            );
            let end = self.ctx.EndDraw(None, None);
            self.ctx.SetTarget(None);
            end?;
            self.swap.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    unsafe fn fill(&self, l: f32, t: f32, r: f32, b: f32, c: Color) {
        self.brush.SetColor(&color(c));
        self.ctx.FillRectangle(
            &D2D_RECT_F {
                left: l,
                top: t,
                right: r,
                bottom: b,
            },
            &self.brush,
        );
    }

    /// Draws one line of text inside `rect`, clipped to it.
    unsafe fn draw_text(&self, text: &str, size: f32, weight: Weight, c: Color, rect: D2D_RECT_F) {
        let Ok(format) = self.text.format(size, weight) else {
            return;
        };
        let wide: Vec<u16> = text.encode_utf16().collect();
        self.brush.SetColor(&color(c));
        self.ctx.DrawText(
            &wide,
            &format,
            &rect,
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    unsafe fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, c: Color) {
        self.brush.SetColor(&color(c));
        self.ctx.DrawLine(
            Vector2 { X: x1, Y: y1 },
            Vector2 { X: x2, Y: y2 },
            &self.brush,
            1.0,
            None,
        );
    }

    unsafe fn caption_buttons(&self, w: f32, hover: Hover) {
        let h = TITLE_BAR_DIP;
        let kinds = [Hover::Minimize, Hover::Maximize, Hover::Close];
        for (i, kind) in kinds.into_iter().enumerate() {
            let left = w - BUTTON_DIP * (3 - i) as f32;
            let right = left + BUTTON_DIP;
            let mut ink = theme::TEXT;
            if hover == kind {
                if kind == Hover::Close {
                    self.fill(left, 0.0, right, h, theme::CLOSE_HOVER);
                    ink = theme::ON_CLOSE_HOVER;
                } else {
                    self.fill(left, 0.0, right, h, theme::BUTTON_HOVER);
                }
            }
            let (cx, cy) = (left + BUTTON_DIP / 2.0, h / 2.0);
            match kind {
                Hover::Minimize => self.line(cx - 5.0, cy, cx + 5.0, cy, ink),
                Hover::Maximize => {
                    self.brush.SetColor(&color(ink));
                    self.ctx.DrawRectangle(
                        &D2D_RECT_F {
                            left: cx - 5.0,
                            top: cy - 5.0,
                            right: cx + 5.0,
                            bottom: cy + 5.0,
                        },
                        &self.brush,
                        1.0,
                        None,
                    );
                }
                _ => {
                    self.line(cx - 5.0, cy - 5.0, cx + 5.0, cy + 5.0, ink);
                    self.line(cx - 5.0, cy + 5.0, cx + 5.0, cy - 5.0, ink);
                }
            }
        }
    }
}

fn color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.0,
        g: c.1,
        b: c.2,
        a: c.3,
    }
}
