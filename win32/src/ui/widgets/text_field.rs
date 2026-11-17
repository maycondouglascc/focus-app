//! Borderless `RICHEDIT50W` text input with canvas-rendered border/focus ring,
//! OS spellcheck (`IMF_SPELLCHECKING`), DPI-scaled fonts, high-contrast tinting,
//! and subclassed Tab / Shift+Tab / Esc routing.

use std::sync::OnceLock;

use taffy::prelude::*;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, DrawTextW, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE, FW_NORMAL, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::Controls::RichEdit::{
    CFM_COLOR, CHARFORMAT2W, EM_GETLANGOPTIONS, EM_SETBKGNDCOLOR, EM_SETCHARFORMAT,
    EM_SETEVENTMASK, EM_SETLANGOPTIONS, ENM_CHANGE, IMF_AUTOKEYBOARD, IMF_SPELLCHECKING,
    MSFTEDIT_CLASS, SCF_ALL, SCF_DEFAULT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, VK_ESCAPE, VK_RETURN, VK_TAB};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetClientRect, GetParent, GetWindowLongW, GetWindowTextLengthW,
    GetWindowTextW, IsWindow, SendMessageW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE,
    SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_KEYDOWN, WM_KILLFOCUS, WM_NCDESTROY, WM_PAINT, WM_SETFOCUS, WM_SETFONT, WM_SETTEXT,
    WS_BORDER, WS_CHILD, WS_EX_CLIENTEDGE, WS_TABSTOP, WS_VISIBLE,
};

pub const WM_RICHEDIT_FOCUS: u32 = WM_APP + 2;
pub const WM_RICHEDIT_BLUR: u32 = WM_APP + 3;
pub const WM_RICHEDIT_SUBMIT: u32 = WM_APP + 4;
const EM_SETREADONLY: u32 = 0x00CF;

use super::interactive_a11y;
use crate::ui::frame;
use crate::ui::theme::{to_colorref, Palette};
use crate::ui::tree::{style, A11yRole, HitMode, Leaf, Node, PaintStyle, Rect, Tree, WidgetId};

const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_WANTRETURN: u32 = 0x1000;

const SUBCLASS_ID: usize = 0x464F_4355;

static MSFTEDIT: OnceLock<bool> = OnceLock::new();

fn ensure_richedit_loaded() -> bool {
    *MSFTEDIT.get_or_init(|| {
        // SAFETY: loads the system `msftedit.dll` that registers `RICHEDIT50W`.
        unsafe { LoadLibraryW(w!("msftedit.dll")).is_ok() }
    })
}

struct SubclassData {
    placeholder: Vec<u16>,
    placeholder_color: u32,
    multiline: bool,
}

unsafe extern "system" fn richedit_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    uid: usize,
    ref_data: usize,
) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let vk = (wparam.0 & 0xFFFF) as u16;
            if vk == VK_TAB.0 || vk == VK_ESCAPE.0 {
                if let Ok(parent) = GetParent(hwnd) {
                    return SendMessageW(parent, WM_KEYDOWN, Some(wparam), Some(lparam));
                }
            } else if vk == VK_RETURN.0 && ref_data != 0 {
                let state = &*(ref_data as *const SubclassData);
                if !state.multiline {
                    if let Ok(parent) = GetParent(hwnd) {
                        let _ = SendMessageW(
                            parent,
                            WM_RICHEDIT_SUBMIT,
                            Some(WPARAM(hwnd.0 as usize)),
                            Some(lparam),
                        );
                    }
                    return LRESULT(0);
                }
            }
        }
        WM_SETFOCUS => {
            if let Ok(parent) = GetParent(hwnd) {
                let _ = SendMessageW(
                    parent,
                    WM_RICHEDIT_FOCUS,
                    Some(WPARAM(hwnd.0 as usize)),
                    Some(lparam),
                );
            }
        }
        WM_KILLFOCUS => {
            if let Ok(parent) = GetParent(hwnd) {
                let _ = SendMessageW(
                    parent,
                    WM_RICHEDIT_BLUR,
                    Some(WPARAM(hwnd.0 as usize)),
                    Some(lparam),
                );
            }
        }
        WM_PAINT => {
            let res = DefSubclassProc(hwnd, msg, wparam, lparam);
            if ref_data != 0 && GetWindowTextLengthW(hwnd) == 0 {
                let state = &*(ref_data as *const SubclassData);
                if !state.placeholder.is_empty() {
                    let hdc = GetDC(Some(hwnd));
                    if !hdc.0.is_null() {
                        let mut rc = RECT::default();
                        let _ = GetClientRect(hwnd, &mut rc);
                        SetBkMode(hdc, TRANSPARENT);
                        SetTextColor(hdc, COLORREF(state.placeholder_color));
                        let hfont = HGDIOBJ(SendMessageW(hwnd, 0x0031, None, None).0 as *mut _);
                        let old = if !hfont.0.is_null() {
                            SelectObject(hdc, hfont)
                        } else {
                            HGDIOBJ::default()
                        };
                        let mut buf = state.placeholder.clone();
                        let flags = if state.multiline {
                            DT_LEFT | DT_NOPREFIX | DT_WORDBREAK
                        } else {
                            DT_LEFT | DT_NOPREFIX | DT_SINGLELINE | DT_VCENTER
                        };
                        let _ = DrawTextW(hdc, &mut buf, &mut rc, flags);
                        if !old.0.is_null() {
                            let _ = SelectObject(hdc, old);
                        }
                        let _ = ReleaseDC(Some(hwnd), hdc);
                    }
                }
            }
            return res;
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(richedit_subclass_proc), uid);
            if ref_data != 0 {
                drop(Box::from_raw(ref_data as *mut SubclassData));
            }
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

