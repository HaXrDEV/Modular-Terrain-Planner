//! The map as one picture: title, the map at 40 px per cell with 1" square
//! numbers, a key, and a legend of the files beside it.

use std::path::Path;

use ab_glyph::{Font, GlyphId, OutlineCurve};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, StrokeDash, Transform};

use crate::draw::{self, Canvas, INK, MUTED, MapView, Rgb};
use crate::map_layout::{MapLayout, map_layout};
use crate::text::{Typeface, typeface};
use crate::{ExportError, ExportInput};

/// Pixels per cell when the map fits.
pub const CELL_PX: f32 = 40.0;
/// Longest side of the image. Larger pictures are slow to open, and some
/// viewers refuse them.
pub const MAX_SIDE_PX: u32 = 8000;

const MARGIN: f32 = 24.0;
const TITLE: f32 = 28.0;
const SUBTITLE: f32 = 15.0;
const AXIS: f32 = 13.0;
const KEY: f32 = 13.0;
const LEGEND_W: f32 = 330.0;
const LEGEND_ROW: f32 = 26.0;
const LEGEND_TEXT: f32 = 14.0;

/// An encoded PNG and its size in pixels.
#[derive(Clone, Debug)]
pub struct RenderedPng {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
    /// Where things were drawn, for tests: map view and swatch corners,
    /// before scaling by `scale`.
    #[cfg(test)]
    pub(crate) view: MapView,
    #[cfg(test)]
    pub(crate) swatches: Vec<[f32; 2]>,
    #[cfg(test)]
    pub(crate) scale: f32,
}

/// Renders the export's level (or all levels together) to PNG bytes.
pub fn render_png(input: &ExportInput) -> Result<RenderedPng, ExportError> {
    input.check_not_empty()?;
    let face = typeface()?;
    let layout = map_layout(input, input.level).ok_or(ExportError::Empty)?;
    let (map_w, map_h) = (layout.bounds.width() as f32, layout.bounds.height() as f32);

    let axis_margin = MapView::axis_margin(face, AXIS);
    let header = MARGIN + TITLE + 10.0 + SUBTITLE + 18.0 + AXIS * 1.6;
    let left = MARGIN + axis_margin;
    let key_h = KEY * 1.9 * 2.0 + 16.0;
    // Shrink the cells first so text stays readable; only scale the whole
    // picture if the frame alone is still too big
    let max = MAX_SIDE_PX as f32;
    let cell = CELL_PX.min((max - left - LEGEND_W - 2.0 * MARGIN) / map_w).min((max - header - key_h - MARGIN) / map_h);
    let cell = cell.max(1.0);

    let view = MapView { origin: [left, header], cell, window: layout.bounds, min_text: 7.0, axis_text: AXIS };
    let map_bottom = header + view.height();
    let map_right = left + view.width();

    let legend_x = map_right + 32.0;
    let legend_top = header;
    let legend_h_avail = (view.height() + key_h).max(400.0);
    let per_column = ((legend_h_avail - LEGEND_ROW * 1.5) / LEGEND_ROW).floor().max(1.0) as usize;
    let columns = layout.legend.len().div_ceil(per_column).max(1);
    let rows_in_tallest = layout.legend.len().min(per_column);
    let legend_bottom = legend_top + LEGEND_ROW * 1.5 + rows_in_tallest as f32 * LEGEND_ROW;

    let width = (legend_x + columns as f32 * LEGEND_W + MARGIN).max(left + 640.0);
    let height = (map_bottom + key_h).max(legend_bottom) + MARGIN;
    let scale = (max / width.max(height)).min(1.0);
    let (px_w, px_h) = ((width * scale).ceil() as u32, (height * scale).ceil() as u32);

    let pixmap = Pixmap::new(px_w.max(1), px_h.max(1))
        .ok_or(ExportError::Encode { format: "PNG image", detail: format!("{px_w} x {px_h} pixels is too large") })?;
    let mut canvas = PngCanvas { pixmap, ts: Transform::from_scale(scale, scale) };
    canvas.pixmap.fill(tiny_skia::Color::WHITE);

    let c: &mut dyn Canvas = &mut canvas;
    let title = face.fit(&input.title, TITLE, width - 2.0 * MARGIN);
    c.text(face, MARGIN, MARGIN + TITLE * 0.8, TITLE, INK, &title);
    let subtitle = subtitle(input, &layout);
    c.text(face, MARGIN, MARGIN + TITLE + 10.0 + SUBTITLE * 0.8, SUBTITLE, MUTED, &face.clean(&subtitle));

    draw::draw_map(c, face, &layout, &view, true);
    draw::draw_key(c, face, left, map_bottom + 16.0, view.width().max(600.0), KEY);
    let swatches = draw_legend(c, face, &layout, legend_x, legend_top, per_column);

    let bytes = encode(canvas.pixmap)?;
    #[cfg(not(test))]
    let _ = swatches;
    Ok(RenderedPng {
        width: px_w,
        height: px_h,
        bytes,
        #[cfg(test)]
        view,
        #[cfg(test)]
        swatches,
        #[cfg(test)]
        scale,
    })
}

