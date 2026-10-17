//! Map drawing shared by the PNG and the PDF export. Each backend implements
//! [`Canvas`] (y down, in its own units: pixels or points), and everything
//! about how a tile looks lives here, once.

use mtp_core::color::luminance;

use crate::map_layout::{CellBounds, MapLayout, MapTile};
use crate::text::Typeface;

pub(crate) type Rgb = [u8; 3];

pub(crate) const BLACK: Rgb = [0, 0, 0];
pub(crate) const WHITE: Rgb = [255, 255, 255];
pub(crate) const INK: Rgb = [33, 33, 33];
pub(crate) const MUTED: Rgb = [105, 105, 105];
pub(crate) const CELL_LINE: Rgb = [228, 228, 228];
pub(crate) const INCH_LINE: Rgb = [180, 180, 180];
pub(crate) const RULE: Rgb = [200, 200, 200];

/// The few drawing operations exports need. Text is a cleaned single line
/// with its baseline at `y`.
pub(crate) trait Canvas {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb);
    fn stroke_rect(&mut self, rect: [f32; 4], color: Rgb, width: f32, dashed: bool);
    fn fill_polygon(&mut self, points: &[[f32; 2]], color: Rgb);
    fn line(&mut self, a: [f32; 2], b: [f32; 2], color: Rgb, width: f32);
    fn text(&mut self, face: &Typeface, x: f32, y: f32, size: f32, color: Rgb, text: &str);
}

/// Text helpers on top of [`Canvas::text`].
pub(crate) fn text_centered(c: &mut dyn Canvas, face: &Typeface, cx: f32, y: f32, size: f32, color: Rgb, s: &str) {
    let w = face.width(s, size);
    c.text(face, cx - w / 2.0, y, size, color, s);
}

pub(crate) fn text_right(c: &mut dyn Canvas, face: &Typeface, right: f32, y: f32, size: f32, color: Rgb, s: &str) {
    let w = face.width(s, size);
    c.text(face, right - w, y, size, color, s);
}

/// Black or white, whichever reads better on `fill`.
pub(crate) fn label_color(fill: Rgb) -> Rgb {
    if luminance(fill) > 140.0 { BLACK } else { WHITE }
}

pub(crate) fn darker(c: Rgb) -> Rgb {
    c.map(|v| (f32::from(v) * 0.55) as u8)
}

fn lighter(c: Rgb) -> Rgb {
    c.map(|v| ((f32::from(v) + 255.0) / 2.0) as u8)
}

/// Which cells of a map are drawn where.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MapView {
    /// Output position of the window's top-left corner.
    pub origin: [f32; 2],
    /// Output units per cell.
    pub cell: f32,
    /// The cells shown: the whole map, or one section of it.
    pub window: CellBounds,
    /// Smallest text worth drawing, in output units.
    pub min_text: f32,
    /// Size of the square numbers along the edges.
    pub axis_text: f32,
}

impl MapView {
    /// Output position of a grid point (y up).
    pub fn at(&self, x: f32, y: f32) -> [f32; 2] {
        [
            self.origin[0] + (x - self.window.min_x as f32) * self.cell,
            self.origin[1] + (self.window.max_y as f32 - y) * self.cell,
        ]
    }

    pub fn width(&self) -> f32 {
        self.window.width() as f32 * self.cell
    }

    pub fn height(&self) -> f32 {
        self.window.height() as f32 * self.cell
    }

    /// Room the square numbers take outside the map, left and top.
    pub fn axis_margin(face: &Typeface, axis_text: f32) -> f32 {
        face.width("000", axis_text) + axis_text * 0.6
    }
}