#[derive(Clone, Debug)]
pub struct TextFieldProps<'a> {
    pub key: &'a str,
    pub aria_label: &'a str,
    pub placeholder: &'a str,
    pub value: &'a str,
    pub multiline: bool,
    pub min_height: f32,
    pub focused: bool,
    pub error: bool,
    pub disabled: bool,
    pub sync_rev: u64,
}

impl<'a> TextFieldProps<'a> {
    pub fn simple(key: &'a str, aria_label: &'a str, value: &'a str) -> Self {
        Self {
            key,
            aria_label,
            placeholder: "",
            value,
            multiline: false,
            min_height: 32.0,
            focused: false,
            error: false,
            disabled: false,
            sync_rev: 0,
        }
    }
}

/// Builds the outer bordered container and its inner `Leaf::RichTextSlot` in `tree`.
/// Returns `(container_node, slot_id)`.
pub fn build_tree_node(
    tree: &mut Tree,
    parent: Node,
    props: &TextFieldProps<'_>,
    palette: &Palette,
) -> (Node, WidgetId) {
    let min_h = if props.multiline {
        props.min_height.max(68.0)
    } else {
        32.0
    };
    let container = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(if props.multiline {
                AlignItems::Stretch
            } else {
                AlignItems::Center
            }),
            padding: style::pad(10.0, 6.0, 10.0, 6.0),
            size: Size {
                width: percent(1.0_f32),
                height: auto(),
            },
            min_size: Size {
                width: length(80.0_f32),
                height: length(min_h),
            },
            ..Default::default()
        },
    );
    let border_color = if props.disabled {
        palette.ctl_border
    } else if props.error {
        palette.danger
    } else if props.focused {
        palette.accent
    } else {
        palette.ctl_border
    };
    let bg = if props.disabled {
        palette.disabled_bg
    } else {
        palette.ctl_bg
    };
    tree.set_paint(
        container,
        PaintStyle {
            bg: Some(bg),
            border_color: Some(border_color),
            border_width: if props.focused {
                palette.border_width.max(1.5)
            } else {
                palette.border_width
            },
            radius: 6.0,
            clip: true,
        },
    );

    let slot = tree.add_leaf(
        container,
        Style {
            flex_grow: 1.0,
            size: Size {
                width: auto(),
                height: if props.multiline {
                    length((min_h - 12.0).max(40.0))
                } else {
                    length(20.0_f32)
                },
            },
            ..Default::default()
        },
        Leaf::RichTextSlot {
            key: props.key.into(),
            placeholder: props.placeholder.into(),
            value: props.value.into(),
            multiline: props.multiline,
            sync_rev: props.sync_rev,
        },
    );
    tree.set_hit(slot, HitMode::Interactive);
    tree.set_focusable(slot, true);
    tree.set_disabled(slot, props.disabled);
    let mut info = interactive_a11y(A11yRole::TextInput, props.aria_label, props.disabled);
    info.value = Some(props.value.into());
    tree.set_a11y(slot, info);

    let slot_id = if props.key.is_empty() {
        tree.id(slot)
    } else {
        tree.set_key(slot, props.key)
    };
    (container, slot_id)
}

/// Live native `RICHEDIT50W` child window synchronized with a `Leaf::RichTextSlot`.
pub struct RichEditField {
    hwnd: HWND,
    slot_id: WidgetId,
    key: String,
    sync_rev: u64,
    disabled: bool,
    font: HFONT,
    #[allow(dead_code)] // verified by M8 DPI unit tests
    font_height_px: i32,
    subclass_ptr: *mut SubclassData,
}