/// Renders the export and writes it to `path`, replacing the file only once
/// the whole image is encoded.
pub fn export_png(path: &Path, input: &ExportInput) -> Result<(), ExportError> {
    let png = render_png(input)?;
    crate::write_file(path, &png.bytes)
}

pub(crate) fn subtitle(input: &ExportInput, layout: &MapLayout) -> String {
    let level = match layout.level {
        Some(l) => format!("Level {l}"),
        None if input.levels().len() > 1 => "All levels".to_string(),
        None => format!("Level {}", input.levels().first().copied().unwrap_or(0)),
    };
    let n = layout.tiles.len();
    let tiles = if n == 1 { "1 tile".to_string() } else { format!("{n} tiles") };
    format!("{level}, {tiles}. North is up. Numbers along the edges count 1\" squares (2 x 2 cells).")
}

/// Colour, name, count and size of each file, in columns of `per_column`.
/// Returns each swatch's top-left corner.
fn draw_legend(
    c: &mut dyn Canvas,
    face: &Typeface,
    layout: &MapLayout,
    x: f32,
    y: f32,
    per_column: usize,
) -> Vec<[f32; 2]> {
    c.text(face, x, y + 16.0, 18.0, INK, "Tiles");
    let mut corners = Vec::with_capacity(layout.legend.len());
    for (i, row) in layout.legend.iter().enumerate() {
        let col = (i / per_column) as f32;
        let (rx, ry) = (x + col * LEGEND_W, y + LEGEND_ROW * 1.5 + (i % per_column) as f32 * LEGEND_ROW);
        let color: Rgb = layout.tiles.iter().find(|t| t.key == row.key).map_or([200, 200, 200], |t| t.color);
        let s = 18.0;
        c.fill_rect(rx, ry, s, s, color);
        c.stroke_rect([rx + 0.5, ry + 0.5, s - 1.0, s - 1.0], draw::darker(color), 1.0, false);
        corners.push([rx, ry]);
        let size = format!("{} x {}", row.size.w, row.size.h);
        let count = format!("{}x", row.count);
        let right = rx + LEGEND_W - 16.0;
        draw::text_right(c, face, right, ry + 14.0, LEGEND_TEXT - 1.0, MUTED, &size);
        let size_w = face.width(&size, LEGEND_TEXT - 1.0) + 10.0;
        let count_w = face.width("000x", LEGEND_TEXT);
        draw::text_right(c, face, right - size_w, ry + 14.0, LEGEND_TEXT, INK, &count);
        let name = face.fit(&row.display_name, LEGEND_TEXT, right - size_w - count_w - 8.0 - (rx + s + 8.0));
        c.text(face, rx + s + 8.0, ry + 14.0, LEGEND_TEXT, INK, &name);
    }
    corners
}

