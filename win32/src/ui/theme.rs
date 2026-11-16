//! Color tokens. Widgets never use literal colors, so a dark theme can be added
//! later by swapping this table (v1 is light only).

#[derive(Clone, Copy)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

pub const fn rgb(hex: u32) -> Color {
    Color(
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
        1.0,
    )
}

pub const SURFACE: Color = rgb(0xFFFFFF);
pub const TITLE_BAR: Color = rgb(0xF3F3F3);
pub const TEXT: Color = rgb(0x1A1A1A);
pub const BUTTON_HOVER: Color = rgb(0xE5E5E5);
pub const CLOSE_HOVER: Color = rgb(0xC42B1C);
pub const ON_CLOSE_HOVER: Color = rgb(0xFFFFFF);
