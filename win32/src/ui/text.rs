//! DirectWrite text: one shared factory per window and a small cache of text
//! formats by size. Measurement works without any window or graphics device,
//! so layout code and tests can use it directly.

use std::cell::RefCell;

use windows::core::{w, Result};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};

// Regular and `measure` are used by the layout engine, which comes next.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    SemiBold,
}

pub struct TextSystem {
    factory: IDWriteFactory,
    /// (size in tenths of a DIP, weight) -> format
    formats: RefCell<Vec<(u32, Weight, IDWriteTextFormat)>>,
}

impl TextSystem {
    pub fn new() -> Result<Self> {
        // SAFETY: plain factory creation.
        let factory = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)? };
        Ok(Self {
            factory,
            formats: RefCell::new(Vec::new()),
        })
    }

    /// Single-line, vertically centered format. Sizes are DIPs.
    pub fn format(&self, size: f32, weight: Weight) -> Result<IDWriteTextFormat> {
        let key = (size * 10.0).round() as u32;
        if let Some((_, _, f)) = self
            .formats
            .borrow()
            .iter()
            .find(|(k, w, _)| *k == key && *w == weight)
        {
            return Ok(f.clone());
        }
        // SAFETY: all arguments are valid for the call; the format is owned by us.
        let f = unsafe {
            let f = self.factory.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                match weight {
                    Weight::Regular => DWRITE_FONT_WEIGHT_NORMAL,
                    Weight::SemiBold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
                },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f
        };
        self.formats.borrow_mut().push((key, weight, f.clone()));
        Ok(f)
    }

    /// Width and height of `text` on one line, in DIPs.
    #[allow(dead_code)]
    pub fn measure(&self, text: &str, size: f32, weight: Weight) -> Result<(f32, f32)> {
        let format = self.format(size, weight)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: `wide` outlives the layout calls; metrics is a valid out value.
        unsafe {
            let layout =
                self.factory
                    .CreateTextLayout(&wide, &format, f32::MAX / 4.0, f32::MAX / 4.0)?;
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
}
