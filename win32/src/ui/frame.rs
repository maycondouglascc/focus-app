//! Geometry of the custom window frame. Pure functions, no Win32 calls, so the
//! behaviour is unit tested. The client area covers the whole window: the title
//! bar, the caption buttons and the resize borders are all decided here and
//! reported to Windows through `WM_NCHITTEST` (the Windows 11 Snap Layouts
//! flyout needs `HTMAXBUTTON` for the maximize button).

/// Title bar height in DIPs (device independent pixels, 96 per inch).
pub const TITLE_BAR_DIP: f32 = 32.0;
/// Width of each caption button in DIPs.
pub const BUTTON_DIP: f32 = 46.0;
/// Width of the invisible resize border in DIPs.
pub const RESIZE_DIP: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Client,
    Caption,
    Minimize,
    Maximize,
    Close,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Physical pixels per DIP at a given DPI.
pub fn scale(dpi: u32) -> f32 {
    dpi.max(1) as f32 / 96.0
}

pub fn px(dip: f32, dpi: u32) -> i32 {
    (dip * scale(dpi)).round() as i32
}

/// `x`, `y` are client coordinates in physical pixels; `w`, `h` the client size.
pub fn hit_test(x: i32, y: i32, w: i32, h: i32, dpi: u32, maximized: bool) -> Hit {
    if x < 0 || y < 0 || x >= w || y >= h {
        return Hit::Client;
    }
    if !maximized {
        let b = px(RESIZE_DIP, dpi);
        let (l, r, t, bt) = (x < b, x >= w - b, y < b, y >= h - b);
        match (l, r, t, bt) {
            (true, _, true, _) => return Hit::TopLeft,
            (_, true, true, _) => return Hit::TopRight,
            (true, _, _, true) => return Hit::BottomLeft,
            (_, true, _, true) => return Hit::BottomRight,
            (_, _, true, _) => return Hit::Top,
            (_, _, _, true) => return Hit::Bottom,
            (true, ..) => return Hit::Left,
            (_, true, ..) => return Hit::Right,
            _ => {}
        }
    }
    if y < px(TITLE_BAR_DIP, dpi) {
        let bw = px(BUTTON_DIP, dpi);
        return match (w - x) / bw.max(1) {
            0 => Hit::Close,
            1 => Hit::Maximize,
            2 => Hit::Minimize,
            _ => Hit::Caption,
        };
    }
    Hit::Client
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: i32 = 1000;
    const H: i32 = 700;

    #[test]
    fn body_is_client_and_title_bar_is_caption() {
        assert_eq!(hit_test(500, 400, W, H, 96, false), Hit::Client);
        assert_eq!(hit_test(500, 15, W, H, 96, false), Hit::Caption);
    }

    #[test]
    fn caption_buttons_sit_at_the_right_edge() {
        assert_eq!(hit_test(W - 10, 15, W, H, 96, false), Hit::Close);
        assert_eq!(hit_test(W - 60, 15, W, H, 96, false), Hit::Maximize);
        assert_eq!(hit_test(W - 100, 15, W, H, 96, false), Hit::Minimize);
        assert_eq!(hit_test(W - 150, 15, W, H, 96, false), Hit::Caption);
    }

    #[test]
    fn buttons_scale_with_dpi() {
        // At 200% each button is 92 px wide and the bar 64 px tall.
        assert_eq!(hit_test(W - 80, 40, W, H, 192, false), Hit::Close);
        assert_eq!(hit_test(W - 100, 40, W, H, 192, false), Hit::Maximize);
        assert_eq!(hit_test(W - 80, 70, W, H, 192, false), Hit::Client);
        // At 96 dpi the same x is already on the maximize button's neighbour.
        assert_eq!(hit_test(W - 80, 15, W, H, 96, false), Hit::Maximize);
    }

    #[test]
    fn edges_and_corners_resize() {
        assert_eq!(hit_test(0, 300, W, H, 96, false), Hit::Left);
        assert_eq!(hit_test(W - 1, 300, W, H, 96, false), Hit::Right);
        assert_eq!(hit_test(300, H - 1, W, H, 96, false), Hit::Bottom);
        assert_eq!(hit_test(300, 0, W, H, 96, false), Hit::Top);
        assert_eq!(hit_test(0, 0, W, H, 96, false), Hit::TopLeft);
        assert_eq!(hit_test(W - 1, 0, W, H, 96, false), Hit::TopRight);
        assert_eq!(hit_test(0, H - 1, W, H, 96, false), Hit::BottomLeft);
        assert_eq!(hit_test(W - 1, H - 1, W, H, 96, false), Hit::BottomRight);
    }

    #[test]
    fn top_border_wins_over_the_close_button() {
        assert_eq!(hit_test(W - 3, 0, W, H, 96, false), Hit::TopRight);
        assert_eq!(hit_test(W - 60, 1, W, H, 96, false), Hit::Top);
    }

    #[test]
    fn maximized_windows_do_not_resize() {
        assert_eq!(hit_test(0, 300, W, H, 96, true), Hit::Client);
        assert_eq!(hit_test(300, 0, W, H, 96, true), Hit::Caption);
        assert_eq!(hit_test(W - 1, 0, W, H, 96, true), Hit::Close);
    }

    #[test]
    fn outside_the_window_is_client() {
        assert_eq!(hit_test(-1, 5, W, H, 96, false), Hit::Client);
        assert_eq!(hit_test(W, 5, W, H, 96, false), Hit::Client);
    }
}
