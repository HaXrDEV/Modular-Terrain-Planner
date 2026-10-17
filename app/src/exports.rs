//! Build plan exports: asks where to save, then hands the document to the
//! export crate with what the library knows about each tile.

use std::collections::HashMap;
use std::path::PathBuf;

use mtp_core::tile::TileKey;
use mtp_export::{ExportInput, Paper, PathMode, PdfOptions, TileMeta};

use crate::editor::Editor;
use crate::library::Library;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Pdf,
    Png,
    Csv,
}

/// Paper sizes, as stored in the settings.
pub const PAPERS: [(&str, Paper); 5] = [
    ("A4", Paper::A4),
    ("A3", Paper::A3),
    ("Letter", Paper::Letter),
    ("Legal", Paper::Legal),
    ("Tabloid", Paper::Tabloid),
];

pub fn paper(name: &str) -> Paper {
    PAPERS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map_or(Paper::A4, |(_, p)| *p)
}

/// What exports print for each tile: names, pack, colour, and the measured
/// volume for filament estimates.
fn tile_info(editor: &Editor, library: &Library) -> HashMap<TileKey, TileMeta> {
    editor
        .doc
        .tiles()
        .iter()
        .map(|t| {
            let meta = match library.tile(&t.key) {
                Some(lt) => TileMeta {
                    name: lt.name.clone(),
                    folder: library
                        .folder(lt.folder)
                        .map_or_else(|| t.key.folder_name().to_owned(), |f| f.name.clone()),
                    color: lt.color,
                    volume_mm3: Some(lt.volume_mm3),
                },
                None => TileMeta::from_key(&t.key),
            };
            (t.key.clone(), meta)
        })
        .collect()
}

/// Exports after asking for a file name. `Ok(None)` when the dialog was cancelled.
pub fn export(
    kind: ExportKind,
    editor: &Editor,
    library: &Library,
    paper_name: &str,
    full_paths: bool,
) -> Result<Option<PathBuf>, String> {
    let title = editor.title();
    let base = title.replace(' ', "_");
    let (filter, ext, name) = match kind {
        ExportKind::Pdf => ("PDF build plan", "pdf", format!("{base}_build_plan.pdf")),
        ExportKind::Png => ("PNG image", "png", format!("{base}_assembly_map.png")),
        ExportKind::Csv => ("CSV print list", "csv", format!("{base}_print_list.csv")),
    };
    let mut dialog = rfd::FileDialog::new().add_filter(filter, &[ext]).set_file_name(&name);
    if let Some(dir) = editor.path.as_ref().and_then(|p| p.parent()) {
        dialog = dialog.set_directory(dir);
    }
    let Some(mut path) = dialog.save_file() else { return Ok(None) };
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
        path.as_mut_os_string().push(format!(".{ext}"));
    }

    let info = tile_info(editor, library);
    let input = ExportInput { title, state: editor.doc.state(), info: &info, level: None };
    let result = match kind {
        ExportKind::Pdf => {
            mtp_export::export_pdf(&path, &input, &PdfOptions { paper: paper(paper_name), ..Default::default() })
        }
        ExportKind::Png => mtp_export::export_png(&path, &input),
        ExportKind::Csv => {
            mtp_export::export_csv(&path, &input, if full_paths { PathMode::Full } else { PathMode::Relative })
        }
    };
    result.map(|()| Some(path)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_names_round_trip() {
        for (name, p) in PAPERS {
            assert_eq!(paper(name), p);
        }
        assert_eq!(paper("unknown"), Paper::A4);
    }
}
