//! The 3D/2D view: turns pointer and keys into tool actions and draws the
//! layout with the renderer into a texture shown in the UI.

use std::collections::HashSet;

use eframe::egui::{self, Color32, Key, PointerButton, Pos2, Rect, Sense, Stroke, Vec2 as EVec2};
use eframe::egui_wgpu;
use glam::{Vec2, Vec3};
use mtp_core::color::tile_color;
use mtp_core::rules::{self, Height, Mode, Proposal, Refusal};
use mtp_core::tile::{PlacedId, TileKey};
use mtp_render::picking::{self, MeshView};
use mtp_render::scene::screen_rect;
use mtp_render::{Camera, DrawKind, DrawTile, Frame, FrameStats, Palette, Projection, Renderer, Target};

use crate::editor::{Editor, Tool};
use crate::library::Library;

/// Tiers uploaded per frame at most, so a big layout sharpens over a few
/// frames instead of stalling one.
const UPLOADS_PER_FRAME: usize = 24;

#[derive(Default)]
enum Drag {
    #[default]
    None,
    /// Moving the selection: where the drag started on the ground, at the
    /// height of the tile it started on.
    Move {
        start: Vec2,
        height: f32,
    },
    BoxSelect {
        start: Pos2,
    },
    /// Placing tiles along the drag; remembers the last origin used.
    Paint {
        last: Option<(i32, i32)>,
    },
    Erase,
    Pan,
}

/// What the pointer is over.
#[derive(Clone, Copy, Debug)]
pub struct Hover {
    /// Point on the ground (or on the top of the tile under the pointer).
    pub point: Vec2,
    pub tile: Option<PlacedId>,
}

pub struct Viewport {
    pub camera: Camera,
    target: Option<(Target, egui::TextureId)>,
    drag: Drag,
    pub show_grid: bool,
    pub stats: FrameStats,
    pub hover: Option<Hover>,
    /// Why the current ghost would be refused, for the status bar.
    pub refusal: Option<Refusal>,
    context_tile: Option<PlacedId>,
    context_point: Option<Vec2>,
    framed_once: bool,
}

/// Everything the viewport works with for one frame.
pub struct ViewCtx<'a> {
    pub editor: &'a mut Editor,
    pub library: &'a Library,
    pub renderer: &'a mut Renderer,
    pub render_state: &'a egui_wgpu::RenderState,
    pub detail: f32,
    /// A dialog has the keyboard.
    pub keys_blocked: bool,
}

fn color_for(library: &Library, key: &TileKey) -> [u8; 3] {
    library.tile(key).map_or_else(|| tile_color(key.folder_name(), key.file_name()), |t| t.color)
}

fn draw_tile(p: &Proposal, z: f32, color: [u8; 3], kind: DrawKind) -> DrawTile {
    DrawTile { key: p.key.clone(), x: p.x, y: p.y, z, size: p.size, rotation: p.rotation, color, selected: false, kind }
}

impl Viewport {
    pub fn new(cols: u32, rows: u32, projection: Projection) -> Self {
        Self {
            camera: Camera::for_grid(cols, rows, projection),
            target: None,
            drag: Drag::None,
            show_grid: true,
            stats: FrameStats::default(),
            hover: None,
            refusal: None,
            context_tile: None,
            context_point: None,
            framed_once: false,
        }
    }

    pub fn reset_view(&mut self, cols: u32, rows: u32) {
        self.camera = Camera::for_grid(cols, rows, self.camera.projection);
    }

    pub fn toggle_projection(&mut self) {
        // Keep about the same area in view: top-down distance is half the
        // visible height, perspective distance is that over tan(fov / 2)
        let half_fov = (mtp_render::camera::FOV_Y_DEGREES / 2.0).to_radians().tan();
        self.camera.projection = match self.camera.projection {
            Projection::TopDown => {
                self.camera.distance /= half_fov;
                Projection::Perspective
            }
            Projection::Perspective => {
                self.camera.distance *= half_fov;
                Projection::TopDown
            }
        };
    }

