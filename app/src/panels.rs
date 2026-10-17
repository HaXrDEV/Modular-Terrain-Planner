//! The Library, Inspector and Problems panels.

use std::collections::BTreeSet;

use eframe::egui::{self, Color32, RichText};
use mtp_core::footprint::Turn;
use mtp_core::rules::Mode;
use mtp_core::tile::TileKey;
use mtp_core::units::{SourceUnit, TileSize, describe_cells};

use crate::editor::{Editor, Tool};
use crate::library::{FolderId, FolderStatus, Library};

/// A problem for the Problems panel instead of yet another message box.
#[derive(Clone, Debug)]
pub struct Problem {
    pub text: String,
}

#[derive(Default)]
pub struct LibraryPanel {
    pub search: String,
    /// Footprints to show, empty for all.
    pub sizes: BTreeSet<(u32, u32)>,
    collapsed: BTreeSet<FolderId>,
}

/// Requests from the panels that the app carries out.
#[derive(Debug, PartialEq)]
pub enum Request {
    AddFolder { recursive: bool },
    RemoveFolder(FolderId),
    ReloadFolder(FolderId),
    SetBattleMap,
    ClearBattleMap,
    GridSize,
    SampleTiles,
}

pub fn size_text(size: TileSize) -> String {
    let inches = |c: u32| {
        let i = c as f32 / 2.0;
        if i.fract() == 0.0 { format!("{i:.0}") } else { format!("{i:.1}") }
    };
    format!("{}×{} cells ({}\"×{}\")", size.w, size.h, inches(size.w), inches(size.h))
}

