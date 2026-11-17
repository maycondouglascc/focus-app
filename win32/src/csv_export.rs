//! CSV export for Reports:
//! - Excel locale dialect: UTF-8 BOM (`\u{FEFF}`), `LOCALE_SLIST` delimiter (`;` in pt-BR),
//!   `LOCALE_SDECIMAL` decimal mark (`,` in pt-BR), CRLF line endings.
//! - Standard CSV dialect: UTF-8, `,` delimiter, `.` decimal mark, CRLF line endings.
//! - File picker via Win32 `IFileSaveDialog`.

use std::collections::HashMap;
use std::path::Path;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileSaveDialog, IFileSaveDialog, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, SIGDN_FILESYSPATH,
};

use crate::db::{Project, TimeEntry};
use crate::format::{csv_date, time as fmt_time};
use crate::locale::SystemLocale;
use crate::stats::earned;
use crate::time::HOUR;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsvDialect {
    ExcelLocale { list_sep: char, decimal_sep: char },
    Standard,
}

impl CsvDialect {
    pub fn from_locale(locale: &SystemLocale, standard: bool) -> Self {
        if standard {
            Self::Standard
        } else {
            Self::ExcelLocale {
                list_sep: locale.list_sep,
                decimal_sep: locale.decimal_sep,
            }
        }
    }
}

