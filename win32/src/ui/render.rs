//! Direct3D + DirectComposition + Direct2D surface for windows and popups.
//! Created when a window opens, dropped (all COM objects released) when it
//! closes. Drawing coordinates are DIPs: the device context carries the DPI.

use std::cell::RefCell;
use std::rc::Rc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

use windows::core::{Interface, Result, HRESULT};
use windows::Win32::Foundation::{E_FAIL, HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Device, ID2D1DeviceContext, ID2D1Factory, ID2D1Factory1,
    ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT,
    D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_CAP_STYLE_ROUND,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_MULTI_THREADED, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
    D2D1_SWEEP_DIRECTION_CLOCKWISE,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_SINGLETHREADED, D3D11_SDK_VERSION,
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
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows_numerics::Vector2;

use super::chrome::{Chrome, TITLE_SIZE};
use super::frame::{BUTTON_DIP, TITLE_BAR_DIP};
use super::strings;
use super::text::{Align, TextFont, TextSystem, Weight, Wrap};
use super::theme::{Color, Palette};
use super::tree::{DrawCmd, Leaf, PaintStep, Rect, Tree};

/// `D2DERR_RECREATE_TARGET`
pub const D2DERR_RECREATE_TARGET: HRESULT = HRESULT(0x8899000Cu32 as i32);

#[cfg(test)]
static RENDERER_GEN: AtomicU64 = AtomicU64::new(0);

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
            || c == D2DERR_RECREATE_TARGET
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

/// Parameters for rendering a single frame.
pub struct FrameOpts<'a> {
    pub dpi: u32,
    pub w: f32,
    pub h: f32,
    pub hover: Hover,
    pub palette: &'a Palette,
    pub mica: bool,
    pub show_title_bar: bool,
    pub tree: Option<&'a Tree>,
    pub focus_ring: Option<Rect>,
    /// True while native `RichEdit` children are deferred past frame 1 so `RichTextSlot` draws its placeholder/value.
    pub render_rich_slots: bool,
}

struct SendGfx(Gfx);

// SAFETY: `ID3D11Device`, `IDXGIFactory2`, multi-threaded `ID2D1Factory1`/`ID2D1Device`, and `IDCompositionDevice` are MTA/free-threaded COM interfaces transferred exclusively via `JoinHandle::join()`.
unsafe impl Send for SendGfx {}

/// Device-level objects shared by every window of this process while open.
struct Gfx {
    d3d: ID3D11Device,
    factory: IDXGIFactory2,
    d2d_factory: ID2D1Factory1,
    d2d_device: ID2D1Device,
    dcomp: IDCompositionDevice,
    round_stroke: ID2D1StrokeStyle,
}

thread_local! {
    /// Text formats are cheap and creating them again every time the window
    /// opens grows memory, so this one lives for the whole process.
    static TEXT: RefCell<Option<Rc<TextSystem>>> = const { RefCell::new(None) };
    static GFX: RefCell<Option<Rc<Gfx>>> = const { RefCell::new(None) };
    static WARMUP_GFX: RefCell<Option<std::thread::JoinHandle<Result<SendGfx>>>> = const { RefCell::new(None) };
}

fn build_gfx() -> Result<Gfx> {
    // SAFETY: `CoIncrementMTAUsage` idempotently initializes process MTA; all COM objects are owned by `Gfx`.
    unsafe {
        let _ = CoIncrementMTAUsage();
        let d3d = d3d_device()?;
        let dxgi: IDXGIDevice = d3d.cast()?;
        let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
        let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, None)?;
        let d2d_device = d2d_factory.CreateDevice(&dxgi)?;
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            ..Default::default()
        };
        let round_stroke = ID2D1Factory::CreateStrokeStyle(&d2d_factory, &props, None)?;
        Ok(Gfx {
            d3d,
            factory,
            d2d_factory,
            d2d_device,
            dcomp,
            round_stroke,
        })
    }
}

/// Starts WARP D3D11 + Direct2D + DirectComposition creation on a worker thread so window/layout work overlaps it.
pub fn warmup_d3d() {
    let has_gfx = GFX.with(|g| g.borrow().is_some());
    if has_gfx {
        return;
    }
    WARMUP_GFX.with(|w| {
        let mut slot = w.borrow_mut();
        if slot.is_none() {
            *slot = Some(std::thread::spawn(|| build_gfx().map(SendGfx)));
        }
    });
}