impl LibraryPanel {
    pub fn ui(&mut self, ui: &mut egui::Ui, library: &Library, editor: &mut Editor, thumb: f32) -> Vec<Request> {
        let mut requests = Vec::new();
        ui.horizontal(|ui| {
            if ui.button("Add folder…").on_hover_text("Load every STL file in a folder").clicked() {
                requests.push(Request::AddFolder { recursive: false });
            }
            if ui.button("With subfolders…").on_hover_text("Load a folder and every folder inside it").clicked() {
                requests.push(Request::AddFolder { recursive: true });
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.search).hint_text("Search all folders").desired_width(f32::INFINITY),
        )
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search tiles"));

        let mut footprints: BTreeSet<(u32, u32)> = BTreeSet::new();
        for t in library.tiles() {
            footprints.insert((t.size.w.min(t.size.h), t.size.w.max(t.size.h)));
        }
        if footprints.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if ui.selectable_label(self.sizes.is_empty(), "All").clicked() {
                    self.sizes.clear();
                }
                for fp in footprints.iter().take(12) {
                    let on = self.sizes.contains(fp);
                    if ui.selectable_label(on, format!("{}×{}", fp.0, fp.1)).clicked() {
                        if on {
                            self.sizes.remove(fp);
                        } else {
                            self.sizes.insert(*fp);
                        }
                    }
                }
            });
        }
        if !editor.recent.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Recent").weak()).on_hover_text("Keys 1 to 9 pick these");
                for (i, (key, size)) in editor.recent.clone().into_iter().enumerate() {
                    let on = matches!(&editor.tool, Tool::Place { key: k, .. } if *k == key);
                    let thumb = library.tile(&key).and_then(|t| t.thumbnail);
                    let label = format!("{}", i + 1);
                    let resp = match thumb {
                        Some(tex) => ui.add(
                            egui::Button::image_and_text(egui::Image::new((tex, egui::vec2(24.0, 24.0))), label)
                                .selected(on),
                        ),
                        None => ui.add(egui::Button::new(label).selected(on)),
                    };
                    if resp.on_hover_text(key.stem()).clicked() {
                        editor.tool = Tool::Place { key, size, rotation: Default::default() };
                    }
                }
            });
        }
        ui.separator();

        if library.folders().is_empty() {
            ui.add_space(12.0);
            ui.label("No tiles yet.");
            ui.label(RichText::new("Add a folder of STL files, or drop one onto the window.").weak());
            ui.add_space(8.0);
            if ui.button("Try the sample tiles").on_hover_text("A few stone floors, brick walls and a pillar").clicked()
            {
                requests.push(Request::SampleTiles);
            }
            return requests;
        }

        let query = self.search.to_lowercase();
        let chosen = match &editor.tool {
            Tool::Place { key, .. } => Some(key.clone()),
            _ => None,
        };
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for folder in library.folders() {
                let open = !self.collapsed.contains(&folder.id);
                let header = match &folder.status {
                    FolderStatus::Scanning => format!("{} (looking…)", folder.name),
                    FolderStatus::Loading { done, total } => format!("{} ({done} of {total})", folder.name),
                    FolderStatus::Ready => format!("{} ({})", folder.name, folder.tiles.len()),
                    FolderStatus::Unavailable(_) => format!("{} (not found)", folder.name),
                };
                let resp = egui::CollapsingHeader::new(header).id_salt(folder.id).open(Some(open)).show(ui, |ui| {
                    if let FolderStatus::Unavailable(reason) = &folder.status {
                        ui.label(RichText::new(reason).color(ui.visuals().warn_fg_color));
                        ui.label(RichText::new(&folder.path).weak().small());
                        ui.horizontal(|ui| {
                            if ui.button("Try again").clicked() {
                                requests.push(Request::ReloadFolder(folder.id));
                            }
                            if ui.button("Remove").clicked() {
                                requests.push(Request::RemoveFolder(folder.id));
                            }
                        });
                        return;
                    }
                    if !folder.skipped.is_empty() {
                        ui.label(
                            RichText::new(format!("{} files couldn't be read (see Problems)", folder.skipped.len()))
                                .color(ui.visuals().warn_fg_color)
                                .small(),
                        );
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        for key in &folder.tiles {
                            let Some(t) = library.tile(key) else { continue };
                            let fp = (t.size.w.min(t.size.h), t.size.w.max(t.size.h));
                            if !self.sizes.is_empty() && !self.sizes.contains(&fp) {
                                continue;
                            }
                            if !query.is_empty()
                                && !t.name.to_lowercase().contains(&query)
                                && !t.relative.to_lowercase().contains(&query)
                            {
                                continue;
                            }
                            let selected = chosen.as_ref() == Some(key);
                            let resp = tile_button(ui, t.thumbnail, &t.name, t.color, selected, thumb);
                            let resp = resp.on_hover_ui(|ui| {
                                ui.label(RichText::new(&t.name).strong());
                                ui.label(size_text(t.size));
                                ui.label(format!("{:.1} mm high", t.size.height_mm()));
                                if t.unit == SourceUnit::Inches {
                                    ui.label("Drawn in inches, converted to millimetres");
                                }
                                if t.relative.contains('/') {
                                    ui.label(RichText::new(&t.relative).weak());
                                }
                            });
                            if resp.clicked() {
                                editor.tool = if selected {
                                    Tool::Select
                                } else {
                                    Tool::Place { key: key.clone(), size: t.size, rotation: Default::default() }
                                };
                            }
                        }
                    });
                });
                if resp.header_response.clicked() {
                    if open {
                        self.collapsed.insert(folder.id);
                    } else {
                        self.collapsed.remove(&folder.id);
                    }
                }
                resp.header_response.context_menu(|ui| {
                    if ui.button("Load again").clicked() {
                        requests.push(Request::ReloadFolder(folder.id));
                        ui.close();
                    }
                    if ui.button("Remove from library…").clicked() {
                        requests.push(Request::RemoveFolder(folder.id));
                        ui.close();
                    }
                });
            }
        });
        requests
    }
}