    /// Fits the selection, else the layout, else the grid, into view.
    pub fn frame(&mut self, editor: &Editor) {
        let tiles: Vec<_> =
            if editor.selection.is_empty() { editor.doc.tiles().iter().collect() } else { editor.selected_tiles() };
        if tiles.is_empty() {
            let g = editor.doc.grid();
            self.camera.frame(Vec3::ZERO, Vec3::new(g.cols as f32, g.rows as f32, 0.0), 1.1);
            return;
        }
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for t in tiles {
            let b = t.bounds();
            lo = lo.min(Vec3::from(b.min));
            hi = hi.max(Vec3::from(b.max));
        }
        self.camera.frame(lo, hi, 1.25);
    }

    fn ensure_target(&mut self, renderer: &Renderer, rs: &egui_wgpu::RenderState, size: [u32; 2]) {
        if self.target.as_ref().is_some_and(|(t, _)| t.size == size) {
            return;
        }
        let target = renderer.create_target(size);
        let mut egui_renderer = rs.renderer.write();
        let id = match self.target.take() {
            Some((_, id)) => {
                egui_renderer.update_egui_texture_from_wgpu_texture(
                    &rs.device,
                    &target.output_view,
                    wgpu_nearest(),
                    id,
                );
                id
            }
            None => egui_renderer.register_native_texture(&rs.device, &target.output_view, wgpu_nearest()),
        };
        self.target = Some((target, id));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: ViewCtx<'_>) {
        let ViewCtx { editor, library, renderer, render_state, detail, keys_blocked } = cx;
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Layout view"));
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
        let aspect = rect.width() / rect.height().max(1.0);
        if !self.framed_once {
            self.framed_once = true;
            self.frame(editor);
        }

        let typing = ui.ctx().egui_wants_keyboard_input() || keys_blocked;
        let (modifiers, space) = ui.input(|i| (i.modifiers, i.key_down(Key::Space)));
        let mode = if modifiers.command { Mode::Free } else { Mode::Snap };
        let uv_of = |p: Pos2| {
            let v = (p - rect.min) / rect.size();
            Vec2::new(v.x, v.y)
        };

        // Camera: right drag orbits (3D) or pans (2D); middle or Space drag pans; wheel zooms
        let pan_drag = response.dragged_by(PointerButton::Middle)
            || (space && response.dragged_by(PointerButton::Primary))
            || (self.camera.projection == Projection::TopDown && response.dragged_by(PointerButton::Secondary));
        if pan_drag {
            let d = response.drag_delta();
            self.camera.pan(Vec2::new(d.x, d.y), rect.height());
        } else if response.dragged_by(PointerButton::Secondary) {
            let d = response.drag_delta();
            self.camera.orbit(Vec2::new(d.x, d.y));
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0
                && let Some(p) = response.hover_pos()
            {
                self.camera.zoom_at(scroll / 50.0, uv_of(p), aspect);
            }
        }
        if !typing {
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            let speed = self.camera.cells_per_pixel(rect.height()) * 900.0 * dt;
            let mut pan = Vec2::ZERO;
            ui.input(|i| {
                for (k, v) in [(Key::W, Vec2::Y), (Key::S, -Vec2::Y), (Key::A, -Vec2::X), (Key::D, Vec2::X)] {
                    if i.key_down(k) && !i.modifiers.command {
                        pan += v;
                    }
                }
            });
            if pan != Vec2::ZERO {
                self.camera.nudge(pan * speed);
                ui.ctx().request_repaint();
            }
            self.keyboard(ui, editor, mode);
        }

        // What the pointer is over
        let grid = editor.doc.grid();
        let doc_tiles: Vec<DrawTile> = editor
            .doc
            .tiles()
            .iter()
            .map(|t| DrawTile {
                key: t.key.clone(),
                x: t.x,
                y: t.y,
                z: t.z,
                size: t.size,
                rotation: t.rotation,
                color: color_for(library, &t.key),
                selected: editor.selection.contains(&t.id),
                kind: DrawKind::Tile,
            })
            .collect();
        let ids: Vec<PlacedId> = editor.doc.tiles().iter().map(|t| t.id).collect();
        let mesh_of = |t: &DrawTile| {
            library.tile(&t.key).map(|lt| {
                let lod = &lt.mesh.lods[lt.preview_tier()];
                MeshView { positions: &lod.positions, indices: &lod.indices }
            })
        };
        self.hover = response.hover_pos().or(response.interact_pointer_pos()).and_then(|p| {
            let (origin, dir) = self.camera.ray(uv_of(p), aspect);
            match picking::pick(&doc_tiles, origin, dir, mesh_of) {
                Some((i, t)) => Some(Hover { point: (origin + dir * t).truncate(), tile: Some(ids[i]) }),
                None => self.camera.ground_point(uv_of(p), aspect, 0.0).map(|point| Hover { point, tile: None }),
            }
        });
        let ground_at = |p: Pos2, height: f32| self.camera.ground_point(uv_of(p), aspect, height);

        // Alt+click picks up a tile (eyedropper)
        let alt_click = modifiers.alt && response.clicked_by(PointerButton::Primary);
        if alt_click
            && let Some(id) = self.hover.and_then(|h| h.tile)
            && let Some(t) = editor.doc.tile(id)
        {
            let (key, size, rotation) = (t.key.clone(), t.size, t.rotation);
            editor.tell(format!("Placing {}.", key.stem()));
            editor.tool = Tool::Place { key, size, rotation };
        }

        // Ghosts and tool actions
        let mut ghosts: Vec<DrawTile> = Vec::new();
        self.refusal = None;
        let primary_drag_start = response.drag_started_by(PointerButton::Primary) && !space && !alt_click;
        let primary_click = response.clicked_by(PointerButton::Primary) && !alt_click;
        match editor.tool.clone() {
            Tool::Place { key, size, rotation } => {
                if let Some(h) = self.hover {
                    let (w, d) = mtp_core::footprint::effective_size(size, rotation);
                    let mut x = h.point.x - w as f32 / 2.0;
                    let mut y = h.point.y - d as f32 / 2.0;
                    if mode == Mode::Snap {
                        x = x.round();
                        y = y.round();
                    }
                    let proposal =
                        Proposal { key: key.clone(), size, size_known: true, x, y, z: 0.0, rotation, level: 0 };
                    let check = |m| {
                        rules::arrange(
                            grid,
                            editor.doc.tiles(),
                            &HashSet::new(),
                            std::slice::from_ref(&proposal),
                            m,
                            Height::Settle,
                        )
                    };
                    let (z, kind) = match check(mode) {
                        Ok(z) => (z[0], DrawKind::Ghost),
                        Err(r) => {
                            self.refusal = Some(r);
                            (check(Mode::Free).map_or(0.0, |z| z[0]), DrawKind::RefusedGhost)
                        }
                    };
                    ghosts.push(draw_tile(&proposal, z, color_for(library, &key), kind));
                    let origin = (x.floor() as i32, y.floor() as i32);
                    // Painting over tiles of the same kind leaves them alone
                    // instead of stacking copies; a click still stacks
                    let same_there = editor
                        .doc
                        .tiles()
                        .iter()
                        .any(|t| t.key == key && t.rotation == rotation && t.x == x && t.y == y);
                    let paint = matches!(self.drag, Drag::Paint { last } if last != Some(origin)) && !same_there;
                    if (primary_click || paint || primary_drag_start) && kind == DrawKind::Ghost {
                        editor.place(vec![proposal], mode);
                        self.drag = Drag::Paint { last: Some(origin) };
                    } else if primary_drag_start {
                        self.drag = Drag::Paint { last: Some(origin) };
                    }
                }
            }
            Tool::Paste => {
                if let (Some(h), Some(clip)) = (self.hover, editor.clipboard.clone()) {
                    let mut x = h.point.x - clip.w / 2.0;
                    let mut y = h.point.y - clip.h / 2.0;
                    if mode == Mode::Snap {
                        x = x.round();
                        y = y.round();
                    }
                    let proposals = clip.at(x, y);
                    let result =
                        rules::arrange(grid, editor.doc.tiles(), &HashSet::new(), &proposals, mode, Height::Settle);
                    let kind = if result.is_ok() { DrawKind::Ghost } else { DrawKind::RefusedGhost };
                    if let Err(r) = result {
                        self.refusal = Some(r);
                    }
                    let zs = result.unwrap_or_else(|_| proposals.iter().map(|p| p.z).collect());
                    for (p, z) in proposals.iter().zip(zs) {
                        ghosts.push(draw_tile(p, z, color_for(library, &p.key), kind));
                    }
                    if primary_click && kind == DrawKind::Ghost {
                        editor.paste_at(x, y, mode);
                    }
                }
            }
            Tool::Erase => {
                let erase_now = primary_click || primary_drag_start || matches!(self.drag, Drag::Erase);
                if primary_drag_start {
                    self.drag = Drag::Erase;
                }
                if erase_now && let Some(id) = self.hover.and_then(|h| h.tile) {
                    editor.delete(&[id]);
                }
            }
            Tool::Select => {
                if primary_click {
                    match self.hover.and_then(|h| h.tile) {
                        Some(id) if modifiers.shift => {
                            if !editor.selection.remove(&id) {
                                editor.selection.insert(id);
                            }
                        }
                        Some(id) => editor.selection = [id].into(),
                        None if !modifiers.shift => editor.selection.clear(),
                        None => {}
                    }
                }
                if primary_drag_start && let Some(p) = response.interact_pointer_pos() {
                    match self.hover.and_then(|h| h.tile) {
                        Some(id) => {
                            if !editor.selection.contains(&id) {
                                if !modifiers.shift {
                                    editor.selection.clear();
                                }
                                editor.selection.insert(id);
                            }
                            let height = editor.doc.tile(id).map_or(0.0, |t| t.z);
                            if let Some(start) = ground_at(p, height) {
                                self.drag = Drag::Move { start, height };
                            }
                        }
                        None => self.drag = Drag::BoxSelect { start: p },
                    }
                }
                if let Drag::Move { start, height } = self.drag
                    && let Some(p) = response.interact_pointer_pos().or(response.hover_pos())
                    && let Some(now) = ground_at(p, height)
                {
                    let mut delta = now - start;
                    if mode == Mode::Snap {
                        delta = delta.round();
                    }
                    let moving: HashSet<PlacedId> = editor.selection.iter().copied().collect();
                    let proposals: Vec<Proposal> = editor
                        .selected_tiles()
                        .iter()
                        .map(|t| Proposal { x: t.x + delta.x, y: t.y + delta.y, ..Proposal::from_placed(t) })
                        .collect();
                    let height_rule = if mode == Mode::Snap { Height::Settle } else { Height::Keep };
                    let result = rules::arrange(grid, editor.doc.tiles(), &moving, &proposals, mode, height_rule);
                    let kind = if result.is_ok() { DrawKind::Ghost } else { DrawKind::RefusedGhost };
                    if let Err(r) = &result {
                        self.refusal = Some(*r);
                    }
                    if delta != Vec2::ZERO {
                        let zs = result.unwrap_or_else(|_| proposals.iter().map(|p| p.z).collect());
                        for (p, z) in proposals.iter().zip(zs) {
                            ghosts.push(draw_tile(p, z, color_for(library, &p.key), kind));
                        }
                    }
                    if response.drag_stopped() {
                        if delta != Vec2::ZERO {
                            editor.move_selection(delta.x, delta.y, mode);
                        }
                        self.drag = Drag::None;
                    }
                }
                if let Drag::BoxSelect { start } = self.drag
                    && let Some(now) = response.interact_pointer_pos().or(response.hover_pos())
                {
                    let band = Rect::from_two_pos(start, now);
                    if response.drag_stopped() {
                        let vp = self.camera.view_proj(aspect);
                        let view = glam::Vec2::new(rect.width(), rect.height());
                        if !modifiers.shift {
                            editor.selection.clear();
                        }
                        for (t, id) in doc_tiles.iter().zip(&ids) {
                            let (min, max) = t.bounds();
                            if let Some((lo, hi)) = screen_rect(&vp, min, max, view) {
                                let r = Rect::from_min_max(
                                    rect.min + EVec2::new(lo.x, lo.y),
                                    rect.min + EVec2::new(hi.x, hi.y),
                                );
                                if band.intersects(r) {
                                    editor.selection.insert(*id);
                                }
                            }
                        }
                        self.drag = Drag::None;
                    }
                }
            }
        }
        if pan_drag && primary_drag_start {
            self.drag = Drag::Pan;
        }
        // Moves and box selections finish above; anything else ends with the button
        if response.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
            self.drag = Drag::None;
        }