impl RichEditField {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        parent: HWND,
        slot_id: WidgetId,
        key: impl Into<String>,
        sync_rev: u64,
        multiline: bool,
        placeholder: &str,
        palette: &Palette,
        dpi: u32,
        text_scale: f32,
    ) -> Option<Self> {
        if !ensure_richedit_loaded() {
            return None;
        }
        // SAFETY: `parent` is a live window owned by the UI thread.
        unsafe {
            let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
            let mut style_flags = WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0;
            if multiline {
                style_flags |= ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN;
            } else {
                style_flags |= ES_AUTOHSCROLL;
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                MSFTEDIT_CLASS,
                w!(""),
                WINDOW_STYLE(style_flags),
                0,
                0,
                100,
                20,
                Some(parent),
                None,
                Some(hinstance),
                None,
            )
            .ok()?;

            let cur_lang = SendMessageW(hwnd, EM_GETLANGOPTIONS, None, None).0 as u32;
            let _ = SendMessageW(
                hwnd,
                EM_SETLANGOPTIONS,
                None,
                Some(LPARAM(
                    (cur_lang | IMF_SPELLCHECKING | IMF_AUTOKEYBOARD) as isize,
                )),
            );
            let _ = SendMessageW(
                hwnd,
                EM_SETEVENTMASK,
                None,
                Some(LPARAM(ENM_CHANGE as isize)),
            );

            let placeholder_utf16: Vec<u16> = placeholder.encode_utf16().collect();
            let subclass_ptr = Box::into_raw(Box::new(SubclassData {
                placeholder: placeholder_utf16,
                placeholder_color: to_colorref(palette.faint),
                multiline,
            }));
            let _ = SetWindowSubclass(
                hwnd,
                Some(richedit_subclass_proc),
                SUBCLASS_ID,
                subclass_ptr as usize,
            );

            let mut field = Self {
                hwnd,
                slot_id,
                key: key.into(),
                sync_rev,
                disabled: false,
                font: HFONT::default(),
                font_height_px: 0,
                subclass_ptr,
            };
            field.sync_theme(palette, dpi, text_scale);
            Some(field)
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    #[cfg(test)]
    pub fn font_height_px(&self) -> i32 {
        self.font_height_px
    }

    pub fn slot_id(&self) -> WidgetId {
        self.slot_id
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn sync_spec(
        &mut self,
        slot_id: WidgetId,
        value: &str,
        placeholder: &str,
        sync_rev: u64,
        disabled: bool,
        palette: &Palette,
    ) {
        self.slot_id = slot_id;
        if sync_rev != self.sync_rev {
            self.sync_rev = sync_rev;
            self.set_text(value);
        }
        if !self.subclass_ptr.is_null() {
            // SAFETY: `subclass_ptr` stays alive until `WM_NCDESTROY` during `DestroyWindow` in `Drop`.
            unsafe {
                let want: Vec<u16> = placeholder.encode_utf16().collect();
                if (*self.subclass_ptr).placeholder != want {
                    (*self.subclass_ptr).placeholder = want;
                }
            }
        }
        if disabled != self.disabled {
            self.disabled = disabled;
            // SAFETY: `self.hwnd` is our owned child window.
            unsafe {
                let _ = SendMessageW(
                    self.hwnd,
                    EM_SETREADONLY,
                    Some(WPARAM(usize::from(disabled))),
                    None,
                );
                let _ = EnableWindow(self.hwnd, !disabled);
                let bg = if disabled {
                    palette.disabled_bg
                } else {
                    palette.ctl_bg
                };
                let _ = SendMessageW(
                    self.hwnd,
                    EM_SETBKGNDCOLOR,
                    Some(WPARAM(0)),
                    Some(LPARAM(to_colorref(bg) as isize)),
                );
            }
        }
    }

    /// Repositions and shows/hides the native `RICHEDIT50W` window to match the tree slot.
    pub fn sync_bounds(&self, rect_dip: Rect, hidden: bool) {
        // SAFETY: `self.hwnd` is our owned child window.
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                return;
            }
            if hidden || rect_dip.w <= 1.0 || rect_dip.h <= 1.0 {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            let scale = frame::scale(GetDpiForWindow(self.hwnd));
            let _ = SetWindowPos(
                self.hwnd,
                None,
                (rect_dip.x * scale).round() as i32,
                (rect_dip.y * scale).round() as i32,
                (rect_dip.w * scale).round().max(1.0) as i32,
                (rect_dip.h * scale).round().max(1.0) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        }
    }

    /// Updates font metrics, interior background, and text color to match `Palette`, DPI, and text scale.
    pub fn sync_theme(&mut self, palette: &Palette, dpi: u32, text_scale: f32) {
        // SAFETY: `self.hwnd` is our live child HWND.
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                return;
            }
            let scale = frame::scale(dpi);
            let height = -((13.0 * scale * text_scale.clamp(0.5, 4.0)).round() as i32);
            let new_font = CreateFontW(
                height,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                w!("Segoe UI"),
            );
            if !new_font.0.is_null() {
                let _ = SendMessageW(
                    self.hwnd,
                    WM_SETFONT,
                    Some(WPARAM(new_font.0 as usize)),
                    Some(LPARAM(1)),
                );
                if !self.font.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(self.font.0));
                }
                self.font = new_font;
                self.font_height_px = height;
            }

            let _ = SendMessageW(
                self.hwnd,
                EM_SETBKGNDCOLOR,
                Some(WPARAM(0)),
                Some(LPARAM(to_colorref(palette.ctl_bg) as isize)),
            );
            let mut cf = CHARFORMAT2W::default();
            cf.Base.cbSize = std::mem::size_of::<CHARFORMAT2W>() as u32;
            cf.Base.dwMask = CFM_COLOR;
            cf.Base.crTextColor = COLORREF(to_colorref(palette.text));
            let _ = SendMessageW(
                self.hwnd,
                EM_SETCHARFORMAT,
                Some(WPARAM((SCF_DEFAULT | SCF_ALL) as usize)),
                Some(LPARAM((&cf as *const CHARFORMAT2W) as isize)),
            );
            if !self.subclass_ptr.is_null() {
                (*self.subclass_ptr).placeholder_color = to_colorref(palette.faint);
            }
        }
    }

    pub fn set_text(&self, text: &str) {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        // SAFETY: `wide` is NUL-terminated for synchronous `SendMessageW(WM_SETTEXT)`.
        unsafe {
            let _ = SendMessageW(
                self.hwnd,
                WM_SETTEXT,
                None,
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
    }

    pub fn text(&self) -> String {
        // SAFETY: reads length + UTF-16 buffer from our child window.
        unsafe {
            let len = GetWindowTextLengthW(self.hwnd);
            if len <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; (len + 1) as usize];
            let read = GetWindowTextW(self.hwnd, &mut buf).max(0) as usize;
            String::from_utf16_lossy(&buf[..read])
        }
    }

    pub fn has_spellchecking(&self) -> bool {
        // SAFETY: queries `EM_GETLANGOPTIONS` on our live child HWND.
        let opts = unsafe { SendMessageW(self.hwnd, EM_GETLANGOPTIONS, None, None).0 as u32 };
        (opts & IMF_SPELLCHECKING) != 0
    }

    pub fn is_borderless(&self) -> bool {
        // SAFETY: reads standard style words of our child HWND.
        unsafe {
            let style = GetWindowLongW(self.hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongW(self.hwnd, GWL_EXSTYLE) as u32;
            (style & WS_BORDER.0) == 0 && (ex & WS_EX_CLIENTEDGE.0) == 0
        }
    }
}

impl Drop for RichEditField {
    fn drop(&mut self) {
        // SAFETY: destroys the owned child HWND (which triggers `WM_NCDESTROY` cleanup) and GDI font.
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
            if !self.font.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(self.font.0));
                self.font = HFONT::default();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::render::reset_shared;
    use crate::ui::text::TextSystem;
    use crate::ui::window;

    #[test]
    fn richedit_text_field_is_borderless_spellchecked_and_tracks_slot_bounds() {
        crate::ui::init();
        assert!(window::open());
        window::pump();

        let text_sys = TextSystem::new().expect("dwrite");
        let mut tree = Tree::new(Style {
            size: style::size(320.0, 120.0),
            ..style::column()
        });
        let root = tree.root();
        let (_container, slot_id) = build_tree_node(
            &mut tree,
            root,
            &TextFieldProps {
                key: "test.note",
                aria_label: "Task note",
                placeholder: "Write a short note…",
                value: "",
                multiline: false,
                min_height: 32.0,
                focused: true,
                error: false,
                disabled: false,
                sync_rev: 0,
            },
            &Palette::LIGHT,
        );
        tree.layout(320.0, 120.0, &text_sys);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        let slots = tree.rich_text_slots();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].id, slot_id);
        assert!(!slots[0].hidden);
        assert!(slots[0].rect.w > 100.0);

        let owner = window::hwnd().expect("main window hwnd");
        // SAFETY: `owner` is our live test window owned by this thread.
        let dpi = unsafe { GetDpiForWindow(owner) };
        let field = RichEditField::create(
            owner,
            slot_id,
            "test.note",
            0,
            false,
            "Write a short note…",
            &Palette::LIGHT,
            dpi,
            1.0,
        )
        .expect("RICHEDIT50W child");

        assert!(
            field.is_borderless(),
            "RICHEDIT50W must have no native border"
        );
        assert!(
            field.has_spellchecking(),
            "IMF_SPELLCHECKING must be enabled"
        );

        field.sync_bounds(slots[0].rect, slots[0].hidden);
        field.set_text("Focus review");
        assert_eq!(field.text(), "Focus review");

        drop(field);
        window::close();
        window::pump();
        reset_shared();
    }
}
