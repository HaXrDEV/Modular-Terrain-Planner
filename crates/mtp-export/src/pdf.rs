//! The build plan as a printable PDF: a map of each level, larger sections of
//! it when the whole map would print too small to read, the print list and
//! the placements table. Everything is vector, text is real text in the
//! bundled font.

use std::ops::Range;
use std::path::Path;

use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke, StrokeDash};
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla::{Document, SerializeSettings};

use crate::draw::{self, Canvas, INK, MUTED, MapView, RULE, Rgb, WHITE};
use crate::map_layout::{CellBounds, MapLayout, format_mm, map_layout};
use crate::placements::placements;
use crate::print_list::{PrintRow, PrintTotals, print_list};
use crate::text::{FONT_BYTES, Typeface, typeface};
use crate::{ExportError, ExportInput};

/// Paper sizes people print build plans on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paper {
    #[default]
    A4,
    A3,
    Letter,
    Legal,
    Tabloid,
}

impl Paper {
    /// Width and height in millimetres, portrait.
    pub fn size_mm(self) -> (f32, f32) {
        match self {
            Self::A4 => (210.0, 297.0),
            Self::A3 => (297.0, 420.0),
            Self::Letter => (215.9, 279.4),
            Self::Legal => (215.9, 355.6),
            Self::Tabloid => (279.4, 431.8),
        }
    }