        // Right click: a menu for the tile under the pointer, never a delete
        if response.secondary_clicked() {
            self.context_tile = self.hover.and_then(|h| h.tile);
            self.context_point = self.hover.map(|h| h.point);
        }
        response.context_menu(|ui| self.context_menu(ui, editor, mode));

        // Upload what the last frame wanted, then draw
        let mut uploaded = 0;
        for (key, tier) in std::mem::take(&mut self.stats.wanted) {
            if uploaded >= UPLOADS_PER_FRAME {
                ui.ctx().request_repaint();
                break;
            }
            if let Some(t) = library.tile(&key)
                && let Some(lod) = t.mesh.lods.get(tier)
            {
                renderer.upload_tier(&key, tier, &lod.positions, &lod.indices);
                uploaded += 1;
            }
        }
        if uploaded > 0 {
            ui.ctx().request_repaint();
        }

        self.ensure_target(renderer, render_state, size);
        let Some((target, texture)) = self.target.as_mut() else { return };
        let image_rect = editor.doc.ground_image().map(|img| img.rect).filter(|r| r[2] > 0.0 && r[3] > 0.0);
        self.stats = renderer.render(
            target,
            &Frame {
                view_proj: self.camera.view_proj(aspect),
                grid: [grid.cols, grid.rows],
                tiles: &doc_tiles,
                ghosts: &ghosts,
                palette: if ui.visuals().dark_mode { Palette::DARK } else { Palette::LIGHT },
                pixels_per_point: ppp,
                show_ground: true,
                show_grid: self.show_grid,
                image_rect,
                lod_bias: detail,
            },
        );
        if !self.stats.wanted.is_empty() {
            ui.ctx().request_repaint();
        }
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter().with_clip_rect(rect).image(*texture, rect, uv, Color32::WHITE);
        if let Drag::BoxSelect { start } = self.drag
            && let Some(now) = response.interact_pointer_pos()
        {
            let band = Rect::from_two_pos(start, now);
            let fill = ui.visuals().selection.bg_fill;
            ui.painter().rect(band, 0.0, fill.gamma_multiply(0.2), Stroke::new(1.0, fill), egui::StrokeKind::Inside);
        }
        if !ghosts.is_empty() || matches!(self.drag, Drag::Move { .. }) {
            ui.ctx().request_repaint();
        }
    }

    fn keyboard(&mut self, ui: &egui::Ui, editor: &mut Editor, mode: Mode) {
        let nudge = ui.input_mut(|i| {
            let step = if i.modifiers.shift { 2.0 } else { 1.0 };
            let mut d = Vec2::ZERO;
            for (k, v) in [
                (Key::ArrowUp, Vec2::Y),
                (Key::ArrowDown, -Vec2::Y),
                (Key::ArrowLeft, -Vec2::X),
                (Key::ArrowRight, Vec2::X),
            ] {
                if i.consume_key(egui::Modifiers::NONE, k) || i.consume_key(egui::Modifiers::SHIFT, k) {
                    d += v * step;
                }
            }
            d
        });
        if nudge != Vec2::ZERO {
            if editor.selection.is_empty() {
                self.camera.nudge(nudge * 4.0);
            } else {
                editor.move_selection(nudge.x, nudge.y, mode);
            }
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, editor: &mut Editor, mode: Mode) {
        let tile = self.context_tile.and_then(|id| editor.doc.tile(id).cloned());
        if let Some(t) = &tile {
            ui.label(egui::RichText::new(t.key.stem()).strong());
            if !editor.selection.contains(&t.id) {
                editor.selection = [t.id].into();
            }
            if ui.button("Place more of this tile").clicked() {
                editor.tool = Tool::Place { key: t.key.clone(), size: t.size, rotation: t.rotation };
                ui.close();
            }
            if ui.button("Select all of this tile").clicked() {
                editor.select_same(&t.key);
                ui.close();
            }
            if let Tool::Place { key, size, .. } = editor.tool.clone()
                && key != t.key
                && ui.button(format!("Replace with {}", key.stem())).clicked()
            {
                editor.replace_selection(&key, size, mode);
                ui.close();
            }
            ui.separator();
            if ui.button("Copy").clicked() {
                editor.copy();
                ui.close();
            }
            if ui.button("Delete").clicked() {
                editor.delete_selection();
                ui.close();
            }
        } else {
            let can_paste = editor.clipboard.is_some();
            if ui.add_enabled(can_paste, egui::Button::new("Paste here")).clicked() {
                if let Some(p) = self.context_point {
                    editor.paste_at(p.x.round(), p.y.round(), mode);
                }
                ui.close();
            }
            if ui.button("Select all").clicked() {
                editor.select_all();
                ui.close();
            }
        }
    }
}

fn wgpu_nearest() -> eframe::wgpu::FilterMode {
    eframe::wgpu::FilterMode::Nearest
}