fn quote_csv(field: &str) -> String {
    let escaped = field.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

fn format_decimal(v: f64, digits: usize, decimal_sep: char) -> String {
    let s = format!("{v:.digits$}");
    if decimal_sep == '.' {
        s
    } else {
        s.replace('.', &decimal_sep.to_string())
    }
}

fn format_rate(rate: f64, decimal_sep: char) -> String {
    if (rate.fract()).abs() < 1e-6 {
        format!("{}", rate.round() as i64)
    } else {
        format_decimal(rate, 2, decimal_sep)
    }
}

/// Serializes `entries` to CSV bytes (`Vec<u8>`) using `dialect`.
pub fn format_entries_csv(
    entries: &[TimeEntry],
    by_id: &HashMap<&str, &Project>,
    now_ms: i64,
    is_24h: bool,
    dialect: CsvDialect,
) -> Vec<u8> {
    let (sep, dec, bom) = match dialect {
        CsvDialect::ExcelLocale {
            list_sep,
            decimal_sep,
        } => (list_sep, decimal_sep, true),
        CsvDialect::Standard => (',', '.', false),
    };
    let sep_str = sep.to_string();

    let mut out = String::new();
    if bom {
        out.push('\u{FEFF}');
    }
    let headers = [
        "Date",
        "Start",
        "End",
        "Hours",
        "Project",
        "Client",
        "Description",
        "Billable",
        "Rate",
        "Currency",
        "Amount",
    ];
    out.push_str(
        &headers
            .iter()
            .map(|h| quote_csv(h))
            .collect::<Vec<_>>()
            .join(&sep_str),
    );
    out.push_str("\r\n");

    for e in entries {
        let p = e
            .project_id
            .as_deref()
            .and_then(|id| by_id.get(id).copied());
        let end = e.end.unwrap_or(now_ms);
        let hours = ((end - e.start).max(0) as f64) / (HOUR as f64);
        let amount = earned(e, p, i64::MIN, i64::MAX, now_ms);
        let cols = [
            csv_date(e.start),
            fmt_time(e.start, is_24h),
            fmt_time(end, is_24h),
            format_decimal(hours, 2, dec),
            p.map(|x| x.name.as_str()).unwrap_or("").to_string(),
            p.map(|x| x.client.as_str()).unwrap_or("").to_string(),
            e.description.clone(),
            if e.billable { "yes" } else { "no" }.to_string(),
            p.map(|x| format_rate(x.rate, dec)).unwrap_or_default(),
            p.map(|x| x.currency.as_str()).unwrap_or("").to_string(),
            format_decimal(amount, 2, dec),
        ];
        out.push_str(
            &cols
                .iter()
                .map(|c| quote_csv(c))
                .collect::<Vec<_>>()
                .join(&sep_str),
        );
        out.push_str("\r\n");
    }

    out.into_bytes()
}

/// Converts the Win32 `IFileSaveDialog` outcome: user cancellation maps to `Ok(None)`,
/// whereas COM/dialog errors propagate as `Err(message)` so callers never silence failures.
pub fn classify_save_dialog_result(
    res: windows::core::Result<std::path::PathBuf>,
) -> Result<Option<std::path::PathBuf>, String> {
    let cancel_hr = ERROR_CANCELLED.to_hresult();
    match res {
        Ok(p) => Ok(Some(p)),
        Err(e) if e.code() == cancel_hr => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Opens the native Windows Save File dialog for exporting CSV.
pub fn pick_csv_save_path(
    owner: HWND,
    suggested_name: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    // SAFETY: STA COM initialization paired with `CoUninitialize` when `S_OK`/`S_FALSE` succeeded.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let need_uninit = hr.is_ok();
        let res = (|| -> windows::core::Result<std::path::PathBuf> {
            let dlg: IFileSaveDialog =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
            let filter = [COMDLG_FILTERSPEC {
                pszName: w!("CSV files (*.csv)"),
                pszSpec: w!("*.csv"),
            }];
            dlg.SetFileTypes(&filter)?;
            dlg.SetDefaultExtension(w!("csv"))?;
            let fname = HSTRING::from(suggested_name);
            dlg.SetFileName(PCWSTR(fname.as_ptr()))?;
            let opts = dlg.GetOptions()?;
            dlg.SetOptions(opts | FOS_OVERWRITEPROMPT | FOS_PATHMUSTEXIST)?;
            dlg.Show(Some(owner))?;
            let item = dlg.GetResult()?;
            let ptr = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path_str = ptr.to_string()?;
            CoTaskMemFree(Some(ptr.0 as *const _));
            Ok(std::path::PathBuf::from(path_str))
        })();
        if need_uninit {
            CoUninitialize();
        }
        classify_save_dialog_result(res)
    }
}

pub fn write_csv_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::local_day_at;

    fn sample_data() -> (Project, Vec<TimeEntry>) {
        let p = Project {
            id: "p1".into(),
            name: "São Paulo, Design".into(),
            client: "Café \"Norte\"".into(),
            color: "#3b7ddd".into(),
            rate: 150.0,
            currency: "BRL".into(),
            billable_default: true,
            archived: false,
            created_at: 1,
        };
        let start = local_day_at(2026, 11, 17, 9 * 60);
        let end = local_day_at(2026, 11, 17, 10 * 60 + 30);
        let entries = vec![TimeEntry {
            id: "e1".into(),
            project_id: Some("p1".into()),
            description: "Revisão de interface".into(),
            start,
            end: Some(end),
            billable: true,
            source: "calendar".into(),
        }];
        (p, entries)
    }

    #[test]
    fn excel_pt_br_csv_writes_utf8_bom_semicolon_and_decimal_comma() {
        let (p, entries) = sample_data();
        let mut by_id = HashMap::new();
        by_id.insert(p.id.as_str(), &p);
        let loc = SystemLocale::pt_br();
        let bytes = format_entries_csv(
            &entries,
            &by_id,
            0,
            true,
            CsvDialect::from_locale(&loc, false),
        );
        // UTF-8 BOM for Excel pt-BR
        assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
        let text = std::str::from_utf8(&bytes[3..]).unwrap();
        let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\";\""));
        assert!(lines[1].contains("\"1,50\""));
        assert!(lines[1].contains("\"225,00\""));
        assert!(lines[1].contains("\"Café \"\"Norte\"\"\""));
        assert!(lines[1].contains("\"Revisão de interface\""));
    }

    #[test]
    fn standard_csv_uses_comma_dot_and_no_bom() {
        let (p, entries) = sample_data();
        let mut by_id = HashMap::new();
        by_id.insert(p.id.as_str(), &p);
        let bytes = format_entries_csv(&entries, &by_id, 0, true, CsvDialect::Standard);
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
        let text = std::str::from_utf8(&bytes).unwrap();
        let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\",\""));
        assert!(lines[1].contains("\"1.50\""));
        assert!(lines[1].contains("\"225.00\""));
    }

    #[test]
    fn classify_save_dialog_result_distinguishes_cancel_from_failure() {
        let canceled = windows::core::Error::from(ERROR_CANCELLED.to_hresult());
        assert_eq!(classify_save_dialog_result(Err(canceled)), Ok(None));

        let failed = windows::core::Error::from(windows::Win32::Foundation::E_FAIL);
        assert!(classify_save_dialog_result(Err(failed)).is_err());
    }
}