    fn portrait(self) -> [f32; 2] {
        let (w, h) = self.size_mm();
        [mm(w), mm(h)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdfOptions {
    pub paper: Paper,
    /// Compress page contents. Only tests that read the raw file turn it off.
    pub compress: bool,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self { paper: Paper::A4, compress: true }
    }
}

fn mm(v: f32) -> f32 {
    v * 72.0 / 25.4
}

const MARGIN: f32 = 34.0;
const TITLE: f32 = 15.0;
const HEADING: f32 = 10.0;
const HEADER_H: f32 = MARGIN + TITLE + 6.0 + HEADING + 12.0;
const FOOTER_H: f32 = MARGIN + 6.0;
const FOOTER: f32 = 8.0;
const AXIS: f32 = 6.5;
const KEY: f32 = 7.5;
const KEY_H: f32 = KEY * 1.9 * 3.0 + 8.0;
const TABLE: f32 = 8.5;
const LINE: f32 = TABLE * 1.3;
const PAD: f32 = 3.0;
const MIN_TEXT: f32 = 3.5;

/// The overview prints a cell at least this big, or gets detail pages.
const READABLE_CELL: f32 = 4.0 * 72.0 / 25.4;
/// Cell size on detail pages.
const DETAIL_CELL: f32 = 7.0 * 72.0 / 25.4;
/// Overviews of small maps don't blow tiles up beyond this.
const MAX_CELL: f32 = 15.0 * 72.0 / 25.4;

/// Renders the build plan and writes it to `path`.
pub fn export_pdf(path: &Path, input: &ExportInput, options: &PdfOptions) -> Result<(), ExportError> {
    let bytes = render_pdf(input, options)?;
    crate::write_file(path, &bytes)
}

/// A part of a level's map printed on its own page.
#[derive(Clone, Debug)]
struct Section {
    name: String,
    cells: CellBounds,
}

enum Content {
    Overview { layout: usize, view: MapView, sections: Vec<Section> },
    Detail { layout: usize, view: MapView },
    Table { table: usize, rows: Range<usize>, totals: bool },
}

struct PagePlan {
    size: [f32; 2],
    heading: String,
    content: Content,
}

/// Every page of the plan, worked out before drawing so each page knows
/// how many there are.
struct Plan {
    pages: Vec<PagePlan>,
    layouts: Vec<MapLayout>,
    tables: [Table; 2],
}

fn plan(input: &ExportInput, face: &Typeface, paper: Paper) -> Plan {
    let paper = paper.portrait();
    let layouts: Vec<MapLayout> = input.levels().into_iter().filter_map(|l| map_layout(input, Some(l))).collect();
    let mut pages = Vec::new();
    for (i, layout) in layouts.iter().enumerate() {
        plan_map_pages(&mut pages, face, layout, i, paper);
    }

    let rows = print_list(input);
    let totals = PrintTotals::of(&rows);
    let width = paper[0] - 2.0 * MARGIN;
    let tables = [print_table(face, &rows, &totals, width), placements_table(face, input, width)];
    for (i, table) in tables.iter().enumerate() {
        plan_table_pages(&mut pages, table, i, paper);
    }
    Plan { pages, layouts, tables }
}

/// Renders the build plan to PDF bytes.
pub fn render_pdf(input: &ExportInput, options: &PdfOptions) -> Result<Vec<u8>, ExportError> {
    input.check_not_empty()?;
    let face = typeface()?;
    let plan = plan(input, face, options.paper);

    let font = Font::new(FONT_BYTES.into(), 0).ok_or(ExportError::Font)?;
    let mut doc =
        Document::new_with(SerializeSettings { compress_content_streams: options.compress, ..Default::default() });
    doc.set_metadata(
        krilla::metadata::Metadata::new().title(input.title.clone()).creator("Modular Terrain Planner".to_string()),
    );
    for n in 0..plan.pages.len() {
        let size = plan.pages[n].size;
        let settings = PageSettings::from_wh(size[0], size[1])
            .ok_or(ExportError::Encode { format: "PDF", detail: "a page has no size".to_string() })?;
        let mut page = doc.start_page_with(settings);
        let mut surface = page.surface();
        draw_page(&mut PdfCanvas { surface: &mut surface, font: font.clone() }, face, &input.title, &plan, n);
        surface.finish();
        page.finish();
    }
    doc.finish().map_err(|e| ExportError::Encode { format: "PDF", detail: e.to_string() })
}

fn draw_page(c: &mut dyn Canvas, face: &Typeface, title: &str, plan: &Plan, n: usize) {
    let page = &plan.pages[n];
    draw_frame(c, face, title, &page.heading, page.size, n + 1, plan.pages.len());
    match &page.content {
        Content::Overview { layout, view, sections } => {
            draw::draw_map(c, face, &plan.layouts[*layout], view, true);
            for s in sections {
                draw_section_outline(c, face, view, s);
            }
            draw_map_key(c, face, view, page.size);
        }
        Content::Detail { layout, view } => {
            draw::draw_map(c, face, &plan.layouts[*layout], view, true);
            draw_map_key(c, face, view, page.size);
        }
        Content::Table { table, rows, totals } => {
            draw_table(c, face, &plan.tables[*table], rows.clone(), *totals);
        }
    }
}

fn level_name(layout: &MapLayout) -> String {
    layout.level.map_or("All levels".to_string(), |l| format!("Level {l}"))
}

/// Where a map goes on a page of `size`: origin and room, in points.
fn map_area(face: &Typeface, size: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    let left = MARGIN + MapView::axis_margin(face, AXIS);
    let top = HEADER_H + AXIS * 1.8;
    ([left, top], [size[0] - MARGIN - left, size[1] - top - FOOTER_H - KEY_H])
}

/// The overview, in the orientation that prints it largest, and detail
/// pages when it prints too small to read.
fn plan_map_pages(pages: &mut Vec<PagePlan>, face: &Typeface, layout: &MapLayout, index: usize, paper: [f32; 2]) {
    let b = layout.bounds;
    let (mw, mh) = (b.width() as f32, b.height() as f32);
    let fit = |size: [f32; 2]| {
        let (_, room) = map_area(face, size);
        (room[0] / mw).min(room[1] / mh)
    };
    let landscape = [paper[1], paper[0]];
    let size = if fit(landscape) > fit(paper) { landscape } else { paper };
    let cell = fit(size).min(MAX_CELL);
    let (origin, room) = map_area(face, size);
    let view = MapView {
        origin: [origin[0] + (room[0] - mw * cell) / 2.0, origin[1]],
        cell,
        window: b,
        min_text: MIN_TEXT,
        axis_text: AXIS,
    };

    let sections = if cell < READABLE_CELL { split_into_sections(face, b, paper) } else { (Vec::new(), paper) };
    let heading = if sections.0.is_empty() {
        format!("{} map", level_name(layout))
    } else {
        format!("{} map. Dashed outlines are the sections printed larger on the next pages.", level_name(layout))
    };
    pages.push(PagePlan {
        size,
        heading,
        content: Content::Overview { layout: index, view, sections: sections.0.clone() },
    });

    let (sections, size) = sections;
    for section in sections {
        let (origin, _) = map_area(face, size);
        let c = section.cells;
        let heading = format!(
            "{}, section {}: cells x {} to {}, y {} to {}",
            level_name(layout),
            section.name,
            c.min_x,
            c.max_x - 1,
            c.min_y,
            c.max_y - 1
        );
        let view = MapView { origin, cell: DETAIL_CELL, window: c, min_text: MIN_TEXT, axis_text: AXIS };
        pages.push(PagePlan { size, heading, content: Content::Detail { layout: index, view } });
    }
}

/// Splits a map into page-sized sections at [`DETAIL_CELL`], in whichever
/// orientation needs fewer pages. Sections are an even number of cells so
/// the 1" squares line up across pages. Named by row from the top (A, B, ...)
/// and column from the left (1, 2, ...).
fn split_into_sections(face: &Typeface, b: CellBounds, paper: [f32; 2]) -> (Vec<Section>, [f32; 2]) {
    let plan = |size: [f32; 2]| {
        let (_, room) = map_area(face, size);
        let fit = |r: f32| {
            let n = (r / DETAIL_CELL).floor() as i32;
            (n - n % 2).max(2)
        };
        let (cols, rows) = (fit(room[0]), fit(room[1]));
        let (nx, ny) = ((b.width() + cols - 1) / cols, (b.height() + rows - 1) / rows);
        // Even out the sections so the last one isn't a sliver
        let even = |len: i32, n: i32| {
            let s = (len + n - 1) / n;
            s + s % 2
        };
        (nx, ny, even(b.width(), nx), even(b.height(), ny))
    };
    let landscape = [paper[1], paper[0]];
    let (p, l) = (plan(paper), plan(landscape));
    let ((nx, ny, sw, sh), size) = if l.0 * l.1 < p.0 * p.1 { (l, landscape) } else { (p, paper) };

    let mut sections = Vec::new();
    for row in 0..ny {
        for col in 0..nx {
            let cells = CellBounds {
                min_x: b.min_x + col * sw,
                max_x: (b.min_x + (col + 1) * sw).min(b.max_x),
                min_y: (b.max_y - (row + 1) * sh).max(b.min_y),
                max_y: b.max_y - row * sh,
            };
            if cells.width() > 0 && cells.height() > 0 {
                sections.push(Section { name: format!("{}{}", row_letters(row), col + 1), cells });
            }
        }
    }
    (sections, size)
}

/// A, B, ..., Z, AA, AB, ...
fn row_letters(mut n: i32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (n % 26) as u8);
        n = n / 26 - 1;
        if n < 0 {
            break;
        }
    }
    s.reverse();
    String::from_utf8_lossy(&s).into_owned()
}