fn encode(pixmap: Pixmap) -> Result<Vec<u8>, ExportError> {
    let fail = |e: ::png::EncodingError| ExportError::Encode { format: "PNG image", detail: e.to_string() };
    let (w, h) = (pixmap.width(), pixmap.height());
    let data = pixmap.take_demultiplied();
    let mut out = Vec::new();
    let mut encoder = ::png::Encoder::new(&mut out, w, h);
    encoder.set_color(::png::ColorType::Rgba);
    encoder.set_depth(::png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(&data).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(out)
}

/// Draws anything onto a white picture of `size` units at `scale` pixels
/// each, for previewing PDF pages in tests.
#[cfg(test)]
pub(crate) fn render_canvas(
    size: [f32; 2],
    scale: f32,
    draw: impl FnOnce(&mut dyn Canvas),
) -> Result<Vec<u8>, ExportError> {
    let (w, h) = ((size[0] * scale).ceil() as u32, (size[1] * scale).ceil() as u32);
    let pixmap = Pixmap::new(w, h).ok_or(ExportError::Encode { format: "PNG image", detail: "no size".into() })?;
    let mut canvas = PngCanvas { pixmap, ts: Transform::from_scale(scale, scale) };
    canvas.pixmap.fill(tiny_skia::Color::WHITE);
    draw(&mut canvas);
    encode(canvas.pixmap)
}

struct PngCanvas {
    pixmap: Pixmap,
    ts: Transform,
}

fn paint(color: Rgb) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], 255);
    p.anti_alias = true;
    p
}

