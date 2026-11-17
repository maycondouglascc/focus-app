//! DirectWrite text: one shared factory per process and a small cache of text
//! formats by scaled size and style. Measurement works without any window or
//! graphics device, so layout code and tests can use it directly.

use std::cell::{Cell, RefCell};

use windows::core::{w, Result};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_MEDIUM,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextFont {
    Ui,
    #[allow(dead_code)] // used by widgets using Segoe Fluent Icons
    Icons,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Weight {
    Regular,
    Medium,
    SemiBold,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wrap {
    NoWrap,
    Word,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FormatKey {
    scaled_tenths: u32,
    font: TextFont,
    weight: Weight,
    align: Align,
    wrap: Wrap,
}

pub struct TextSystem {
    factory: IDWriteFactory,
    text_scale: Cell<f32>,
    formats: RefCell<Vec<(FormatKey, IDWriteTextFormat)>>,
}

impl TextSystem {
    pub fn new() -> Result<Self> {
        // SAFETY: plain factory creation.
        let factory = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)? };
        Ok(Self {
            factory,
            text_scale: Cell::new(1.0),
            formats: RefCell::new(Vec::new()),
        })
    }

    /// Sets the OS text scale multiplier (`UISettings.TextScaleFactor`).
    /// Clears cached formats if the scale changed. Returns `true` if changed.
    pub fn set_text_scale(&self, scale: f32) -> bool {
        let clamped = if scale.is_finite() && scale > 0.0 {
            scale.clamp(0.5, 4.0)
        } else {
            1.0
        };
        if (self.text_scale.get() - clamped).abs() < 0.001 {
            return false;
        }
        self.text_scale.set(clamped);
        self.formats.borrow_mut().clear();
        true
    }

    pub fn text_scale(&self) -> f32 {
        self.text_scale.get()
    }

    pub fn format_styled(
        &self,
        font: TextFont,
        size: f32,
        weight: Weight,
        align: Align,
        wrap: Wrap,
    ) -> Result<IDWriteTextFormat> {
        let scaled = (size * self.text_scale.get()).max(1.0);
        let key = FormatKey {
            scaled_tenths: (scaled * 10.0).round() as u32,
            font,
            weight,
            align,
            wrap,
        };
        if let Some((_, f)) = self.formats.borrow().iter().find(|(k, _)| *k == key) {
            return Ok(f.clone());
        }
        let family = match font {
            TextFont::Ui => w!("Segoe UI Variable Text"),
            TextFont::Icons => w!("Segoe Fluent Icons"),
        };
        // SAFETY: all arguments are valid for the call; the format is owned by us.
        let f = unsafe {
            let f = self.factory.CreateTextFormat(
                family,
                None,
                match weight {
                    Weight::Regular => DWRITE_FONT_WEIGHT_NORMAL,
                    Weight::Medium => DWRITE_FONT_WEIGHT_MEDIUM,
                    Weight::SemiBold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
                },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                scaled,
                w!("en-us"),
            )?;
            f.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            })?;
            f.SetParagraphAlignment(match wrap {
                Wrap::NoWrap => DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                Wrap::Word => DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            })?;
            f.SetWordWrapping(match wrap {
                Wrap::NoWrap => DWRITE_WORD_WRAPPING_NO_WRAP,
                Wrap::Word => DWRITE_WORD_WRAPPING_WRAP,
            })?;
            f
        };
        self.formats.borrow_mut().push((key, f.clone()));
        Ok(f)
    }

    /// Width and height of `text` on one line, in DIPs.
    #[allow(dead_code)]
    pub fn measure(&self, text: &str, size: f32, weight: Weight) -> Result<(f32, f32)> {
        self.measure_styled(
            TextFont::Ui,
            text,
            size,
            weight,
            Align::Left,
            Wrap::NoWrap,
            f32::MAX / 4.0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn measure_styled(
        &self,
        font: TextFont,
        text: &str,
        size: f32,
        weight: Weight,
        align: Align,
        wrap: Wrap,
        max_width: f32,
    ) -> Result<(f32, f32)> {
        let format = self.format_styled(font, size, weight, align, wrap)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: `wide` outlives the layout calls; metrics is a valid out value.
        unsafe {
            let layout = self.factory.CreateTextLayout(
                &wide,
                &format,
                max_width.max(1.0),
                f32::MAX / 4.0,
            )?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Ok((m.widthIncludingTrailingWhitespace, m.height))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longer_text_is_wider() {
        let t = TextSystem::new().unwrap();
        let (a, _) = t.measure("Focus", 12.0, Weight::Regular).unwrap();
        let (b, _) = t.measure("Focus timer", 12.0, Weight::Regular).unwrap();
        assert!(a > 10.0 && b > a);
    }

    #[test]
    fn bigger_text_is_taller_and_formats_are_cached() {
        let t = TextSystem::new().unwrap();
        let (_, h12) = t.measure("Focus", 12.0, Weight::Regular).unwrap();
        let (_, h24) = t.measure("Focus", 24.0, Weight::Regular).unwrap();
        assert!(h24 > h12 * 1.5);
        t.measure("again", 12.0, Weight::Regular).unwrap();
        assert_eq!(t.formats.borrow().len(), 2);
    }

    #[test]
    fn empty_text_measures_zero_width() {
        let t = TextSystem::new().unwrap();
        let (w, _) = t.measure("", 12.0, Weight::Regular).unwrap();
        assert_eq!(w, 0.0);
    }

    #[test]
    fn text_scale_factor_scales_metrics_and_resets_cache() {
        let t = TextSystem::new().unwrap();
        let (w1, h1) = t.measure("Focus timer", 14.0, Weight::Regular).unwrap();
        assert!(!t.set_text_scale(1.0));
        assert!(t.set_text_scale(1.5));
        assert_eq!(t.formats.borrow().len(), 0);
        let (w2, h2) = t.measure("Focus timer", 14.0, Weight::Regular).unwrap();
        assert!(w2 > w1 * 1.35);
        assert!(h2 > h1 * 1.35);
    }
}