fn draw_section_outline(c: &mut dyn Canvas, face: &Typeface, view: &MapView, s: &Section) {
    let [x0, y0] = view.at(s.cells.min_x as f32, s.cells.max_y as f32);
    let (w, h) = (s.cells.width() as f32 * view.cell, s.cells.height() as f32 * view.cell);
    c.stroke_rect([x0, y0, w, h], INK, 0.8, true);
    let size = 8.0;
    let lw = face.width(&s.name, size) + 4.0;
    c.fill_rect(x0 + 1.5, y0 + 1.5, lw, size + 3.0, WHITE);
    c.text(face, x0 + 3.5, y0 + 1.5 + size, size, INK, &s.name);
}

fn draw_map_key(c: &mut dyn Canvas, face: &Typeface, view: &MapView, size: [f32; 2]) {
    let top = (view.origin[1] + view.height() + 10.0).min(size[1] - FOOTER_H - KEY_H + 8.0);
    draw::draw_key(c, face, MARGIN, top, size[0] - 2.0 * MARGIN, KEY);
}

/// Title, page heading and page number.
fn draw_frame(c: &mut dyn Canvas, face: &Typeface, title: &str, heading: &str, size: [f32; 2], n: usize, total: usize) {
    let width = size[0] - 2.0 * MARGIN;
    c.text(face, MARGIN, MARGIN + TITLE * 0.8, TITLE, INK, &face.fit(title, TITLE, width));
    c.text(face, MARGIN, MARGIN + TITLE + 6.0 + HEADING * 0.8, HEADING, MUTED, &face.fit(heading, HEADING, width));
    let base = size[1] - MARGIN + FOOTER;
    c.line([MARGIN, base - FOOTER - 4.0], [size[0] - MARGIN, base - FOOTER - 4.0], RULE, 0.5);
    c.text(face, MARGIN, base, FOOTER, MUTED, "Modular Terrain Planner");
    draw::text_right(c, face, size[0] - MARGIN, base, FOOTER, MUTED, &format!("Page {n} of {total}"));
}

