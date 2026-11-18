//! Pure-Rust anti-aliased tray icon rasterizer, Win32 `HICON` builder, and `.ico` encoder.
//!
//! Renders the Focus clock/ring mark directly into RGBA buffers using signed
//! distance fields so tray startup never loads Direct2D, Direct3D, or external
//! image crates.

use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, HICON, IMAGE_FLAGS,
};

/// Visual state reflected in the taskbar notification area icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconState {
    Stopped,
    /// Timer running; `progress_steps` is `0..=60` (1 step = 1 minute around the dial).
    Running {
        progress_steps: u8,
    },
    Paused,
    Error,
}

/// Taskbar background polarity (`SystemUsesLightTheme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskbarTheme {
    Dark,
    Light,
}

#[derive(Clone, Copy)]
struct Rgba(f32, f32, f32, f32);

impl Rgba {
    const TRANSPARENT: Self = Self(0.0, 0.0, 0.0, 0.0);

    const fn from_hex(hex: u32, a: f32) -> Self {
        Self(
            ((hex >> 16) & 0xFF) as f32 / 255.0,
            ((hex >> 8) & 0xFF) as f32 / 255.0,
            (hex & 0xFF) as f32 / 255.0,
            a,
        )
    }

    fn over(self, dst: Self) -> Self {
        let sa = self.3.clamp(0.0, 1.0);
        if sa <= 0.0 {
            return dst;
        }
        let da = dst.3.clamp(0.0, 1.0);
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 1e-6 {
            return Self::TRANSPARENT;
        }
        let r = (self.0 * sa + dst.0 * da * (1.0 - sa)) / out_a;
        let g = (self.1 * sa + dst.1 * da * (1.0 - sa)) / out_a;
        let b = (self.2 * sa + dst.2 * da * (1.0 - sa)) / out_a;
        Self(r, g, b, out_a)
    }

    fn with_alpha(self, a: f32) -> Self {
        Self(self.0, self.1, self.2, (self.3 * a).clamp(0.0, 1.0))
    }

