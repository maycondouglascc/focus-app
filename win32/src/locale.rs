//! Windows user locale queries (`Win32::Globalization::GetLocaleInfoEx`) for
//! default currency (`LOCALE_SINTLSYMBOL`), week start (`LOCALE_IFIRSTDAYOFWEEK`),
//! 24h/12h clock (`LOCALE_ITIME`), and CSV/number separators (`LOCALE_SLIST`,
//! `LOCALE_SDECIMAL`, `LOCALE_STHOUSAND`).
#![allow(dead_code)] // wired across F3 screens and CSV writer

use windows::core::PCWSTR;
use windows::Win32::Globalization::{
    GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK, LOCALE_ITIME, LOCALE_SDECIMAL, LOCALE_SINTLSYMBOL,
    LOCALE_SLIST, LOCALE_STHOUSAND,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemLocale {
    /// ISO 4217 currency code (e.g. `"BRL"`, `"USD"`, `"PLN"`).
    pub currency: String,
    /// First day of week as `0 = Mon .. 6 = Sun` (`LOCALE_IFIRSTDAYOFWEEK`).
    pub first_day_mon0: u8,
    /// True for 24-hour clock, false for 12-hour clock (`LOCALE_ITIME`).
    pub is_24h: bool,
    /// List separator for Excel CSV (`';'` in pt-BR, `','` in en-US).
    pub list_sep: char,
    /// Decimal separator (`','` in pt-BR, `'.'` in en-US).
    pub decimal_sep: char,
    /// Thousands grouping separator (`'.'` in pt-BR, `','` in en-US).
    pub thousand_sep: char,
}

impl Default for SystemLocale {
    fn default() -> Self {
        Self::en_us()
    }
}

impl SystemLocale {
    pub fn en_us() -> Self {
        Self {
            currency: "USD".into(),
            first_day_mon0: 6,
            is_24h: false,
            list_sep: ',',
            decimal_sep: '.',
            thousand_sep: ',',
        }
    }

    #[cfg(test)]
    pub fn pt_br() -> Self {
        Self {
            currency: "BRL".into(),
            first_day_mon0: 6,
            is_24h: true,
            list_sep: ';',
            decimal_sep: ',',
            thousand_sep: '.',
        }
    }

    /// Queries the current Windows user locale via `GetLocaleInfoEx`.
    pub fn query() -> Self {
        let currency = normalize_currency(query_lctype(LOCALE_SINTLSYMBOL).as_deref());
        let first_day_mon0 = query_lctype(LOCALE_IFIRSTDAYOFWEEK)
            .and_then(|s| s.trim().parse::<u8>().ok())
            .filter(|&d| d <= 6)
            .unwrap_or(0);
        let is_24h = query_lctype(LOCALE_ITIME)
            .map(|s| s.trim() == "1")
            .unwrap_or(true);
        let decimal_sep = query_first_char(LOCALE_SDECIMAL).unwrap_or('.');
        let mut list_sep = query_first_char(LOCALE_SLIST).unwrap_or(',');
        if list_sep == decimal_sep {
            list_sep = ';';
        }
        let thousand_sep = query_first_char(LOCALE_STHOUSAND).unwrap_or(',');
        Self {
            currency,
            first_day_mon0,
            is_24h,
            list_sep,
            decimal_sep,
            thousand_sep,
        }
    }
}

pub fn normalize_currency(raw: Option<&str>) -> String {
    if let Some(s) = raw {
        let trimmed = s.trim();
        if trimmed.len() == 3 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
            return trimmed.to_ascii_uppercase();
        }
    }
    "USD".into()
}

fn query_first_char(lctype: u32) -> Option<char> {
    query_lctype(lctype).and_then(|s| s.chars().next())
}

fn query_lctype(lctype: u32) -> Option<String> {
    let mut buf = [0u16; 64];
    // SAFETY: `PCWSTR::null()` requests `LOCALE_NAME_USER_DEFAULT` and `buf` is stack-allocated.
    let len = unsafe { GetLocaleInfoEx(PCWSTR::null(), lctype, Some(&mut buf)) };
    if len <= 1 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..(len as usize - 1)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_locale_query_returns_valid_fields() {
        let loc = SystemLocale::query();
        assert_eq!(loc.currency.len(), 3);
        assert!(loc.currency.chars().all(|c| c.is_ascii_uppercase()));
        assert!(loc.first_day_mon0 <= 6);
        assert_ne!(loc.list_sep, loc.decimal_sep);
    }

    #[test]
    fn currency_normalization_validates_iso_4217_codes() {
        assert_eq!(normalize_currency(Some("brl")), "BRL");
        assert_eq!(normalize_currency(Some(" PLN ")), "PLN");
        assert_eq!(normalize_currency(Some("")), "USD");
        assert_eq!(normalize_currency(Some("R$")), "USD");
        assert_eq!(normalize_currency(None), "USD");
    }
}