struct Column {
    title: &'static str,
    /// Share of the table's width.
    width: f32,
    right: bool,
}

/// A table with every cell already wrapped to its column.
struct Table {
    columns: Vec<Column>,
    widths: Vec<f32>,
    /// Lines of each cell of each row.
    rows: Vec<Vec<Vec<String>>>,
    /// Shown above the table on every page.
    note: Vec<String>,
    /// Shown after the last row.
    totals: Vec<String>,
}

impl Table {
    fn new(face: &Typeface, columns: Vec<Column>, cells: Vec<Vec<String>>, width: f32) -> Self {
        let sum: f32 = columns.iter().map(|c| c.width).sum();
        let widths: Vec<f32> = columns.iter().map(|c| c.width / sum * width).collect();
        let rows = cells
            .into_iter()
            .map(|row| row.iter().zip(&widths).map(|(text, w)| face.wrap(text, TABLE, w - 2.0 * PAD)).collect())
            .collect();
        Self { columns, widths, rows, note: Vec::new(), totals: Vec::new() }
    }

    fn row_height(&self, i: usize) -> f32 {
        let lines = self.rows[i].iter().map(Vec::len).max().unwrap_or(1).max(1);
        lines as f32 * LINE + 2.0 * PAD
    }

    fn header_height(&self) -> f32 {
        LINE + 2.0 * PAD
    }

    fn note_height(&self) -> f32 {
        if self.note.is_empty() { 0.0 } else { self.note.len() as f32 * LINE + 6.0 }
    }

    fn totals_height(&self) -> f32 {
        if self.totals.is_empty() { 0.0 } else { self.totals.len() as f32 * LINE + 10.0 }
    }
}