/// Returns whether a background warmup or cached graphics stack is active.
pub fn is_cold_start() -> bool {
    GFX.with(|g| g.borrow().is_none()) && TEXT.with(|t| t.borrow().is_none())
}

/// Forgets the shared device objects; the next window builds new ones.
pub fn reset_shared() {
    if let Some(handle) = WARMUP_GFX.with(|w| w.borrow_mut().take()) {
        let _ = handle.join();
    }
    GFX.with(|g| g.borrow_mut().take());
}

pub fn create_text_system_raw() -> Result<SendText> {
    TextSystem::new().map(SendText)
}

pub struct SendText(TextSystem);

// SAFETY: `IDWriteFactory` (`DWRITE_FACTORY_TYPE_SHARED`) is free-threaded and transferred uniquely via `join()`.
unsafe impl Send for SendText {}

pub fn cache_text_system(wrapped: SendText) -> Rc<TextSystem> {
    if let Some(t) = TEXT.with(|t| t.borrow().clone()) {
        return t;
    }
    let t = Rc::new(wrapped.0);
    TEXT.with(|c| *c.borrow_mut() = Some(t.clone()));
    t
}

pub fn text_system() -> Result<Rc<TextSystem>> {
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
    let gfx = Rc::new(match WARMUP_GFX.with(|w| w.borrow_mut().take()) {
        Some(handle) => match handle.join() {
            Ok(res) => res?.0,
            Err(_) => build_gfx()?,
        },
        None => build_gfx()?,
    });
    GFX.with(|g| *g.borrow_mut() = Some(gfx.clone()));
    Ok(gfx)
}

pub struct Renderer {
    gfx: Rc<Gfx>,
    text: Rc<TextSystem>,
    chrome: Chrome,
    swap: IDXGISwapChain1,
    ctx: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    // Held so the composition tree stays alive as long as the renderer.
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    first_present: bool,
    present_sync: bool,
    #[cfg(test)]
    generation: u64,
    next_draw_error: Option<HRESULT>,
}