/// Draws the grid, the tiles in the view's window and, with `axes`, the 1"
/// square numbers along the top and left edges.
pub(crate) fn draw_map(c: &mut dyn Canvas, face: &Typeface, layout: &MapLayout, view: &MapView, axes: bool) {
    let win = view.window;
    let [ox, oy] = view.origin;
    c.fill_rect(ox, oy, view.width(), view.height(), WHITE);

    // Cell lines where they're far enough apart to help, 1" lines always
    let thin = (view.cell * 0.02).max(0.2);
    for x in win.min_x..=win.max_x {
        let inch = x.rem_euclid(2) == 0;
        if inch || view.cell >= 6.0 {
            let [px, _] = view.at(x as f32, 0.0);
            let (color, w) = if inch { (INCH_LINE, thin * 1.5) } else { (CELL_LINE, thin) };
            c.line([px, oy], [px, oy + view.height()], color, w);
        }
    }
    for y in win.min_y..=win.max_y {
        let inch = y.rem_euclid(2) == 0;
        if inch || view.cell >= 6.0 {
            let [_, py] = view.at(0.0, y as f32);
            let (color, w) = if inch { (INCH_LINE, thin * 1.5) } else { (CELL_LINE, thin) };
            c.line([ox, py], [ox + view.width(), py], color, w);
        }
    }

    for t in &layout.tiles {
        draw_tile(c, face, t, view);
    }

    c.stroke_rect([ox, oy, view.width(), view.height()], INCH_LINE, thin * 2.0, false);
    if axes {
        draw_axes(c, face, view);
    }
}

/// A tile's rectangle within the window, in output units: `[x, y, w, h]`.
fn visible_rect(t: &MapTile, view: &MapView) -> Option<[f32; 4]> {
    let win = view.window;
    let [x, y, w, h] = t.rect;
    let x0 = x.max(win.min_x as f32);
    let x1 = (x + w).min(win.max_x as f32);
    let y0 = y.max(win.min_y as f32);
    let y1 = (y + h).min(win.max_y as f32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let [px, py] = view.at(x0, y1);
    Some([px, py, (x1 - x0) * view.cell, (y1 - y0) * view.cell])
}

fn draw_tile(c: &mut dyn Canvas, face: &Typeface, t: &MapTile, view: &MapView) {
    let Some([x, y, w, h]) = visible_rect(t, view) else { return };
    let fill = if t.placeholder { lighter(t.color) } else { t.color };
    let border = (view.cell * 0.05).clamp(0.3, 3.0);
    // Inset so neighbours' borders don't merge into one line
    let inset = border / 2.0;
    c.fill_rect(x, y, w, h, fill);
    c.stroke_rect([x + inset, y + inset, w - border, h - border], darker(t.color), border, t.placeholder);

    let ink = label_color(fill);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);

    // Arrow on the front edge, pointing out of the tile
    let [dx, dy] = t.page_arrow();
    let reach = if dx.abs() > dy.abs() { w / 2.0 } else { h / 2.0 };
    let size = (w.min(h) * 0.16).min(view.cell * 0.3);
    if size >= view.min_text * 0.5 {
        let tip = [cx + dx * (reach - size * 0.4), cy + dy * (reach - size * 0.4)];
        let back = [tip[0] - dx * size, tip[1] - dy * size];
        let side = [-dy * size * 0.6, dx * size * 0.6];
        c.fill_polygon(&[tip, [back[0] + side[0], back[1] + side[1]], [back[0] - side[0], back[1] - side[1]]], ink);
    }

    // Number, and the name on tiles big enough to hold it
    let id = format!("#{}", t.label);
    let mut id_size = (view.cell * 0.42).min(h * 0.4);
    let id_width = face.width(&id, id_size);
    if id_width > w * 0.8 {
        id_size *= w * 0.8 / id_width;
    }

    // Height badge in the bottom-right corner, with the labels moved up
    // out of its way
    let mut label_cy = cy;
    if let Some(height) = &t.height_label {
        let size = (id_size * 0.6).max(view.min_text);
        let bw = face.width(height, size) + size * 0.6;
        let bh = size * 1.35;
        if bw <= w && bh * 2.0 <= h {
            let (bx, by) = (x + w - bw - border, y + h - bh - border);
            c.fill_rect(bx, by, bw, bh, WHITE);
            c.stroke_rect([bx, by, bw, bh], INK, border * 0.5, false);
            c.text(face, bx + size * 0.3, by + bh - size * 0.38, size, INK, height);
            label_cy -= bh / 2.0;
        }
    }

    if id_size >= view.min_text {
        let big = t.rect[2] >= 2.0 && t.rect[3] >= 2.0;
        let name_size = (id_size * 0.62).max(view.min_text);
        let name = if big { face.fit(&t.display_name, name_size, w * 0.86) } else { String::new() };
        if name.is_empty() {
            text_centered(c, face, cx, label_cy + id_size * 0.35, id_size, ink, &id);
        } else {
            text_centered(c, face, cx, label_cy, id_size, ink, &id);
            text_centered(c, face, cx, label_cy + name_size * 1.3, name_size, ink, &name);
        }
    }
}