fn print_table(face: &Typeface, rows: &[PrintRow], totals: &PrintTotals, width: f32) -> Table {
    let volumes = totals.has_volume();
    let mut columns = vec![
        Column { title: "Tile", width: 3.2, right: false },
        Column { title: "Folder", width: 2.2, right: false },
        Column { title: "Size", width: 1.8, right: false },
        Column { title: "Height", width: 1.0, right: true },
        Column { title: "Count", width: 0.9, right: true },
    ];
    let owned = totals.knows_owned;
    if owned {
        columns.push(Column { title: "Owned", width: 0.9, right: true });
        columns.push(Column { title: "To print", width: 1.0, right: true });
    }
    if volumes {
        columns.push(Column { title: "Volume", width: 1.3, right: true });
    }
    let cells = rows
        .iter()
        .map(|r| {
            let mut row = vec![
                r.display_name.clone(),
                r.folder.clone(),
                format!("{} x {} cells", r.size.w, r.size.h),
                format!("{} mm", format_mm(r.size.height_mm())),
                r.count.to_string(),
            ];
            if owned {
                row.push(r.owned.map_or(String::new(), |o| o.to_string()));
                row.push(r.to_print().to_string());
            }
            if volumes {
                row.push(r.volume_mm3.map_or("unknown".to_string(), |v| format!("{:.1} cm\u{b3}", v / 1000.0)));
            }
            row
        })
        .collect();
    let mut table = Table::new(face, columns, cells, width);
    table.note = face.wrap(
        "One row per STL file. Size is the footprint in 12.5 mm cells; 2 x 2 cells is a 1\" square.",
        TABLE,
        width,
    );
    let tiles = if totals.tiles == 1 { "1 tile".to_string() } else { format!("{} tiles", totals.tiles) };
    let files = if totals.files == 1 { "1 file".to_string() } else { format!("{} files", totals.files) };
    let mut summary = vec![format!("Total: {tiles} from {files}.")];
    if owned {
        summary.push(format!("{} to print after the ones you own.", totals.to_print));
    }
    if volumes {
        let mut line = format!(
            "Filament estimate: {:.1} cm\u{b3} of solid volume{}. Prints with infill use less.",
            totals.volume_mm3 / 1000.0,
            if owned { " for what's left to print" } else { "" }
        );
        if totals.files_without_volume > 0 {
            line.push_str(&format!(" {} files have no volume yet and aren't counted.", totals.files_without_volume));
        }
        summary.push(line);
    }
    table.totals = summary.iter().flat_map(|s| face.wrap(s, TABLE, width)).collect();
    table
}

fn placements_table(face: &Typeface, input: &ExportInput, width: f32) -> Table {
    let columns = vec![
        Column { title: "ID", width: 0.7, right: true },
        Column { title: "Tile", width: 3.0, right: false },
        Column { title: "Folder", width: 2.0, right: false },
        Column { title: "X", width: 0.7, right: true },
        Column { title: "Y", width: 0.7, right: true },
        Column { title: "Turn", width: 0.8, right: true },
        Column { title: "Height", width: 1.0, right: true },
        Column { title: "Level", width: 0.8, right: true },
    ];
    let rows = print_list(input);
    let cells = placements(input)
        .into_iter()
        .filter_map(|(id, n)| {
            let t = input.state.tiles.iter().find(|t| t.id == id)?;
            let row = rows.iter().find(|r| r.key == t.key);
            let meta = input.info.meta(&t.key);
            Some(vec![
                format!("#{n}"),
                row.map_or(meta.name.clone(), |r| r.display_name.clone()),
                meta.folder,
                format_cells(t.x),
                format_cells(t.y),
                format!("{}\u{b0}", t.rotation.degrees()),
                format!("{} mm", format_mm(f64::from(t.z) * mtp_core::units::CELL_MM)),
                t.level.to_string(),
            ])
        })
        .collect();
    let mut table = Table::new(face, columns, cells, width);
    table.note = face.wrap(
        "X and Y: the tile's bottom-left corner, in cells from the grid's bottom-left corner (1 cell is 12.5 mm). \
         Turn: counter-clockwise, seen from above. Height: the tile's base above the floor.",
        TABLE,
        width,
    );
    table
}

fn format_cells(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').to_string()
    }
}

