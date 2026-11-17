//! Color tokens and high-contrast palette mapping. Widgets never use literal
//! colors; everything reads from `Palette` so high contrast works across every
//! control and a dark theme can swap the table later.

use windows::Win32::Graphics::Gdi::{
    GetSysColor, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWFRAME, COLOR_WINDOWTEXT,
    SYS_COLOR_INDEX,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

pub const fn rgb(hex: u32) -> Color {
    rgba(hex, 1.0)
}

pub const fn rgba(hex: u32, a: f32) -> Color {
    Color(
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
        a,
    )
}

/// Converts a Win32 `COLORREF` (`0x00BBGGRR`) into an opaque `Color`.
pub fn bgr(colorref: u32) -> Color {
    Color(
        (colorref & 0xFF) as f32 / 255.0,
        ((colorref >> 8) & 0xFF) as f32 / 255.0,
        ((colorref >> 16) & 0xFF) as f32 / 255.0,
        1.0,
    )
}

#[allow(dead_code)] // used by charts and tinted controls in F2/F3
pub fn mix(fg: Color, bg: Color, t: f32) -> Color {
    let k = t.clamp(0.0, 1.0);
    Color(
        bg.0 + (fg.0 - bg.0) * k,
        bg.1 + (fg.1 - bg.1) * k,
        bg.2 + (fg.2 - bg.2) * k,
        bg.3 + (fg.3 - bg.3) * k,
    )
}

/// Semantic color and stroke tokens used by every widget and chart.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)] // fields consumed progressively as widgets and charts are added
pub struct Palette {
    pub surface: Color,
    pub backdrop: Color,
    pub title_bar: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub hairline: Color,
    pub card_border: Color,
    pub ctl_border: Color,
    pub ctl_bg: Color,
    pub ctl_hover: Color,
    pub ctl_pressed: Color,
    pub seg_track: Color,
    pub primary_bg: Color,
    pub primary_hover: Color,
    pub on_primary: Color,
    pub danger: Color,
    pub danger_hover: Color,
    pub danger_text: Color,
    pub danger_bg: Color,
    pub accent: Color,
    pub accent_bg: Color,
    pub done: Color,
    pub done_bg: Color,
    pub attn: Color,
    pub attn_bg: Color,
    pub disabled_text: Color,
    pub disabled_bg: Color,
    pub toggle_off: Color,
    pub toggle_on: Color,
    pub scrim: Color,
    pub tooltip_bg: Color,
    pub tooltip_text: Color,
    pub button_hover: Color,
    pub close_hover: Color,
    pub on_close_hover: Color,
    pub chart_grid: Color,
    pub chart_empty_cell: Color,
    pub border_width: f32,
    pub focus_ring_width: f32,
    pub focus_ring_offset: f32,
    pub shadows: bool,
}

impl Palette {
    /// Default light theme matching the Focus design tokens.
    pub const LIGHT: Self = Self {
        surface: rgb(0xFFFFFF),
        backdrop: rgb(0xF4F6F9),
        title_bar: rgb(0xF3F3F3),
        text: rgb(0x1B2230),
        muted: rgb(0x667085),
        faint: rgb(0x8A93A3),
        hairline: rgb(0xEEF1F5),
        card_border: rgb(0xE8ECF1),
        ctl_border: rgb(0xE4E8EE),
        ctl_bg: rgb(0xFFFFFF),
        ctl_hover: rgb(0xF8FAFC),
        ctl_pressed: rgb(0xEEF1F5),
        seg_track: rgb(0xF1F4F8),
        primary_bg: rgb(0x1B2230),
        primary_hover: rgb(0x2A3345),
        on_primary: rgb(0xFFFFFF),
        danger: rgb(0xC8372D),
        danger_hover: rgb(0xAE2E25),
        danger_text: rgb(0xB42318),
        danger_bg: rgb(0xFEF3F2),
        accent: rgb(0x3B5BDB),
        accent_bg: rgb(0xEEF2FF),
        done: rgb(0x1B8A4A),
        done_bg: rgb(0xECF8F1),
        attn: rgb(0xB4410F),
        attn_bg: rgb(0xFFF4ED),
        disabled_text: rgb(0x8A93A3),
        disabled_bg: rgb(0xF4F6F9),
        toggle_off: rgb(0xD6DCE5),
        toggle_on: rgb(0x1B2230),
        scrim: rgba(0x1B2230, 0.25),
        tooltip_bg: rgb(0x1B2230),
        tooltip_text: rgb(0xFFFFFF),
        button_hover: rgb(0xE5E5E5),
        close_hover: rgb(0xC42B1C),
        on_close_hover: rgb(0xFFFFFF),
        chart_grid: rgb(0xEEF1F5),
        chart_empty_cell: rgb(0xF4F6F9),
        border_width: 1.0,
        focus_ring_width: 2.0,
        focus_ring_offset: 2.0,
        shadows: true,
    };

    /// Builds a high-contrast palette from a `GetSysColor`-compatible lookup.
    /// Shadows are disabled and borders strengthened so edges stay legible.
    pub fn high_contrast_from_sys_colors(get: impl Fn(SYS_COLOR_INDEX) -> u32) -> Self {
        let window = bgr(get(COLOR_WINDOW));
        let window_text = bgr(get(COLOR_WINDOWTEXT));
        let btn_face = bgr(get(COLOR_BTNFACE));
        let btn_text = bgr(get(COLOR_BTNTEXT));
        let highlight = bgr(get(COLOR_HIGHLIGHT));
        let highlight_text = bgr(get(COLOR_HIGHLIGHTTEXT));
        let gray_text = bgr(get(COLOR_GRAYTEXT));
        let frame = bgr(get(COLOR_WINDOWFRAME));
        let hot = bgr(get(COLOR_HOTLIGHT));

        Self {
            surface: window,
            backdrop: window,
            title_bar: window,
            text: window_text,
            muted: window_text,
            faint: gray_text,
            hairline: frame,
            card_border: frame,
            ctl_border: btn_text,
            ctl_bg: btn_face,
            ctl_hover: highlight,
            ctl_pressed: highlight,
            seg_track: btn_face,
            primary_bg: highlight,
            primary_hover: hot,
            on_primary: highlight_text,
            danger: highlight,
            danger_hover: hot,
            danger_text: window_text,
            danger_bg: btn_face,
            accent: hot,
            accent_bg: highlight,
            done: highlight,
            done_bg: btn_face,
            attn: hot,
            attn_bg: btn_face,
            disabled_text: gray_text,
            disabled_bg: window,
            toggle_off: btn_face,
            toggle_on: highlight,
            scrim: rgba(0x000000, 0.5),
            tooltip_bg: window,
            tooltip_text: window_text,
            button_hover: highlight,
            close_hover: highlight,
            on_close_hover: highlight_text,
            chart_grid: frame,
            chart_empty_cell: btn_face,
            border_width: 2.0,
            focus_ring_width: 2.0,
            focus_ring_offset: 2.0,
            shadows: false,
        }
    }

    pub fn for_system(high_contrast: bool) -> Self {
        if !high_contrast {
            return Self::LIGHT;
        }
        // SAFETY: `GetSysColor` accepts any `SYS_COLOR_INDEX` value and reads system state.
        Self::high_contrast_from_sys_colors(|idx| unsafe { GetSysColor(idx) })
    }
}

#[allow(dead_code)] // project and chart palettes used in F2 widgets and F3 screens
pub const PROJECT_COLORS: [Color; 12] = [
    rgb(0x3B7DDD),
    rgb(0xD9488A),
    rgb(0x2F9E8F),
    rgb(0xB98A1E),
    rgb(0x6C56D9),
    rgb(0x3F8F4F),
    rgb(0xC23A4B),
    rgb(0x5B6B8C),
    rgb(0x2A9BC4),
    rgb(0x9A4FA0),
    rgb(0x78872C),
    rgb(0x2F4B8A),
];

#[allow(dead_code)]
pub const NO_PROJECT: Color = rgb(0x8A93A3);

#[allow(dead_code)]
pub const CHART_BLUE: Color = rgb(0x4C84E8);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgr_unpacks_win32_colorref_channels() {
        let c = bgr(0x00_AA_55_FF); // R=0xFF, G=0x55, B=0xAA
        assert!((c.0 - 1.0).abs() < 1e-4);
        assert!((c.1 - 85.0 / 255.0).abs() < 1e-4);
        assert!((c.2 - 170.0 / 255.0).abs() < 1e-4);
        assert_eq!(c.3, 1.0);
    }

    #[test]
    fn high_contrast_uses_sys_colors_strengthens_borders_and_drops_shadows() {
        let p = Palette::high_contrast_from_sys_colors(|idx| match idx {
            COLOR_WINDOW => 0x00_00_00_00,        // black
            COLOR_WINDOWTEXT => 0x00_FF_FF_FF,    // white
            COLOR_BTNFACE => 0x00_00_00_00,       // black
            COLOR_BTNTEXT => 0x00_00_FF_FF,       // yellow
            COLOR_HIGHLIGHT => 0x00_FF_FF_00,     // cyan
            COLOR_HIGHLIGHTTEXT => 0x00_00_00_00, // black
            COLOR_GRAYTEXT => 0x00_00_FF_00,      // green
            COLOR_WINDOWFRAME => 0x00_FF_FF_FF,   // white
            COLOR_HOTLIGHT => 0x00_FF_00_FF,      // magenta
            _ => 0,
        });
        assert_eq!(p.surface, rgb(0x000000));
        assert_eq!(p.text, rgb(0xFFFFFF));
        assert_eq!(p.ctl_border, rgb(0xFFFF00));
        assert_eq!(p.border_width, 2.0);
        assert!(!p.shadows);
    }

    #[test]
    fn light_palette_keeps_shadows_and_hairline_borders() {
        let p = Palette::for_system(false);
        assert_eq!(p.border_width, 1.0);
        assert!(p.shadows);
        assert_ne!(p.accent, p.danger);
    }
}