impl Canvas for PngCanvas {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        if let Some(r) = Rect::from_xywh(x, y, w, h) {
            self.pixmap.fill_rect(r, &paint(color), self.ts, None);
        }
    }

    fn stroke_rect(&mut self, rect: [f32; 4], color: Rgb, width: f32, dashed: bool) {
        let [x, y, w, h] = rect;
        let Some(r) = Rect::from_xywh(x, y, w, h) else { return };
        let path = PathBuilder::from_rect(r);
        let dash = if dashed { StrokeDash::new(vec![width * 3.0, width * 2.0], 0.0) } else { None };
        let stroke = Stroke { width, dash, ..Stroke::default() };
        self.pixmap.stroke_path(&path, &paint(color), &stroke, self.ts, None);
    }

    fn fill_polygon(&mut self, points: &[[f32; 2]], color: Rgb) {
        let mut pb = PathBuilder::new();
        for (i, [x, y]) in points.iter().copied().enumerate() {
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        pb.close();
        if let Some(path) = pb.finish() {
            self.pixmap.fill_path(&path, &paint(color), FillRule::Winding, self.ts, None);
        }
    }

    fn line(&mut self, a: [f32; 2], b: [f32; 2], color: Rgb, width: f32) {
        let mut pb = PathBuilder::new();
        pb.move_to(a[0], a[1]);
        pb.line_to(b[0], b[1]);
        if let Some(path) = pb.finish() {
            let stroke = Stroke { width, ..Stroke::default() };
            self.pixmap.stroke_path(&path, &paint(color), &stroke, self.ts, None);
        }
    }

    /// Fills the glyph outlines as paths, so text is anti-aliased like
    /// everything else and sits exactly where the PDF puts it.
    fn text(&mut self, face: &Typeface, x: f32, y: f32, size: f32, color: Rgb, text: &str) {
        let k = size / face.units_per_em();
        let mut pb = PathBuilder::new();
        for g in face.layout(text) {
            let Some(outline) = face.font.outline(GlyphId(g.id)) else { continue };
            let gx = x + g.x * size;
            let at = |p: ab_glyph::Point| (gx + p.x * k, y - p.y * k);
            let mut last: Option<(f32, f32)> = None;
            for curve in &outline.curves {
                let start = match curve {
                    OutlineCurve::Line(p, _) | OutlineCurve::Quad(p, _, _) | OutlineCurve::Cubic(p, _, _, _) => at(*p),
                };
                if last.is_none_or(|l| (l.0 - start.0).abs() > 1e-4 || (l.1 - start.1).abs() > 1e-4) {
                    if last.is_some() {
                        pb.close();
                    }
                    pb.move_to(start.0, start.1);
                }
                let end = match curve {
                    OutlineCurve::Line(_, p) => {
                        let p = at(*p);
                        pb.line_to(p.0, p.1);
                        p
                    }
                    OutlineCurve::Quad(_, c, p) => {
                        let (c, p) = (at(*c), at(*p));
                        pb.quad_to(c.0, c.1, p.0, p.1);
                        p
                    }
                    OutlineCurve::Cubic(_, c1, c2, p) => {
                        let (c1, c2, p) = (at(*c1), at(*c2), at(*p));
                        pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p.0, p.1);
                        p
                    }
                };
                last = Some(end);
            }
            if last.is_some() {
                pb.close();
            }
        }
        if let Some(path) = pb.finish() {
            self.pixmap.fill_path(&path, &paint(color), FillRule::Winding, self.ts, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DefaultInfo;
    use crate::testutil::{state, tile};

    fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = ::png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("png header");
        let mut buf = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buf).expect("png data");
        (info.width, info.height, buf)
    }

    fn pixel(img: &(u32, u32, Vec<u8>), x: f32, y: f32) -> [u8; 3] {
        let i = ((y as u32 * img.0 + x as u32) * 4) as usize;
        [img.2[i], img.2[i + 1], img.2[i + 2]]
    }

    #[test]
    fn draws_tiles_and_legend() {
        let s = state(
            30,
            20,
            vec![
                tile(1, "Packs/OpenForge/floor.stl", 4, 4, 2.0, 2.0),
                tile(2, "Packs/OpenForge/wall.stl", 2, 1, 10.0, 8.0),
            ],
        );
        let input = ExportInput { title: "Crypt".into(), state: &s, info: &DefaultInfo, level: None };
        let png = render_png(&input).expect("png");
        let img = decode(&png.bytes);
        assert_eq!((img.0, img.1), (png.width, png.height));
        // Bounds are x 1..13, y 1..10: 12 x 9 cells at 40 px, nothing scaled
        assert_eq!(png.scale, 1.0);
        assert_eq!(png.view.width(), 12.0 * 40.0);
        assert_eq!(png.view.height(), 9.0 * 40.0);
        assert!(png.width as f32 > png.view.origin[0] + png.view.width() + LEGEND_W);

        // Left of the label, at mid height, the floor tile shows its colour
        let floor = mtp_core::color::tile_color("OpenForge", "floor.stl");
        let [px, py] = png.view.at(3.0, 4.0);
        let got = pixel(&img, px, py);
        assert!(got.iter().zip(floor).all(|(a, b)| a.abs_diff(b) <= 8), "{got:?} vs {floor:?}");

        // Both files have a swatch in their colour
        assert_eq!(png.swatches.len(), 2);
        let [sx, sy] = png.swatches[0];
        assert_eq!(pixel(&img, sx + 9.0, sy + 9.0), floor);
    }

    #[test]
    fn huge_maps_stay_under_the_limit() {
        let s = state(2000, 600, vec![tile(1, "a/b.stl", 2, 2, 0.0, 0.0), tile(2, "a/b.stl", 2, 2, 1998.0, 598.0)]);
        let input = ExportInput { title: "Huge".into(), state: &s, info: &DefaultInfo, level: None };
        let png = render_png(&input).expect("png");
        assert!(png.width <= MAX_SIDE_PX && png.height <= MAX_SIDE_PX, "{} x {}", png.width, png.height);
        assert!(png.width > 7000, "uses the room it has");
    }
}