/// Splits a table into pages, repeating the header on each.
fn plan_table_pages(pages: &mut Vec<PagePlan>, table: &Table, index: usize, paper: [f32; 2]) {
    let heading = if index == 0 { "Print list" } else { "Placements" };
    let room = paper[1] - HEADER_H - FOOTER_H - table.note_height() - table.header_height();
    let mut start = 0;
    let mut used = 0.0;
    let mut first = true;
    let mut push = |rows: Range<usize>, totals: bool, first: &mut bool| {
        let heading = if *first { heading.to_string() } else { format!("{heading} (continued)") };
        *first = false;
        pages.push(PagePlan { size: paper, heading, content: Content::Table { table: index, rows, totals } });
    };
    for i in 0..table.rows.len() {
        let h = table.row_height(i);
        if used + h > room && i > start {
            push(start..i, false, &mut first);
            start = i;
            used = 0.0;
        }
        used += h;
    }
    let end = table.rows.len();
    if used + table.totals_height() <= room {
        push(start..end, true, &mut first);
    } else {
        push(start..end, false, &mut first);
        push(end..end, true, &mut first);
    }
}

fn draw_table(c: &mut dyn Canvas, face: &Typeface, table: &Table, rows: Range<usize>, totals: bool) {
    let mut y = HEADER_H;
    let width: f32 = table.widths.iter().sum();
    let only_totals = rows.is_empty();
    if !only_totals {
        for line in &table.note {
            y += LINE;
            c.text(face, MARGIN, y - LINE * 0.25, TABLE, MUTED, line);
        }
        y += table.note_height() - table.note.len() as f32 * LINE;

        let hh = table.header_height();
        c.fill_rect(MARGIN, y, width, hh, [232, 232, 232]);
        let mut x = MARGIN;
        for (col, w) in table.columns.iter().zip(&table.widths) {
            cell_text(c, face, col.title, x, y + PAD + TABLE, *w, col.right);
            x += w;
        }
        y += hh;
    }
    for i in rows {
        let h = table.row_height(i);
        if i % 2 == 1 {
            c.fill_rect(MARGIN, y, width, h, [246, 246, 246]);
        }
        let mut x = MARGIN;
        for ((lines, col), w) in table.rows[i].iter().zip(&table.columns).zip(&table.widths) {
            for (k, line) in lines.iter().enumerate() {
                cell_text(c, face, line, x, y + PAD + TABLE + k as f32 * LINE, *w, col.right);
            }
            x += w;
        }
        y += h;
        c.line([MARGIN, y], [MARGIN + width, y], RULE, 0.4);
    }
    if totals {
        y += 10.0;
        for line in &table.totals {
            y += LINE;
            c.text(face, MARGIN, y - LINE * 0.25, TABLE, INK, line);
        }
    }
}

fn cell_text(c: &mut dyn Canvas, face: &Typeface, s: &str, x: f32, base: f32, w: f32, right: bool) {
    let s = face.clean(s);
    if right {
        draw::text_right(c, face, x + w - PAD, base, TABLE, INK, &s);
    } else {
        c.text(face, x + PAD, base, TABLE, INK, &s);
    }
}

struct PdfCanvas<'a, 'b> {
    surface: &'a mut krilla::surface::Surface<'b>,
    font: Font,
}

fn paint(color: Rgb) -> krilla::paint::Paint {
    rgb::Color::new(color[0], color[1], color[2]).into()
}

impl PdfCanvas<'_, '_> {
    fn fill_with(&mut self, color: Rgb) {
        self.surface.set_stroke(None);
        self.surface.set_fill(Some(Fill {
            paint: paint(color),
            opacity: NormalizedF32::ONE,
            rule: Default::default(),
        }));
    }
}