/// Numbers each 1" square (2 x 2 cells) from the grid's corner, along the top
/// and the left edge, thinned out so neighbouring numbers never touch.
fn draw_axes(c: &mut dyn Canvas, face: &Typeface, view: &MapView) {
    let win = view.window;
    let size = view.axis_text;
    let square = view.cell * 2.0;
    let widest = face.width(&format!("{}", win.max_x.max(win.max_y) / 2 + 1), size);
    let step = ((widest + size) / square).ceil().max(((size * 1.6) / square).ceil()).max(1.0) as i32;
    let nice = [1, 2, 5, 10, 20, 25, 50, 100, 200, 500, 1000];
    let step = nice.into_iter().find(|&n| n >= step).unwrap_or(1000);

    let [ox, oy] = view.origin;
    for k in (win.min_x.div_euclid(2)..=(win.max_x - 1).div_euclid(2)).filter(|k| (k + 1) % step == 0 || step == 1) {
        let mid = (k * 2) as f32 + 1.0;
        if mid <= win.min_x as f32 || mid >= win.max_x as f32 {
            continue;
        }
        let [px, _] = view.at(mid, 0.0);
        text_centered(c, face, px, oy - size * 0.45, size, MUTED, &(k + 1).to_string());
    }
    for k in (win.min_y.div_euclid(2)..=(win.max_y - 1).div_euclid(2)).filter(|k| (k + 1) % step == 0 || step == 1) {
        let mid = (k * 2) as f32 + 1.0;
        if mid <= win.min_y as f32 || mid >= win.max_y as f32 {
            continue;
        }
        let [_, py] = view.at(0.0, mid);
        text_right(c, face, ox - size * 0.4, py + size * 0.35, size, MUTED, &(k + 1).to_string());
    }
}

/// The key to the map's symbols, as `(symbol, explanation)` drawn by
/// [`draw_key`].
const KEY: [(&str, &str); 4] = [
    ("#12", "Tile number, listed in the placements table"),
    ("arrow", "The side the tile's front faces"),
    ("+6 mm", "Height of the tile's base above the floor"),
    ("dashed", "STL file not found, so the size is a guess"),
];

/// Draws the key in one or two rows starting at `(x, y)` (top), no wider
/// than `width`. Returns the height used.
pub(crate) fn draw_key(c: &mut dyn Canvas, face: &Typeface, x: f32, y: f32, width: f32, size: f32) -> f32 {
    let sym = size * 1.6;
    let gap = size * 1.5;
    let line_h = size * 1.9;
    let (mut px, mut py) = (x, y);
    for (symbol, text) in KEY {
        let entry_w = sym + size * 0.5 + face.width(text, size) + gap;
        if px > x && px + entry_w > x + width {
            px = x;
            py += line_h;
        }
        let base = py + size;
        match symbol {
            "arrow" => {
                let (cx, cy) = (px + sym / 2.0, py + size * 0.55);
                let s = size * 0.55;
                c.fill_polygon(&[[cx, cy - s], [cx - s * 0.7, cy + s * 0.5], [cx + s * 0.7, cy + s * 0.5]], INK);
            }
            "dashed" => {
                c.stroke_rect([px + size * 0.1, py, sym - size * 0.2, size * 1.1], INK, size * 0.08, true);
            }
            "+6 mm" => {
                let s = size * 0.75;
                let bw = face.width(symbol, s) + s * 0.6;
                c.stroke_rect([px, py, bw, size * 1.1], INK, size * 0.06, false);
                c.text(face, px + s * 0.3, base - size * 0.12, s, INK, symbol);
                px += bw - sym;
            }
            _ => text_centered(c, face, px + sym / 2.0, base, size, INK, symbol),
        }
        c.text(face, px + sym + size * 0.5, base, size, MUTED, text);
        px += entry_w;
    }
    py + line_h - y
}