fn d3d_device() -> Result<ID3D11Device> {
    let mut device = None;
    // Prefer WARP for 2D retained UI: avoids loading ~45-50 MB of GPU shader-compiler
    // working set while keeping DirectComposition, Mica backdrop, and 60fps redraws intact.
    for driver in [D3D_DRIVER_TYPE_WARP, D3D_DRIVER_TYPE_HARDWARE] {
        // SAFETY: the out pointer is a valid Option slot; no software module.
        let r = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
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
            let brush = ctx.CreateSolidColorBrush(&color(Palette::LIGHT.text), None)?;

            // `topmost = false` keeps the DirectComposition tree behind child
            // HWNDs (such as RichEdit fields) so native child controls stay visible.
            let target = gfx.dcomp.CreateTargetForHwnd(hwnd, false)?;
            let visual = gfx.dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            gfx.dcomp.Commit()?;
            #[cfg(test)]
            let generation = RENDERER_GEN.fetch_add(1, Ordering::Relaxed) + 1;
            Ok(Self {
                gfx,
                text,
                chrome: Chrome::new(),
                swap,
                ctx,
                brush,
                _target: target,
                _visual: visual,
                first_present: true,
                present_sync: true,
                #[cfg(test)]
                generation,
                next_draw_error: None,
            })
        }
    }

    pub fn set_present_sync(&mut self, sync: bool) {
        self.present_sync = sync;
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub fn inject_draw_error(&mut self, hr: HRESULT) {
        self.next_draw_error = Some(hr);
    }

    pub fn text(&self) -> &TextSystem {
        &self.text
    }

    pub fn set_text_scale(&self, scale: f32) -> bool {
        self.text.set_text_scale(scale)
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

    /// Draws one frame from optional retained `Tree`, title bar chrome, and focus ring.
    pub fn draw_frame(&mut self, opts: FrameOpts<'_>) -> std::result::Result<(), DrawError> {
        if let Some(hr) = self.next_draw_error.take() {
            return Err(DrawError::from(windows::core::Error::from(hr)));
        }
        let palette = opts.palette;
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
            self.ctx.SetDpi(opts.dpi as f32, opts.dpi as f32);
            self.ctx.BeginDraw();

            let top_offset = if opts.show_title_bar {
                TITLE_BAR_DIP
            } else {
                0.0
            };
            if opts.mica {
                let clear = D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                };
                self.ctx.Clear(Some(&clear));
                self.fill(0.0, top_offset, opts.w, opts.h, palette.surface);
            } else {
                self.ctx.Clear(Some(&color(palette.surface)));
                if opts.show_title_bar {
                    self.fill(0.0, 0.0, opts.w, TITLE_BAR_DIP, palette.title_bar);
                }
            }

            if let Some(tree) = opts.tree {
                self.draw_tree(tree, palette, opts.render_rich_slots);
            }

            if let Some(ring) = opts.focus_ring {
                let off = palette.focus_ring_offset;
                self.stroke_rect(
                    Rect {
                        x: ring.x - off,
                        y: ring.y - off,
                        w: ring.w + off * 2.0,
                        h: ring.h + off * 2.0,
                    },
                    palette.accent,
                    palette.focus_ring_width,
                    8.0,
                );
            }

            if opts.show_title_bar {
                self.caption_buttons(opts.w, opts.hover, palette);
                self.chrome.layout(opts.w, &self.text);
                let t = self.chrome.title_rect();
                self.draw_text(
                    TextFont::Ui,
                    strings::APP_NAME,
                    TITLE_SIZE,
                    Weight::SemiBold,
                    Align::Left,
                    Wrap::NoWrap,
                    palette.text,
                    d2d_rect(t),
                );
            }

            let end = self.ctx.EndDraw(None, None);
            self.ctx.SetTarget(None);
            end?;
            let sync_interval = if self.first_present || !self.present_sync {
                0
            } else {
                1
            };
            self.swap.Present(sync_interval, DXGI_PRESENT(0)).ok()?;
            self.first_present = false;
        }
        Ok(())
    }

    unsafe fn draw_tree(&self, tree: &Tree, palette: &Palette, render_rich_slots: bool) {
        tree.visit_paint(&mut |data, rect, step| match step {
            PaintStep::Enter => {
                if data.paint.clip {
                    self.ctx
                        .PushAxisAlignedClip(&d2d_rect(rect), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                }
                if let Some(bg) = data.paint.bg {
                    self.fill_rect(rect, bg, data.paint.radius);
                }
                if let Some(bc) = data.paint.border_color {
                    if data.paint.border_width > 0.0 {
                        self.stroke_rect(rect, bc, data.paint.border_width, data.paint.radius);
                    }
                }
                if let Some(leaf) = data.leaf.as_ref() {
                    self.draw_leaf(leaf, rect, palette, data.disabled, render_rich_slots);
                }
            }
            PaintStep::Leave => {
                if data.paint.clip {
                    self.ctx.PopAxisAlignedClip();
                }
            }
        });
    }

    unsafe fn draw_leaf(
        &self,
        leaf: &Leaf,
        rect: Rect,
        palette: &Palette,
        disabled: bool,
        render_rich_slots: bool,
    ) {
        match leaf {
            Leaf::Text { text, size, weight } => {
                self.draw_text(
                    TextFont::Ui,
                    text,
                    *size,
                    *weight,
                    Align::Left,
                    Wrap::NoWrap,
                    palette.text,
                    d2d_rect(rect),
                );
            }
            Leaf::StyledText {
                text,
                size,
                weight,
                align,
                wrap,
                color,
            } => {
                self.draw_text(
                    TextFont::Ui,
                    text,
                    *size,
                    *weight,
                    *align,
                    *wrap,
                    *color,
                    d2d_rect(rect),
                );
            }
            Leaf::Icon { glyph, size, color } => {
                let mut buf = [0u8; 4];
                let s = glyph.encode_utf8(&mut buf);
                self.draw_text(
                    TextFont::Icons,
                    s,
                    *size,
                    Weight::Regular,
                    Align::Center,
                    Wrap::NoWrap,
                    *color,
                    d2d_rect(rect),
                );
            }
            Leaf::Canvas(cmds) => {
                for cmd in cmds {
                    self.draw_cmd(cmd, rect.x, rect.y);
                }
            }
            Leaf::RichTextSlot {
                value,
                placeholder,
                multiline,
                ..
            } if render_rich_slots => {
                let (txt, col) = if !value.is_empty() {
                    (
                        value.as_str(),
                        if disabled {
                            palette.faint
                        } else {
                            palette.text
                        },
                    )
                } else {
                    (placeholder.as_str(), palette.faint)
                };
                if !txt.is_empty() {
                    self.draw_text(
                        TextFont::Ui,
                        txt,
                        13.0,
                        Weight::Regular,
                        Align::Left,
                        if *multiline { Wrap::Word } else { Wrap::NoWrap },
                        col,
                        d2d_rect(rect),
                    );
                }
            }
            Leaf::RichTextSlot { .. } | Leaf::Box => {}
        }
    }

    unsafe fn draw_cmd(&self, cmd: &DrawCmd, ox: f32, oy: f32) {
        match cmd {
            DrawCmd::FillRect {
                rect,
                color,
                radius,
            } => {
                self.fill_rect(offset_rect(*rect, ox, oy), *color, *radius);
            }
            DrawCmd::StrokeRect {
                rect,
                color,
                width,
                radius,
            } => {
                self.stroke_rect(offset_rect(*rect, ox, oy), *color, *width, *radius);
            }
            DrawCmd::FillCircle {
                cx,
                cy,
                r,
                color: c,
            } => {
                self.brush.SetColor(&color(*c));
                self.ctx.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: Vector2 {
                            X: ox + cx,
                            Y: oy + cy,
                        },
                        radiusX: *r,
                        radiusY: *r,
                    },
                    &self.brush,
                );
            }
            DrawCmd::StrokeArc {
                cx,
                cy,
                r,
                start_deg,
                sweep_deg,
                width,
                color: c,
                round_cap,
            } => {
                self.draw_arc(
                    ox + cx,
                    oy + cy,
                    *r,
                    *start_deg,
                    *sweep_deg,
                    *width,
                    *c,
                    *round_cap,
                );
            }
            DrawCmd::Line {
                x1,
                y1,
                x2,
                y2,
                width,
                color: c,
            } => {
                self.brush.SetColor(&color(*c));
                self.ctx.DrawLine(
                    Vector2 {
                        X: ox + x1,
                        Y: oy + y1,
                    },
                    Vector2 {
                        X: ox + x2,
                        Y: oy + y2,
                    },
                    &self.brush,
                    *width,
                    None,
                );
            }
            DrawCmd::Text {
                rect,
                text,
                size,
                weight,
                align,
                wrap,
                color,
            } => {
                self.draw_text(
                    TextFont::Ui,
                    text,
                    *size,
                    *weight,
                    *align,
                    *wrap,
                    *color,
                    d2d_rect(offset_rect(*rect, ox, oy)),
                );
            }
            DrawCmd::Icon {
                rect,
                glyph,
                size,
                color,
            } => {
                let mut buf = [0u8; 4];
                let s = glyph.encode_utf8(&mut buf);
                self.draw_text(
                    TextFont::Icons,
                    s,
                    *size,
                    Weight::Regular,
                    Align::Center,
                    Wrap::NoWrap,
                    *color,
                    d2d_rect(offset_rect(*rect, ox, oy)),
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_arc(
        &self,
        cx: f32,
        cy: f32,
        r: f32,
        start_deg: f32,
        sweep_deg: f32,
        width: f32,
        c: Color,
        round_cap: bool,
    ) {
        if r <= 0.0 || sweep_deg <= 0.0 {
            return;
        }
        let sweep = sweep_deg.min(359.9);
        let a0 = (start_deg - 90.0).to_radians();
        let a1 = (start_deg + sweep - 90.0).to_radians();
        let p0 = Vector2 {
            X: cx + r * a0.cos(),
            Y: cy + r * a0.sin(),
        };
        let p1 = Vector2 {
            X: cx + r * a1.cos(),
            Y: cy + r * a1.sin(),
        };
        let Ok(path) = self.gfx.d2d_factory.CreatePathGeometry() else {
            return;
        };
        let Ok(sink) = path.Open() else {
            return;
        };
        sink.BeginFigure(p0, D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: p1,
            size: D2D_SIZE_F {
                width: r,
                height: r,
            },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: if sweep >= 180.0 {
                D2D1_ARC_SIZE_LARGE
            } else {
                D2D1_ARC_SIZE_SMALL
            },
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        if sink.Close().is_err() {
            return;
        }
        self.brush.SetColor(&color(c));
        let style = if round_cap {
            Some(&self.gfx.round_stroke)
        } else {
            None
        };
        self.ctx.DrawGeometry(&path, &self.brush, width, style);
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

    unsafe fn fill_rect(&self, rect: Rect, c: Color, radius: f32) {
        self.brush.SetColor(&color(c));
        if radius <= 0.0 {
            self.ctx.FillRectangle(&d2d_rect(rect), &self.brush);
        } else {
            self.ctx.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: d2d_rect(rect),
                    radiusX: radius,
                    radiusY: radius,
                },
                &self.brush,
            );
        }
    }

    unsafe fn stroke_rect(&self, rect: Rect, c: Color, width: f32, radius: f32) {
        self.brush.SetColor(&color(c));
        let inset = width * 0.5;
        let r = D2D_RECT_F {
            left: rect.x + inset,
            top: rect.y + inset,
            right: (rect.x + rect.w - inset).max(rect.x + inset),
            bottom: (rect.y + rect.h - inset).max(rect.y + inset),
        };
        if radius <= 0.0 {
            self.ctx.DrawRectangle(&r, &self.brush, width, None);
        } else {
            self.ctx.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: r,
                    radiusX: radius,
                    radiusY: radius,
                },
                &self.brush,
                width,
                None,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_text(
        &self,
        font: TextFont,
        text: &str,
        size: f32,
        weight: Weight,
        align: Align,
        wrap: Wrap,
        c: Color,
        rect: D2D_RECT_F,
    ) {
        let Ok(format) = self.text.format_styled(font, size, weight, align, wrap) else {
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

    unsafe fn caption_buttons(&self, w: f32, hover: Hover, palette: &Palette) {
        let h = TITLE_BAR_DIP;
        let kinds = [Hover::Minimize, Hover::Maximize, Hover::Close];
        for (i, kind) in kinds.into_iter().enumerate() {
            let left = w - BUTTON_DIP * (3 - i) as f32;
            let right = left + BUTTON_DIP;
            let mut ink = palette.text;
            if hover == kind {
                if kind == Hover::Close {
                    self.fill(left, 0.0, right, h, palette.close_hover);
                    ink = palette.on_close_hover;
                } else {
                    self.fill(left, 0.0, right, h, palette.button_hover);
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

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: `self` owns these live COM interfaces; releasing targets and
        // flushing device state prevents D2D/D3D11/DComp deferred queues from
        // retaining glyph-atlas or staging allocations across window closes.
        unsafe {
            self.ctx.SetTarget(None);
            if let Ok(d2d) = self.ctx.GetDevice() {
                d2d.ClearResources(0);
            }
            let _ = self._visual.SetContent(None);
            let _ = self._target.SetRoot(None);
            let _ = self.gfx.dcomp.Commit();
            let _ = self.gfx.dcomp.WaitForCommitCompletion();
            if let Ok(d3d) = self.swap.GetDevice::<ID3D11Device>() {
                if let Ok(dc) = d3d.GetImmediateContext() {
                    dc.ClearState();
                    dc.Flush();
                }
            }
        }
    }
}

fn d2d_rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

fn offset_rect(r: Rect, ox: f32, oy: f32) -> Rect {
    Rect {
        x: r.x + ox,
        y: r.y + oy,
        w: r.w,
        h: r.h,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_error_classifies_device_loss_hresults() {
        assert!(matches!(
            DrawError::from(windows::core::Error::from(D2DERR_RECREATE_TARGET)),
            DrawError::DeviceLost
        ));
        assert!(matches!(
            DrawError::from(windows::core::Error::from(DXGI_ERROR_DEVICE_REMOVED)),
            DrawError::DeviceLost
        ));
        assert!(matches!(
            DrawError::from(windows::core::Error::from(DXGI_ERROR_DEVICE_RESET)),
            DrawError::DeviceLost
        ));
        assert!(matches!(
            DrawError::from(windows::core::Error::from(E_FAIL)),
            DrawError::Other(_)
        ));
    }
}