fn tile_button(
    ui: &mut egui::Ui,
    thumbnail: Option<egui::TextureId>,
    name: &str,
    color: [u8; 3],
    selected: bool,
    size: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size + 18.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
    let visuals = ui.style().interact_selectable(&resp, selected);
    let image_rect = egui::Rect::from_min_size(rect.min, egui::vec2(size, size));
    ui.painter().rect_filled(image_rect, 4.0, ui.visuals().extreme_bg_color);
    match thumbnail {
        Some(tex) => {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex, image_rect.shrink(2.0), uv, Color32::WHITE);
        }
        None => {
            let c = Color32::from_rgb(color[0], color[1], color[2]);
            ui.painter().rect_filled(image_rect.shrink(size * 0.3), 3.0, c);
        }
    }
    if selected || resp.hovered() || resp.has_focus() {
        ui.painter().rect_stroke(
            image_rect,
            4.0,
            egui::Stroke::new(2.0, visuals.fg_stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        name.to_owned(),
        egui::FontId::proportional(11.0),
        ui.visuals().text_color(),
    );
    job.wrap =
        egui::text::TextWrapping {
            max_width: size, max_rows: 1, break_anywhere: true, overflow_character: Some('…')
        };
    job.halign = egui::Align::Center;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(egui::pos2(rect.center().x, image_rect.max.y + 2.0), galley, ui.visuals().text_color());
    resp
}

pub fn inspector(ui: &mut egui::Ui, editor: &mut Editor, library: &Library) -> Vec<Request> {
    let mut requests = Vec::new();
    match editor.tool.clone() {
        Tool::Place { key, size, rotation } => {
            ui.heading(key.stem());
            if let Some(t) = library.tile(&key) {
                if let Some(tex) = t.thumbnail {
                    ui.image((tex, egui::vec2(160.0, 160.0)));
                }
                ui.label(
                    RichText::new(format!("From {}", library.folder(t.folder).map_or("", |f| f.name.as_str()))).weak(),
                );
            }
            ui.label(size_text(size));
            ui.label(format!("{:.1} mm high", size.height_mm()));
            if let Some(t) = library.tile(&key) {
                ui.label(format!("About {:.1} cm³ of filament", t.volume_mm3 / 1000.0));
            }
            ui.label(format!("Turned {}°", rotation.degrees()));
            let placed = editor.doc.tiles().iter().filter(|t| t.key == key).count();
            ui.label(format!("{placed} placed"));
            ui.horizontal(|ui| {
                if ui.button("⟲ Turn").on_hover_text("Q").clicked() {
                    editor.turn(Turn::CounterClockwise, Mode::Snap);
                }
                if ui.button("Turn ⟳").on_hover_text("E").clicked() {
                    editor.turn(Turn::Clockwise, Mode::Snap);
                }
            });
            if !editor.selection.is_empty()
                && ui.button(format!("Replace selected tiles with {}", key.stem())).clicked()
            {
                editor.replace_selection(&key, size, Mode::Snap);
            }
            ui.separator();
        }
        Tool::Paste => {
            ui.heading("Pasting");
            ui.label("Click to place the copied tiles. Q and E turn them, Esc stops.");
            ui.separator();
        }
        _ => {}
    }

    let selected = editor.selected_tiles().into_iter().cloned().collect::<Vec<_>>();
    if !selected.is_empty() {
        ui.heading(if selected.len() == 1 {
            "Selected tile".to_owned()
        } else {
            format!("{} tiles selected", selected.len())
        });
        if let [t] = selected.as_slice() {
            ui.label(RichText::new(t.key.stem()).strong());
            ui.label(size_text(t.size));
            ui.label(format!("At {:.1}, {:.1} · turned {}°", t.x, t.y, t.rotation.degrees()));
            if t.z > 0.0 {
                ui.label(format!("Raised {:.1} mm", t.z * mtp_core::units::CELL_MM as f32));
            }
            if !t.size_known || library.tile(&t.key).is_none() {
                ui.label(
                    RichText::new("Its STL file isn't loaded: it's kept and saved as it is.")
                        .color(ui.visuals().warn_fg_color),
                );
                ui.label(RichText::new(t.key.as_str()).weak().small());
            }
        }
        ui.horizontal_wrapped(|ui| {
            if let [t] = selected.as_slice()
                && ui.button("Select all of this tile").clicked()
            {
                editor.select_same(&t.key);
            }
            if ui.button("Delete").clicked() {
                editor.delete_selection();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Move to level");
            let level = editor.level;
            if ui.small_button(format!("{}", level + 1)).on_hover_text("The level above").clicked() {
                editor.move_selection_to_level(level + 1, Mode::Snap);
            }
            if ui.small_button(format!("{}", level - 1)).on_hover_text("The level below").clicked() {
                editor.move_selection_to_level(level - 1, Mode::Snap);
            }
        });
        ui.separator();
    }

    ui.heading("Layout");
    let tiles = editor.doc.tiles();
    let files: BTreeSet<&TileKey> = tiles.iter().map(|t| &t.key).collect();
    ui.label(format!("{} tiles from {} STL files", tiles.len(), files.len()));
    if !tiles.is_empty() {
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for t in tiles {
            let b = t.bounds();
            lo = [lo[0].min(b.min[0]), lo[1].min(b.min[1])];
            hi = [hi[0].max(b.max[0]), hi[1].max(b.max[1])];
        }
        ui.label(format!("{} by {}", describe_cells(hi[0] - lo[0]), describe_cells(hi[1] - lo[1])));
    }
    let g = editor.doc.grid();
    ui.horizontal(|ui| {
        ui.label(format!("Grid {} × {} cells", g.cols, g.rows));
        if ui.small_button("Change…").clicked() {
            requests.push(Request::GridSize);
        }
    });

    ui.separator();
    ui.heading("Battle map");
    match editor.doc.ground_image().cloned() {
        None => {
            ui.label(RichText::new("An image under the grid, such as a map to build on.").weak());
            if ui.button("Set battle map…").clicked() {
                requests.push(Request::SetBattleMap);
            }
        }
        Some(mut img) => {
            let name = img.path.rsplit('/').next().unwrap_or(&img.path).to_owned();
            ui.label(name);
            let mut changed = false;
            egui::Grid::new("battle map").num_columns(2).show(ui, |ui| {
                for (label, i) in [("Left", 0), ("Bottom", 1), ("Width", 2), ("Depth", 3)] {
                    ui.label(label);
                    let range = if i < 2 { -2000.0..=4000.0 } else { 1.0..=4000.0 };
                    changed |= ui
                        .add(egui::DragValue::new(&mut img.rect[i]).speed(0.1).range(range).suffix(" cells"))
                        .changed();
                    ui.end_row();
                }
            });
            if changed {
                editor.doc.set_ground_image(Some(img), true);
                editor.changed();
            }
            ui.horizontal(|ui| {
                if ui.button("Change…").clicked() {
                    requests.push(Request::SetBattleMap);
                }
                if ui.button("Remove").clicked() {
                    requests.push(Request::ClearBattleMap);
                }
            });
        }
    }
    requests
}

/// What to print for the layout, after what the user owns.
pub fn print_list(
    ui: &mut egui::Ui,
    editor: &Editor,
    library: &Library,
    inventory: &mut crate::inventory::Inventory,
    density: f32,
) -> Vec<String> {
    let mut problems = Vec::new();
    if editor.doc.tiles().is_empty() {
        ui.label(RichText::new("Place some tiles to see what to print.").weak());
        return problems;
    }
    let info = crate::exports::tile_info(editor, library, inventory);
    let input = mtp_export::ExportInput { title: String::new(), state: editor.doc.state(), info: &info, level: None };
    let rows = mtp_export::print_list(&input);
    let grams = |mm3: f64| mm3 / 1000.0 * f64::from(density);
    let (mut needed, mut to_print, mut filament) = (0u32, 0u32, 0.0f64);
    let mut set = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(ui.available_height() - 70.0).show(ui, |ui| {
        egui::Grid::new("print list").num_columns(4).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("Tile").strong());
            ui.label(RichText::new("Need").strong());
            ui.label(RichText::new("Own").strong());
            ui.label(RichText::new("Print").strong());
            ui.end_row();
            for r in &rows {
                let owned = inventory.owned(&r.key);
                let left = r.count.saturating_sub(owned);
                needed += r.count;
                to_print += left;
                if let Some(v) = r.volume_mm3 {
                    filament += grams(v) * f64::from(left);
                }
                ui.label(&r.display_name).on_hover_text(format!("{}\n{}", r.folder, r.key));
                ui.label(r.count.to_string());
                let mut n = owned;
                if ui.add(egui::DragValue::new(&mut n).range(0..=9999)).changed() {
                    set.push((r.key.clone(), n));
                }
                ui.horizontal(|ui| {
                    if left == 0 {
                        ui.label(RichText::new("✔").color(ui.visuals().text_color()));
                    } else {
                        ui.label(left.to_string());
                        if ui.small_button("Printed").on_hover_text("Count these as owned").clicked() {
                            set.push((r.key.clone(), r.count));
                        }
                    }
                });
                ui.end_row();
            }
        });
    });
    for (key, n) in set {
        if let Err(e) = inventory.set(&key, n) {
            problems.push(e);
        }
    }
    ui.separator();
    ui.label(format!("{to_print} of {needed} tiles still to print."));
    if filament > 0.0 {
        ui.label(format!("About {filament:.0} g of filament if printed solid; infill uses less."))
            .on_hover_text(format!("At {density:.2} g/cm³, set under Settings"));
    }
    if to_print > 0 && ui.button("Mark everything as printed").clicked() {
        for r in &rows {
            if inventory.owned(&r.key) < r.count
                && let Err(e) = inventory.set(&r.key, r.count)
            {
                problems.push(e);
            }
        }
    }
    problems
}

pub fn problems(ui: &mut egui::Ui, problems: &mut Vec<Problem>) {
    if problems.is_empty() {
        ui.label(RichText::new("No problems.").weak());
        return;
    }
    if ui.button("Clear").clicked() {
        problems.clear();
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for p in problems.iter().rev() {
            ui.label(&p.text);
            ui.separator();
        }
    });
}
