//! The print list as a spreadsheet.
//!
//! UTF-8 with a byte order mark, which Excel needs to read accented names,
//! and CRLF line endings throughout. Cells that a spreadsheet would run as a
//! formula get a leading `'`, since file and pack names come from downloads.

use std::path::Path;

use crate::print_list::{PrintRow, print_list};
use crate::{ExportError, ExportInput, map_layout::format_mm};

/// How the `file` column names each STL.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PathMode {
    /// `folder/file.stl`: enough to find it, and keeps user names out of
    /// files that get shared.
    #[default]
    Relative,
    /// The absolute path.
    Full,
}

pub const HEADER: [&str; 9] =
    ["name", "folder", "file", "count", "owned", "to_print", "size_cells", "height_mm", "volume_cm3"];

/// Encodes print list rows as CSV.
pub fn write_csv(rows: &[PrintRow], mode: PathMode) -> Result<Vec<u8>, ExportError> {
    let fail = |e: ::csv::Error| ExportError::Encode { format: "CSV file", detail: e.to_string() };
    let mut out = ::csv::WriterBuilder::new().terminator(::csv::Terminator::CRLF).from_writer(Vec::new());
    out.write_record(HEADER).map_err(fail)?;
    for r in rows {
        let file = match mode {
            PathMode::Relative if r.key.folder_name().is_empty() => r.key.file_name().to_string(),
            PathMode::Relative => format!("{}/{}", r.key.folder_name(), r.key.file_name()),
            PathMode::Full => r.key.as_str().to_string(),
        };
        let record = [
            r.name.clone(),
            r.folder.clone(),
            file,
            r.count.to_string(),
            r.owned.map_or(String::new(), |o| o.to_string()),
            r.to_print().to_string(),
            format!("{}x{}", r.size.w, r.size.h),
            format_mm(r.size.height_mm()),
            r.volume_mm3.map_or(String::new(), |v| format!("{:.1}", v / 1000.0)),
        ];
        out.write_record(record.iter().map(|c| guard(c))).map_err(fail)?;
    }
    let body = out.into_inner().map_err(|e| ExportError::Encode { format: "CSV file", detail: e.to_string() })?;
    let mut bytes = Vec::with_capacity(body.len() + 3);
    bytes.extend_from_slice(b"\xEF\xBB\xBF");
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

/// Writes the print list of `input` to `path`.
pub fn export_csv(path: &Path, input: &ExportInput, mode: PathMode) -> Result<(), ExportError> {
    input.check_not_empty()?;
    let bytes = write_csv(&print_list(input), mode)?;
    crate::write_file(path, &bytes)
}

/// Stops a spreadsheet from running a cell as a formula.
fn guard(cell: &str) -> String {
    if cell.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{cell}") } else { cell.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DefaultInfo;
    use crate::testutil::{state, tile};

    fn rows(paths: &[&str]) -> Vec<PrintRow> {
        let tiles = paths.iter().enumerate().map(|(i, p)| tile(i as u64 + 1, p, 2, 1, i as f32 * 2.0, 0.0)).collect();
        let s = state(40, 40, tiles);
        print_list(&ExportInput { title: "Test".into(), state: &s, info: &DefaultInfo, level: None })
    }

    #[test]
    fn exact_bytes() {
        let rows = rows(&[
            "C:/Users/me/Tiles/OpenForge/floor.stl",
            "C:/Users/me/Tiles/OpenForge/floor.stl",
            "C:/Users/me/Tiles/Packs, vol 2/wall \"tall\".stl",
        ]);
        let text = String::from_utf8(write_csv(&rows, PathMode::Relative).expect("csv")).expect("utf-8");
        assert_eq!(
            text,
            "\u{feff}name,folder,file,count,owned,to_print,size_cells,height_mm,volume_cm3\r\n\
             floor,OpenForge,OpenForge/floor.stl,2,,2,2x1,6,\r\n\
             \"wall \"\"tall\"\"\",\"Packs, vol 2\",\"Packs, vol 2/wall \"\"tall\"\".stl\",1,,1,2x1,6,\r\n"
        );
    }

    #[test]
    fn formulas_are_defused() {
        let rows = rows(&["x/=HYPERLINK(1).stl", "x/+1.stl", "x/-1.stl", "x/@a.stl", "x/plain.stl"]);
        let text = String::from_utf8(write_csv(&rows, PathMode::Relative).expect("csv")).expect("utf-8");
        for name in ["'=HYPERLINK(1)", "'+1", "'-1", "'@a"] {
            assert!(text.lines().any(|l| l.starts_with(name)), "{name} guarded in {text}");
        }
        assert!(text.contains("\r\nplain,x,x/plain.stl,"));
    }

    #[test]
    fn full_paths_on_request() {
        let rows = rows(&["C:/Users/me/Tiles/floor.stl"]);
        let text = String::from_utf8(write_csv(&rows, PathMode::Full).expect("csv")).expect("utf-8");
        assert!(text.contains(",C:/Users/me/Tiles/floor.stl,"));
        let text = String::from_utf8(write_csv(&rows, PathMode::Relative).expect("csv")).expect("utf-8");
        assert!(!text.contains("Users"));
    }
}