impl Canvas for PdfCanvas<'_, '_> {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        let Some(rect) = Rect::from_xywh(x, y, w, h) else { return };
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        if let Some(path) = pb.finish() {
            self.fill_with(color);
            self.surface.draw_path(&path);
        }
    }

    fn stroke_rect(&mut self, rect: [f32; 4], color: Rgb, width: f32, dashed: bool) {
        let [x, y, w, h] = rect;
        let Some(rect) = Rect::from_xywh(x, y, w, h) else { return };
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        if let Some(path) = pb.finish() {
            let dash = dashed.then(|| StrokeDash { array: vec![width * 3.0, width * 2.0], offset: 0.0 });
            self.surface.set_fill(None);
            self.surface.set_stroke(Some(Stroke { paint: paint(color), width, dash, ..Stroke::default() }));
            self.surface.draw_path(&path);
            self.surface.set_stroke(None);
        }
    }

    fn fill_polygon(&mut self, points: &[[f32; 2]], color: Rgb) {
        let mut pb = PathBuilder::new();
        for (i, [x, y]) in points.iter().copied().enumerate() {
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        pb.close();
        if let Some(path) = pb.finish() {
            self.fill_with(color);
            self.surface.draw_path(&path);
        }
    }

    fn line(&mut self, a: [f32; 2], b: [f32; 2], color: Rgb, width: f32) {
        let mut pb = PathBuilder::new();
        pb.move_to(a[0], a[1]);
        pb.line_to(b[0], b[1]);
        if let Some(path) = pb.finish() {
            self.surface.set_fill(None);
            self.surface.set_stroke(Some(Stroke { paint: paint(color), width, ..Stroke::default() }));
            self.surface.draw_path(&path);
            self.surface.set_stroke(None);
        }
    }

    /// Places the glyphs exactly as [`Typeface::layout`] does, so text in
    /// the PDF measures the same as in the PNG.
    fn text(&mut self, face: &Typeface, x: f32, y: f32, size: f32, color: Rgb, text: &str) {
        let laid = face.layout(text);
        if laid.is_empty() {
            return;
        }
        let glyphs: Vec<KrillaGlyph> = laid
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let advance = laid.get(i + 1).map_or(g.advance, |next| next.x - g.x);
                KrillaGlyph::new(GlyphId::new(u32::from(g.id)), advance, 0.0, 0.0, 0.0, g.range.clone(), None)
            })
            .collect();
        self.fill_with(color);
        self.surface.draw_glyphs(Point::from_xy(x, y), &glyphs, self.font.clone(), text, size, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DefaultInfo;
    use crate::testutil::{state, tile};

    fn render(s: &mtp_core::document::DocState, paper: Paper) -> lopdf::Document {
        let input = ExportInput { title: "Crypt of the Lich".into(), state: s, info: &DefaultInfo, level: None };
        let bytes = render_pdf(&input, &PdfOptions { paper, compress: false }).expect("pdf");
        assert!(bytes.starts_with(b"%PDF"));
        assert!(bytes.trim_ascii_end().ends_with(b"%%EOF"));
        lopdf::Document::load_mem(&bytes).expect("a valid PDF")
    }

    fn page_sizes(doc: &lopdf::Document) -> Vec<[f32; 2]> {
        doc.get_pages()
            .values()
            .map(|&id| {
                let page = doc.get_dictionary(id).expect("page");
                let mb = page.get(b"MediaBox").and_then(|m| m.as_array()).expect("media box");
                let n = |i: usize| mb[i].as_float().expect("number");
                [n(2) - n(0), n(3) - n(1)]
            })
            .collect()
    }

    fn small() -> mtp_core::document::DocState {
        state(
            40,
            40,
            vec![
                tile(1, "Packs/OpenForge/floor.stl", 2, 2, 2.0, 2.0),
                tile(2, "Packs/Dragonlock/floor.stl", 2, 2, 6.0, 2.0),
            ],
        )
    }

    #[test]
    fn small_layout_has_map_list_and_placements() {
        let doc = render(&small(), Paper::A4);
        assert_eq!(doc.get_pages().len(), 3);
        let text = doc.extract_text(&[1, 2, 3]).expect("text");
        for needle in
            ["Crypt of the Lich", "Level 0 map", "Print list", "floor (Dragonlock)", "Placements", "Page 3 of 3"]
        {
            assert!(text.contains(needle), "{needle:?} missing from {text}");
        }
    }

    #[test]
    fn large_layout_adds_detail_pages() {
        let s = state(200, 160, vec![tile(1, "a/b.stl", 2, 2, 0.0, 0.0), tile(2, "a/b.stl", 2, 2, 198.0, 158.0)]);
        let doc = render(&s, Paper::A4);
        let pages = doc.get_pages().len();
        assert!(pages > 3 + 4, "{pages} pages");
        let text = doc.extract_text(&[2]).expect("text");
        assert!(text.contains("section A1"), "{text}");
    }

    #[test]
    fn paper_sizes_and_orientation() {
        let a4 = page_sizes(&render(&small(), Paper::A4));
        let pt = |v: f32| mm(v);
        let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 0.5 && (a[1] - b[1]).abs() < 0.5;
        // An 8 x 4 cell map prints best on its side; the tables stay upright
        assert!(close(a4[0], [pt(297.0), pt(210.0)]), "{:?}", a4[0]);
        assert!(close(a4[1], [pt(210.0), pt(297.0)]), "{:?}", a4[1]);
        let letter = page_sizes(&render(&small(), Paper::Letter));
        assert!(close(letter[1], [pt(215.9), pt(279.4)]), "{:?}", letter[1]);
        let tabloid = page_sizes(&render(&small(), Paper::Tabloid));
        assert!(close(tabloid[2], [pt(279.4), pt(431.8)]), "{:?}", tabloid[2]);
    }

    #[test]
    fn long_tables_repeat_their_header() {
        let tiles =
            (0..150).map(|i| tile(i + 1, &format!("a/tile {i}.stl"), 1, 1, (i % 30) as f32, (i / 30) as f32)).collect();
        let doc = render(&state(40, 40, tiles), Paper::A4);
        let pages = doc.get_pages().len() as u32;
        let last = doc.extract_text(&[pages]).expect("text");
        assert!(last.contains("Placements (continued)") && last.contains("Level"), "{last}");
    }

    /// Draws every page through the PNG canvas, for looking at the layout
    /// without a PDF viewer: set `MTP_EXPORT_PREVIEW` to a folder.
    #[test]
    fn preview_pages() {
        let Some(dir) = std::env::var_os("MTP_EXPORT_PREVIEW") else { return };
        let dir = std::path::PathBuf::from(dir);
        let face = typeface().expect("font");
        let large = state(
            120,
            90,
            (0..40).map(|i| tile(i + 1, "a/b.stl", 2, 2, (i % 8) as f32 * 15.0, (i / 8) as f32 * 18.0)).collect(),
        );
        let mut rich = small();
        let mut raised = tile(3, "Packs/OpenForge/platform.stl", 1, 1, 2.0, 2.0);
        raised.z = 0.48;
        let mut turned = tile(4, "Packs/OpenForge/wall.stl", 2, 1, 10.0, 2.0);
        turned.rotation = mtp_core::footprint::Rotation::from_quarters(1);
        let mut lost = tile(5, "Packs/Lost/statue with a very long name indeed.stl", 1, 1, 12.0, 4.0);
        lost.size_known = false;
        let mut upstairs = tile(6, "Packs/OpenForge/floor.stl", 2, 2, 2.0, 2.0);
        upstairs.level = 1;
        rich.tiles.extend([raised, turned, lost, upstairs]);
        for (name, s) in [("rich", rich), ("large", large)] {
            let input = ExportInput { title: "Crypt of the Lich".into(), state: &s, info: &DefaultInfo, level: None };
            let plan = plan(&input, face, Paper::A4);
            for n in 0..plan.pages.len().min(4) {
                let draw = |c: &mut dyn Canvas| draw_page(c, face, &input.title, &plan, n);
                let png = crate::png::render_canvas(plan.pages[n].size, 2.0, draw).expect("png");
                std::fs::write(dir.join(format!("{name}-{}.png", n + 1)), png).expect("write");
            }
        }
    }

    #[test]
    fn sections_are_named_by_row_then_column() {
        assert_eq!(row_letters(0), "A");
        assert_eq!(row_letters(25), "Z");
        assert_eq!(row_letters(26), "AA");
    }
}