    fn to_u8(self) -> [u8; 4] {
        [
            (self.0.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.1.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.2.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.3.clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }
}

#[inline]
fn cov(dist: f32, aa: f32) -> f32 {
    (0.5 - dist / aa).clamp(0.0, 1.0)
}

#[inline]
fn sd_circle(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    ((px - cx).hypot(py - cy)) - r
}

#[inline]
fn sd_ring(px: f32, py: f32, cx: f32, cy: f32, r: f32, half_w: f32) -> f32 {
    sd_circle(px, py, cx, cy, r).abs() - half_w
}

#[inline]
fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, radius: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let denom = bax * bax + bay * bay;
    let h = if denom > 1e-8 {
        ((pax * bax + pay * bay) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (pax - bax * h).hypot(pay - bay * h) - radius
}

/// Signed distance to a rounded box centered at `(cx, cy)` with half-extents `(hx, hy)`.
#[inline]
fn sd_round_box(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hx - r);
    let qy = (py - cy).abs() - (hy - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Shortest angular distance factor along a clockwise arc starting at 12 o'clock.
fn clockwise_arc_dist(px: f32, py: f32, cx: f32, cy: f32, r: f32, half_w: f32, frac: f32) -> f32 {
    let f = frac.clamp(0.0, 1.0);
    if f <= 0.005 {
        return 1e6;
    }
    if f >= 0.995 {
        return sd_ring(px, py, cx, cy, r, half_w);
    }
    let dx = px - cx;
    let dy = py - cy;
    // Angle clockwise from top (0 = 12 o'clock, Tau/4 = 3 o'clock).
    let mut theta = dx.atan2(-dy);
    if theta < 0.0 {
        theta += std::f32::consts::TAU;
    }
    let sweep = f * std::f32::consts::TAU;
    if theta <= sweep {
        sd_ring(px, py, cx, cy, r, half_w)
    } else {
        // Check distance to the two rounded caps at angle 0 and angle `sweep`.
        let cap0 = sd_circle(px, py, cx, cy - r, half_w);
        let cap1 = sd_circle(px, py, cx + r * sweep.sin(), cy - r * sweep.cos(), half_w);
        cap0.min(cap1)
    }
}

/// Renders an anti-aliased RGBA image (`len = size * size * 4`) for the tray icon.
pub fn render_tray_rgba(size: u32, state: IconState, theme: TaskbarTheme) -> Vec<u8> {
    let s = size.max(1) as f32;
    let aa = (1.15 / s).max(0.012);
    let mut out = vec![0u8; (size * size * 4) as usize];

    let base_ink = match theme {
        TaskbarTheme::Dark => Rgba::from_hex(0xF3F4F6, 1.0),
        TaskbarTheme::Light => Rgba::from_hex(0x1B2230, 1.0),
    };
    let track_ink = match theme {
        TaskbarTheme::Dark => Rgba::from_hex(0xF3F4F6, 0.30),
        TaskbarTheme::Light => Rgba::from_hex(0x1B2230, 0.28),
    };
    let active_accent = match theme {
        TaskbarTheme::Dark => Rgba::from_hex(0x5B8CFF, 1.0),
        TaskbarTheme::Light => Rgba::from_hex(0x3B5BDB, 1.0),
    };
    let amber = Rgba::from_hex(0xD97706, 1.0);
    let danger = match theme {
        TaskbarTheme::Dark => Rgba::from_hex(0xEF4444, 1.0),
        TaskbarTheme::Light => Rgba::from_hex(0xC8372D, 1.0),
    };
    let badge_fg = Rgba::from_hex(0xFFFFFF, 1.0);

    let cx = 0.5;
    let cy = 0.5;
    let r = 0.37;
    let stroke = 0.055; // half-width

    for y in 0..size {
        for x in 0..size {
            let u = (x as f32 + 0.5) / s;
            let v = (y as f32 + 0.5) / s;
            let mut px_col = Rgba::TRANSPARENT;

            match state {
                IconState::Stopped => {
                    let ring_cov = cov(sd_ring(u, v, cx, cy, r, stroke), aa);
                    let d_hour = sd_segment(u, v, cx, cy, cx, cy - 0.19, 0.042);
                    let d_min = sd_segment(u, v, cx, cy, cx + 0.14, cy + 0.06, 0.036);
                    let hands_cov = cov(d_hour.min(d_min), aa);
                    px_col = base_ink.with_alpha(ring_cov.max(hands_cov)).over(px_col);
                }
                IconState::Running { progress_steps } => {
                    // Subtle full track so the ring silhouette stays anchored.
                    let track_cov = cov(sd_ring(u, v, cx, cy, r, stroke), aa);
                    px_col = track_ink.with_alpha(track_cov).over(px_col);

                    // Ensure even at minute 0 a bright 15% starting arc + pulsing center dot show running state.
                    let frac = (progress_steps.min(60) as f32 / 60.0).max(0.15);
                    let arc_cov = cov(clockwise_arc_dist(u, v, cx, cy, r, stroke * 1.1, frac), aa);
                    px_col = active_accent.with_alpha(arc_cov).over(px_col);

                    // Crisp hands / active core.
                    let angle = (progress_steps as f32 / 60.0) * std::f32::consts::TAU;
                    let hx = cx + 0.17 * angle.sin();
                    let hy = cy - 0.17 * angle.cos();
                    let d_hand = sd_segment(u, v, cx, cy, hx, hy, 0.038);
                    let d_dot = sd_circle(u, v, cx, cy, 0.08);
                    px_col = base_ink.with_alpha(cov(d_hand, aa)).over(px_col);
                    px_col = active_accent.with_alpha(cov(d_dot, aa)).over(px_col);
                }
                IconState::Paused => {
                    let ring_cov = cov(sd_ring(u, v, cx, cy, r, stroke), aa);
                    px_col = base_ink.with_alpha(ring_cov * 0.55).over(px_col);

                    // Solid rounded pause pill in the lower-right / foreground so paused state is unmistakable.
                    let bx = 0.67;
                    let by = 0.68;
                    let br = 0.25;
                    let halo = cov(sd_circle(u, v, bx, by, br + 0.045), aa);
                    if halo > 0.0 && matches!(theme, TaskbarTheme::Dark) {
                        px_col = Rgba::from_hex(0x111827, halo * 0.75).over(px_col);
                    } else if halo > 0.0 {
                        px_col = Rgba::from_hex(0xFFFFFF, halo * 0.85).over(px_col);
                    }
                    let b_cov = cov(sd_circle(u, v, bx, by, br), aa);
                    px_col = amber.with_alpha(b_cov).over(px_col);

                    let bar1 = sd_round_box(u, v, bx - 0.065, by, 0.032, 0.11, 0.015);
                    let bar2 = sd_round_box(u, v, bx + 0.065, by, 0.032, 0.11, 0.015);
                    let bar_cov = cov(bar1.min(bar2), aa);
                    px_col = badge_fg.with_alpha(bar_cov).over(px_col);
                }
                IconState::Error => {
                    let ring_cov = cov(sd_ring(u, v, cx, cy, r, stroke), aa);
                    px_col = base_ink.with_alpha(ring_cov * 0.65).over(px_col);

                    let bx = 0.67;
                    let by = 0.68;
                    let br = 0.25;
                    let halo = cov(sd_circle(u, v, bx, by, br + 0.045), aa);
                    if halo > 0.0 {
                        let bg = match theme {
                            TaskbarTheme::Dark => 0x111827,
                            TaskbarTheme::Light => 0xFFFFFF,
                        };
                        px_col = Rgba::from_hex(bg, halo * 0.85).over(px_col);
                    }
                    let b_cov = cov(sd_circle(u, v, bx, by, br), aa);
                    px_col = danger.with_alpha(b_cov).over(px_col);

                    // `!` inside error badge
                    let stem = sd_round_box(u, v, bx, by - 0.045, 0.033, 0.075, 0.015);
                    let dot = sd_circle(u, v, bx, by + 0.105, 0.036);
                    let excl_cov = cov(stem.min(dot), aa);
                    px_col = badge_fg.with_alpha(excl_cov).over(px_col);
                }
            }

            let idx = ((y * size + x) * 4) as usize;
            out[idx..idx + 4].copy_from_slice(&px_col.to_u8());
        }
    }
    out
}

/// Renders the branded application icon (`assets/focus.ico`) on a rounded slate tile
/// with a cobalt focus sweep so it reads crisply across light and dark surfaces.
#[allow(dead_code)] // used by `examples/make_icon.rs` and tests
pub fn render_app_icon_rgba(size: u32) -> Vec<u8> {
    let s = size.max(1) as f32;
    let aa = (1.1 / s).max(0.008);
    let mut out = vec![0u8; (size * size * 4) as usize];

    let tile_bg = Rgba::from_hex(0x1B2230, 1.0);
    let tile_border = Rgba::from_hex(0x2E3A52, 1.0);
    let track = Rgba::from_hex(0xFFFFFF, 0.22);
    let sweep = Rgba::from_hex(0x5B8CFF, 1.0);
    let hands = Rgba::from_hex(0xFFFFFF, 1.0);

    for y in 0..size {
        for x in 0..size {
            let u = (x as f32 + 0.5) / s;
            let v = (y as f32 + 0.5) / s;
            let mut px_col = Rgba::TRANSPARENT;

            let d_tile = sd_round_box(u, v, 0.5, 0.5, 0.44, 0.44, 0.14);
            px_col = tile_bg.with_alpha(cov(d_tile, aa)).over(px_col);

            let d_border = d_tile.abs() - 0.012;
            px_col = tile_border.with_alpha(cov(d_border, aa)).over(px_col);

            let d_track = sd_ring(u, v, 0.5, 0.5, 0.27, 0.042);
            px_col = track.with_alpha(cov(d_track, aa)).over(px_col);

            let d_arc = clockwise_arc_dist(u, v, 0.5, 0.5, 0.27, 0.044, 0.72);
            px_col = sweep.with_alpha(cov(d_arc, aa)).over(px_col);

            let d_h1 = sd_segment(u, v, 0.5, 0.5, 0.5, 0.34, 0.030);
            let d_h2 = sd_segment(u, v, 0.5, 0.5, 0.61, 0.55, 0.026);
            let d_c = sd_circle(u, v, 0.5, 0.5, 0.048);
            px_col = hands.with_alpha(cov(d_h1.min(d_h2), aa)).over(px_col);
            px_col = sweep.with_alpha(cov(d_c, aa)).over(px_col);

            let idx = ((y * size + x) * 4) as usize;
            out[idx..idx + 4].copy_from_slice(&px_col.to_u8());
        }
    }
    out
}

/// Creates a Win32 `HICON` from a square RGBA pixel buffer via `user32` directly
/// (avoiding `CreateDIBSection` / `gdi32` / `mscms` initialization on tray startup).
/// Caller owns the returned handle and must destroy it with `DestroyIcon`.
pub fn hicon_from_rgba(size: u32, rgba: &[u8]) -> Option<HICON> {
    if size == 0 || rgba.len() != (size * size * 4) as usize {
        return None;
    }

    let dib = encode_ico_dib_frame(size, rgba);
    // SAFETY: `dib` is a complete 32-bit BGRA `BITMAPINFOHEADER` + XOR/AND icon image for `size x size`.
    unsafe {
        CreateIconFromResourceEx(
            &dib,
            true,
            0x0003_0000,
            size as i32,
            size as i32,
            IMAGE_FLAGS(0),
        )
        .ok()
    }
}

pub fn create_tray_hicon(size: u32, state: IconState, theme: TaskbarTheme) -> Option<HICON> {
    let rgba = render_tray_rgba(size, state, theme);
    hicon_from_rgba(size, &rgba)
}

pub fn destroy_hicon(icon: HICON) {
    if !icon.is_invalid() {
        // SAFETY: `icon` is an owned Win32 icon handle created by `CreateIconFromResourceEx`.
        unsafe {
            let _ = DestroyIcon(icon);
        }
    }
}

/// Encodes square RGBA frames into a standard multi-resolution `.ico` byte stream
/// using uncompressed 32-bit BGRA DIB + 1-bpp AND masks (accepted by Windows `rc.exe` and Explorer).
#[allow(dead_code)] // used by `examples/make_icon.rs` and tests
pub fn encode_ico(sizes: &[u32]) -> Vec<u8> {
    let mut frames: Vec<(u32, Vec<u8>)> = Vec::with_capacity(sizes.len());
    for &size in sizes {
        let rgba = render_app_icon_rgba(size);
        let dib = encode_ico_dib_frame(size, &rgba);
        frames.push((size, dib));
    }

    let count = frames.len() as u16;
    let mut buf = Vec::new();
    // ICONDIR
    buf.extend_from_slice(&0u16.to_le_bytes()); // reserved
    buf.extend_from_slice(&1u16.to_le_bytes()); // type = 1 (ICO)
    buf.extend_from_slice(&count.to_le_bytes());

    let mut offset = 6 + (frames.len() * 16) as u32;
    for (size, data) in &frames {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        buf.push(dim); // bWidth
        buf.push(dim); // bHeight
        buf.push(0); // bColorCount
        buf.push(0); // bReserved
        buf.extend_from_slice(&1u16.to_le_bytes()); // wPlanes
        buf.extend_from_slice(&32u16.to_le_bytes()); // wBitCount
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }

    for (_, data) in frames {
        buf.extend_from_slice(&data);
    }
    buf
}

fn encode_ico_dib_frame(size: u32, rgba: &[u8]) -> Vec<u8> {
    let and_stride = size.div_ceil(32) * 4;
    let color_bytes = size * size * 4;
    let mask_bytes = and_stride * size;
    let mut out = Vec::with_capacity(40 + (color_bytes + mask_bytes) as usize);

    // BITMAPINFOHEADER (bottom-up BGRA, biHeight = 2 * size includes XOR + AND masks)
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(color_bytes + mask_bytes).to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    // Color XOR plane (bottom-up rows, BGRA)
    for y in (0..size).rev() {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            out.push(rgba[idx + 2]);
            out.push(rgba[idx + 1]);
            out.push(rgba[idx]);
            out.push(rgba[idx + 3]);
        }
    }

    // 1-bpp AND mask (bottom-up rows, bit=1 where transparent)
    let row_bytes = and_stride as usize;
    for y in (0..size).rev() {
        let mut row = vec![0u8; row_bytes];
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            if rgba[idx + 3] < 128 {
                let byte_i = (x / 8) as usize;
                let bit = 7 - (x % 8);
                row[byte_i] |= 1 << bit;
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_states_and_themes_produce_distinct_valid_pixels() {
        let sizes = [16, 20, 24, 32, 40, 48];
        for &size in &sizes {
            let stopped = render_tray_rgba(size, IconState::Stopped, TaskbarTheme::Dark);
            let running = render_tray_rgba(
                size,
                IconState::Running { progress_steps: 25 },
                TaskbarTheme::Dark,
            );
            let paused = render_tray_rgba(size, IconState::Paused, TaskbarTheme::Dark);
            let err = render_tray_rgba(size, IconState::Error, TaskbarTheme::Dark);
            let light = render_tray_rgba(size, IconState::Stopped, TaskbarTheme::Light);

            assert_eq!(stopped.len(), (size * size * 4) as usize);
            assert_ne!(stopped, running);
            assert_ne!(stopped, paused);
            assert_ne!(stopped, err);
            assert_ne!(paused, err);
            assert_ne!(stopped, light);

            // Top-left corner pixel must remain transparent.
            assert_eq!(stopped[3], 0);
            // Non-empty opaque pixels must exist.
            assert!(stopped.as_chunks::<4>().0.iter().any(|p| p[3] > 200));

            let icon = create_tray_hicon(
                size,
                IconState::Running { progress_steps: 25 },
                TaskbarTheme::Dark,
            )
            .expect("CreateIconIndirect should succeed");
            destroy_hicon(icon);
        }
    }

    #[test]
    fn running_progress_changes_icon_over_hour() {
        let start = render_tray_rgba(
            24,
            IconState::Running { progress_steps: 5 },
            TaskbarTheme::Dark,
        );
        let mid = render_tray_rgba(
            24,
            IconState::Running { progress_steps: 35 },
            TaskbarTheme::Dark,
        );
        assert_ne!(start, mid);
    }

    #[test]
    fn ico_encoder_writes_expected_directory_entries() {
        let ico = encode_ico(&[16, 20, 24, 32, 40, 48, 256]);
        assert_eq!(&ico[0..6], &[0, 0, 1, 0, 7, 0]);
        // Last directory entry is 256x256, encoded as width=0, height=0.
        let last_entry = &ico[6 + 6 * 16..6 + 7 * 16];
        assert_eq!(last_entry[0], 0);
        assert_eq!(last_entry[1], 0);
    }
}
