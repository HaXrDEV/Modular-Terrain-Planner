//! The window: menus, docked panels, status bar and dialogs around the
//! viewport, plus everything that runs between frames (library loading,
//! thumbnails, autosave).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, RichText};
use eframe::{egui_wgpu, wgpu};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use mtp_core::document::Document;
use mtp_core::footprint::Turn;
use mtp_core::grid::GridSize;
use mtp_core::rules::Mode;
use mtp_core::tile::{PlacedId, TileKey};
use mtp_render::scene::TierSource;
use mtp_render::{DrawKind, DrawTile, Palette, Projection, Renderer, Target};

use crate::commands::{COMMANDS, Command, MOUSE, spec};
use crate::editor::{Editor, Tool};
use crate::exports::{self, ExportKind};
use crate::files::{self, Autosave, PendingOpen};
use crate::library::{self, FolderId, Library};
use crate::panels::{self, LibraryPanel, Problem, Request};
use crate::settings::{APP_NAME, Backend, Paths, SessionFolder, Settings, ThemeChoice, VERSION};
use crate::viewport::{ViewCtx, Viewport};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Library,
    View,
    Inspector,
    PrintList,
    Problems,
}

/// What to do once unsaved changes are dealt with.
#[derive(Clone, Debug)]
enum After {
    New,
    Open(Option<PathBuf>),
    Recover(Autosave),
    Quit,
}

enum Dialog {
    Unsaved(After),
    MissingFolders(PendingOpen),
    /// `from_left`: grow or shrink on the left and bottom instead of the
    /// right and top.
    GridSize {
        cols: u32,
        rows: u32,
        from_left: bool,
    },
    RemoveFolder {
        id: FolderId,
        in_use: Vec<PlacedId>,
    },
    Recovery(Vec<Autosave>),
    Crash(String),
    Message {
        title: String,
        text: String,
    },
}

/// Work a dialog hands back to the app once it closes.
type Then = Box<dyn FnOnce(&mut App)>;

pub struct App {
    paths: Paths,
    settings: Settings,
    render_state: egui_wgpu::RenderState,
    renderer: Renderer,
    library: Library,
    inventory: crate::inventory::Inventory,
    editor: Editor,
    viewport: Viewport,
    library_panel: LibraryPanel,
    dock: DockState<Tab>,
    problems: Vec<Problem>,
    dialog: Option<Dialog>,
    requests: Vec<Request>,
    thumbnails: VecDeque<TileKey>,
    thumb_target: Option<Target>,
    /// One thumbnail texture per tile; re-rendering replaces it in place.
    thumb_textures: std::collections::HashMap<TileKey, wgpu::Texture>,
    /// Whether the thumbnails were last drawn for a dark theme.
    thumbs_dark: Option<bool>,
    last_autosave: Instant,
    frames: u64,
    show_settings: bool,
    show_about: bool,
    show_shortcuts: bool,
    quitting: bool,
    title: String,
    applied: Option<(ThemeChoice, u32)>,
    screenshot: Option<(PathBuf, u64)>,
    /// The battle map the renderer was last given, to follow undo and redo.
    image_shown: Option<String>,
    /// Tells this window's autosave file apart from other windows' and
    /// earlier sessions'.
    session: String,
    /// The autosave this window wrote, the only one it may delete.
    own_autosave: Option<PathBuf>,
    /// Files given at startup, opened once the recovery question is answered.
    deferred_open: Vec<PathBuf>,
    incoming: Option<crossbeam_channel::Receiver<Vec<PathBuf>>>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: Paths,
        settings: Settings,
        startup: Startup,
    ) -> Result<Self, String> {
        let render_state = cc.wgpu_render_state.clone().ok_or("The app was started without its graphics renderer.")?;
        let renderer = Renderer::new(render_state.device.clone(), render_state.queue.clone());
        let ctx = cc.egui_ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let library = Library::new(Some(paths.mesh_cache()), u64::from(settings.mesh_cache_mb) << 20, repaint);

        let mut dock = DockState::new(vec![Tab::View]);
        let tree = dock.main_surface_mut();
        let [view, _] = tree.split_left(NodeIndex::root(), 0.22, vec![Tab::Library]);
        tree.split_right(view, 0.78, vec![Tab::Inspector, Tab::PrintList, Tab::Problems]);

        cc.egui_ctx.global_style_mut(|s| {
            use egui::{FontFamily, FontId, TextStyle};
            s.text_styles.insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
            s.text_styles.insert(TextStyle::Button, FontId::new(14.0, FontFamily::Proportional));
            s.text_styles.insert(TextStyle::Small, FontId::new(12.0, FontFamily::Proportional));
            s.text_styles.insert(TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional));
        });

        let (inventory, inventory_problem) = crate::inventory::Inventory::load(&paths.data);
        let projection = if settings.start_in_3d { Projection::Perspective } else { Projection::TopDown };
        let grid = GridSize::DEFAULT;
        let mut app = Self {
            viewport: Viewport::new(grid.cols, grid.rows, projection),
            editor: Editor::empty(grid),
            paths,
            render_state,
            renderer,
            library,
            inventory,
            library_panel: LibraryPanel::default(),
            dock,
            problems: Vec::new(),
            dialog: None,
            requests: Vec::new(),
            thumbnails: VecDeque::new(),
            thumb_target: None,
            thumb_textures: std::collections::HashMap::new(),
            thumbs_dark: None,
            last_autosave: Instant::now(),
            frames: 0,
            show_settings: false,
            show_about: false,
            show_shortcuts: false,
            quitting: false,
            title: String::new(),
            applied: None,
            screenshot: startup.screenshot.clone().map(|p| (p, 0)),
            session: {
                let nanos =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
                format!("{:x}{:05x}", std::process::id(), nanos & 0xfffff)
            },
            own_autosave: None,
            image_shown: None,
            deferred_open: Vec::new(),
            incoming: startup.incoming.as_ref().map(|i| {
                let _ = i.ctx.set(cc.egui_ctx.clone());
                i.rx.clone()
            }),
            settings,
        };
        for dir in startup.open.iter().filter(|p| p.is_dir()) {
            app.library.add_folder(dir, true);
        }
        app.editor.level_view = app.settings.level_view;
        if let Some(p) = inventory_problem {
            app.problem(p);
        }
        for problem in startup.problems {
            app.problem(problem);
        }
        for f in app.settings.session_folders.clone() {
            app.library.add_folder(Path::new(&f.path), f.recursive);
        }
        let autosaves = files::autosaves(&app.paths.autosave());
        if let Some(report) = startup.crash_report {
            app.dialog = Some(Dialog::Crash(report));
        } else if !autosaves.is_empty() {
            app.dialog = Some(Dialog::Recovery(autosaves));
        }
        // A project given at startup waits for the recovery question, so
        // opening it can't replace the work being offered back
        let projects: Vec<PathBuf> =
            startup.open.into_iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mtp"))).collect();
        if app.dialog.is_some() {
            app.deferred_open = projects;
        } else if let Some(path) = projects.first() {
            app.open_path(path);
        }
        Ok(app)
    }

    fn problem(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::warn!("{text}");
        self.problems.push(Problem { text });
    }

    // ---- Files ------------------------------------------------------------

    fn guard(&mut self, after: After) {
        if self.editor.doc.is_dirty() {
            self.dialog = Some(Dialog::Unsaved(after));
        } else {
            self.proceed(after);
        }
    }

    fn proceed(&mut self, after: After) {
        match after {
            After::New => {
                self.replace_editor(Editor::empty(self.editor.doc.grid()));
                self.viewport.reset_view(self.editor.doc.grid().cols, self.editor.doc.grid().rows);
            }
            After::Open(Some(path)) => self.open_path(&path),
            After::Open(None) => {
                let picked = rfd::FileDialog::new().add_filter("Modular Terrain Planner project", &["mtp"]).pick_file();
                if let Some(path) = picked {
                    self.open_path(&path);
                }
            }
            After::Recover(a) => match files::read_project(&a.file) {
                Ok(mut pending) => {
                    // Missing folders are kept as saved; the recovered tiles stay as they were
                    pending.missing.clear();
                    self.finish_open(pending);
                    // The autosave file is no place to keep working in
                    let file = a.file.to_string_lossy().into_owned();
                    self.settings.recent_projects.retain(|p| *p != file);
                    if let Some(original) = &a.original {
                        self.settings.add_recent_project(&original.to_string_lossy());
                    }
                    self.editor.path = a.original.clone();
                    // The recovered state isn't saved anywhere yet
                    self.editor.doc.mark_unsaved();
                    files::discard_autosave(&a);
                    self.editor.tell("Recovered your unsaved work. Save it to keep it.");
                }
                Err(e) => self.dialog = Some(Dialog::Message { title: "Couldn't recover".into(), text: e }),
            },
            After::Quit => {
                self.quitting = true;
            }
        }
    }

    fn replace_editor(&mut self, editor: Editor) {
        let view = self.editor.level_view;
        self.editor = editor;
        self.editor.level_view = view;
        self.editor.changed();
        self.discard_own_autosave();
        self.load_ground_image();
    }

    /// Deletes this window's autosave; never another window's, nor ones
    /// still waiting to be recovered.
    fn discard_own_autosave(&mut self) {
        if let Some(file) = self.own_autosave.take() {
            files::discard_autosave_file(&file);
        }
    }

    fn open_path(&mut self, path: &Path) {
        match files::read_project(path) {
            Ok(pending) if pending.missing.is_empty() => self.finish_open(pending),
            Ok(pending) => self.dialog = Some(Dialog::MissingFolders(pending)),
            Err(e) => self.dialog = Some(Dialog::Message { title: "Couldn't open the project".into(), text: e }),
        }
    }

    fn finish_open(&mut self, pending: PendingOpen) {
        tracing::info!("opening {}", pending.path.display());
        let state = files::finish_open(&pending);
        for w in &pending.file.warnings {
            self.problem(format!("{}: {w}", pending.path.display()));
        }
        for i in &pending.missing {
            if pending.found[*i].is_none() {
                let f = &pending.file.folders[*i];
                self.problem(format!(
                    "The tile folder {} wasn't found. Its tiles are kept, drawn as grey boxes, and saved as they were.",
                    f.absolute.as_deref().unwrap_or(&f.path)
                ));
            }
        }
        let mut doc = Document::new(state);
        for (path, recursive) in files::folders_to_load(&pending) {
            self.library.add_folder(Path::new(&path), recursive);
        }
        // Tiles already in the library tell the document their real sizes
        let keys: HashSet<TileKey> = doc.tiles().iter().map(|t| t.key.clone()).collect();
        for key in keys {
            if let Some(t) = self.library.tile(&key) {
                doc.learn_size(&key, t.size);
            }
        }
        let grid = doc.grid();
        self.settings.add_recent_project(&pending.path.to_string_lossy());
        self.replace_editor(Editor::new(doc, Some(pending.path.clone())));
        self.viewport.reset_view(grid.cols, grid.rows);
        self.viewport.frame(&self.editor);
        self.editor.tell(format!("Opened {}.", self.editor.title()));
    }

    fn save(&mut self, choose: bool) -> bool {
        let path = match (&self.editor.path, choose) {
            (Some(p), false) => p.clone(),
            _ => {
                let picked = rfd::FileDialog::new()
                    .add_filter("Modular Terrain Planner project", &["mtp"])
                    .set_file_name(format!("{}.mtp", self.editor.title()))
                    .save_file();
                match picked {
                    Some(p) => files::with_extension(p),
                    None => return false,
                }
            }
        };
        match files::save_project(&self.editor.doc, &self.library, &path) {
            Ok(()) => {
                self.editor.doc.mark_saved();
                self.editor.path = Some(path.clone());
                self.settings.add_recent_project(&path.to_string_lossy());
                self.discard_own_autosave();
                self.editor.tell(format!("Saved {}.", path.display()));
                true
            }
            Err(e) => {
                self.dialog = Some(Dialog::Message { title: "Couldn't save".into(), text: e });
                false
            }
        }
    }

    fn autosave_tick(&mut self) {
        let every = Duration::from_secs(u64::from(self.settings.autosave_minutes) * 60);
        if every.is_zero() || self.last_autosave.elapsed() < every {
            return;
        }
        self.last_autosave = Instant::now();
        if self.editor.doc.is_dirty() {
            let dir = self.paths.autosave();
            match files::autosave(&dir, &self.session, &self.editor.doc, &self.library, self.editor.path.as_deref()) {
                Ok(p) => {
                    tracing::info!("autosaved to {}", p.display());
                    self.own_autosave = Some(p);
                }
                Err(e) => self.problem(format!("Autosave failed: {e}")),
            }
        }
    }

    fn set_battle_map(&mut self) {
        let picked =
            rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "gif"]).pick_file();
        if let Some(path) = picked {
            self.set_battle_map_from(&path);
        }
    }

    fn set_battle_map_from(&mut self, path: &Path) {
        match files::decode_image(path, self.renderer.max_texture_side()) {
            Ok(img) => {
                let g = self.editor.doc.grid();
                let rect = files::fit_to_grid(g.cols, g.rows, img.width, img.height);
                self.renderer.set_ground_image(Some((img.width, img.height, &img.rgba)));
                self.editor.doc.set_ground_image(Some(files::ground_image(path, rect)), false);
                self.image_shown = self.editor.doc.ground_image().map(|i| i.path.clone());
                self.editor.changed();
            }
            Err(e) => self.dialog = Some(Dialog::Message { title: "Couldn't use that image".into(), text: e }),
        }
    }

    /// Loads the document's battle map into the renderer (or clears it).
    fn load_ground_image(&mut self) {
        self.image_shown = self.editor.doc.ground_image().map(|i| i.path.clone());
        let Some(img) = self.editor.doc.ground_image().cloned() else {
            self.renderer.set_ground_image(None);
            return;
        };
        match files::decode_image(Path::new(&img.path), self.renderer.max_texture_side()) {
            Ok(d) => {
                self.renderer.set_ground_image(Some((d.width, d.height, &d.rgba)));
                if img.rect[2] <= 0.0 || img.rect[3] <= 0.0 {
                    let g = self.editor.doc.grid();
                    let rect = files::fit_to_grid(g.cols, g.rows, d.width, d.height);
                    self.editor.doc.set_ground_image(Some(files::ground_image(Path::new(&img.path), rect)), false);
                }
            }
            Err(e) => {
                self.renderer.set_ground_image(None);
                self.problem(format!("The battle map isn't shown: {e} It stays in the project."));
            }
        }
    }

    fn add_folder(&mut self, recursive: bool) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            self.library.add_folder(&dir, recursive);
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        self.open_paths(dropped);
        let sent: Vec<Vec<PathBuf>> = self.incoming.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for paths in sent {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.open_paths(paths);
        }
    }

    /// Opens what was dropped or sent by another launch: folders and STL
    /// files go to the library, projects open, images become the battle map.
    fn open_paths(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if path.is_dir() {
                self.library.add_folder(&path, false);
            } else if ext == "mtp" {
                self.guard(After::Open(Some(path)));
            } else if ext == "stl" {
                if let Some(dir) = path.parent() {
                    self.library.add_folder(dir, false);
                }
            } else if ["png", "jpg", "jpeg", "webp", "bmp", "gif"].contains(&ext.as_str()) {
                self.set_battle_map_from(&path);
            } else {
                self.editor.warn(format!("{} isn't a folder, project, STL or image.", path.display()));
            }
        }
    }

    // ---- Commands ---------------------------------------------------------

    fn run(&mut self, ctx: &egui::Context, command: Command) {
        let mode = if ctx.input(|i| i.modifiers.command) { Mode::Free } else { Mode::Snap };
        match command {
            Command::New => self.guard(After::New),
            Command::Open => self.guard(After::Open(None)),
            Command::Save => {
                self.save(false);
            }
            Command::SaveAs => {
                self.save(true);
            }
            Command::AddFolder => self.add_folder(false),
            Command::AddFolderWithSubfolders => self.add_folder(true),
            Command::ExportPdf => self.export(ExportKind::Pdf),
            Command::ExportPng => self.export(ExportKind::Png),
            Command::ExportCsv => self.export(ExportKind::Csv),
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Command::Undo => self.editor.undo(),
            Command::Redo => self.editor.redo(),
            Command::Cut => self.editor.cut(),
            Command::Copy => {
                self.editor.copy();
            }
            Command::Paste => {
                if self.editor.clipboard.is_some() {
                    self.editor.tool = Tool::Paste;
                }
            }
            Command::Delete => self.editor.delete_selection(),
            Command::SelectAll => self.editor.select_all(),
            Command::SelectSame => {
                if let Some(key) = self.editor.selected_tiles().first().map(|t| t.key.clone()) {
                    self.editor.select_same(&key);
                }
            }
            Command::TurnLeft => self.editor.turn(Turn::CounterClockwise, mode),
            Command::TurnRight => self.editor.turn(Turn::Clockwise, mode),
            Command::ToolSelect => self.editor.tool = Tool::Select,
            Command::ToolErase => self.editor.tool = Tool::Erase,
            Command::Escape => {
                if self.editor.tool != Tool::Select {
                    self.editor.tool = Tool::Select;
                } else {
                    self.editor.selection.clear();
                }
            }
            Command::GridSize => {
                let g = self.editor.doc.grid();
                self.dialog = Some(Dialog::GridSize { cols: g.cols, rows: g.rows, from_left: false });
            }
            Command::SetBattleMap => self.set_battle_map(),
            Command::ClearBattleMap => {
                self.editor.doc.set_ground_image(None, false);
                self.editor.changed();
                self.renderer.set_ground_image(None);
            }
            Command::Toggle3d => self.viewport.toggle_projection(),
            Command::Frame => self.viewport.frame(&self.editor),
            Command::ResetView => {
                let g = self.editor.doc.grid();
                self.viewport.reset_view(g.cols, g.rows);
            }
            Command::ToggleGrid => self.viewport.show_grid = !self.viewport.show_grid,
            Command::Settings => self.show_settings = true,
            Command::Shortcuts => self.show_shortcuts = true,
            Command::About => self.show_about = true,
            Command::OpenLogFolder => open_in_file_manager(&self.paths.logs()),
        }
    }

    fn export(&mut self, kind: ExportKind) {
        if self.editor.doc.tiles().is_empty() {
            self.dialog =
                Some(Dialog::Message { title: "Nothing to export".into(), text: "Place some tiles first.".into() });
            return;
        }
        match exports::export(
            kind,
            &self.editor,
            &self.library,
            &self.inventory,
            &self.settings.paper,
            self.settings.csv_full_paths,
        ) {
            Ok(Some(path)) => {
                self.editor.tell(format!("Exported {}.", path.display()));
                open_in_file_manager(&path);
            }
            Ok(None) => {}
            Err(e) => self.dialog = Some(Dialog::Message { title: "Couldn't export".into(), text: e }),
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Open menus and drop-downs keep their keys (Esc closes them)
        if ctx.egui_wants_keyboard_input() || self.dialog.is_some() || egui::Popup::is_any_open(ctx) {
            return;
        }
        // egui turns Ctrl+C, Ctrl+X and Ctrl+V into clipboard events rather
        // than key presses, so consume_shortcut never sees them
        let clipboard: Vec<Command> = ctx.input_mut(|i| {
            let mut commands = Vec::new();
            i.events.retain(|e| match e {
                egui::Event::Copy => {
                    commands.push(Command::Copy);
                    false
                }
                egui::Event::Cut => {
                    commands.push(Command::Cut);
                    false
                }
                egui::Event::Paste(_) => {
                    commands.push(Command::Paste);
                    false
                }
                _ => true,
            });
            commands
        });
        for command in clipboard {
            if matches!(command, Command::Copy | Command::Cut) {
                // Ctrl+V only produces a paste event when the system clipboard holds text
                ctx.copy_text("Modular Terrain Planner tiles".to_owned());
            }
            self.run(ctx, command);
        }
        // Shortcuts with more modifiers first, so Ctrl+Shift+Z isn't taken as Ctrl+Z
        let mut specs: Vec<_> = COMMANDS.iter().filter(|s| s.shortcut.is_some()).collect();
        specs.sort_by_key(|s| s.shortcut.map_or(0, |k| u8::from(k.modifiers.shift) + u8::from(k.modifiers.command)));
        specs.reverse();
        for s in specs {
            if let Some(shortcut) = s.shortcut
                && ctx.input_mut(|i| i.consume_shortcut(&shortcut))
            {
                self.run(ctx, s.command);
            }
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Y)) {
            self.editor.redo();
        }
        // 1 to 9 pick a recently used tile
        let digits =
            [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (i, k) in digits.into_iter().enumerate() {
            if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, k))
                && let Some((key, size)) = self.editor.recent.get(i).cloned()
            {
                let rotation = match &self.editor.tool {
                    Tool::Place { rotation, .. } => *rotation,
                    _ => Default::default(),
                };
                self.editor.tool = Tool::Place { key, size, rotation };
            }
        }
    }

    // ---- Background work --------------------------------------------------

    fn library_events(&mut self) {
        for event in self.library.poll() {
            match event {
                library::Event::TileLoaded(key) => {
                    let Some(t) = self.library.tile(&key) else { continue };
                    let (size, tiers) = (t.size, t.tiers());
                    self.renderer.register_mesh(key.clone(), tiers);
                    if self.editor.doc.learn_size(&key, size) {
                        self.editor.changed();
                    }
                    self.queue_thumbnail(key);
                }
                library::Event::FolderFinished { name, loaded, skipped } => {
                    for s in &skipped {
                        self.problem(format!("Skipped {s}"));
                    }
                    let note = if skipped.is_empty() { String::new() } else { format!(", {} skipped", skipped.len()) };
                    self.editor.tell(format!("Loaded {loaded} tiles from {name}{note}."));
                }
                library::Event::FolderUnavailable { path, message } => {
                    self.problem(format!("{path}: {message} It stays in the library until removed."));
                }
            }
        }
    }

    /// Renders a few library thumbnails per frame, each into its own small texture.
    fn queue_thumbnail(&mut self, key: TileKey) {
        if !self.thumbnails.contains(&key) {
            self.thumbnails.push_back(key);
        }
    }

    fn make_thumbnails(&mut self, ctx: &egui::Context) {
        // Thumbnails are drawn on the theme's background, so they follow it
        let dark = ctx.global_style().visuals.dark_mode;
        if self.thumbs_dark.replace(dark).is_some_and(|was| was != dark) {
            let keys: Vec<TileKey> = self.library.tiles().map(|t| t.key.clone()).collect();
            for key in keys {
                self.queue_thumbnail(key);
            }
        }
        let px = (self.settings.thumbnail_size * ctx.pixels_per_point()).round().clamp(48.0, 384.0) as u32;
        let palette = if ctx.global_style().visuals.dark_mode { Palette::DARK } else { Palette::LIGHT };
        for _ in 0..6 {
            let Some(key) = self.thumbnails.pop_front() else { break };
            let Some(t) = self.library.tile(&key) else { continue };
            let tier = t.preview_tier();
            let lod = &t.mesh.lods[tier];
            if !self.renderer.resident(&key, tier) {
                self.renderer.upload_tier(&key, tier, &lod.positions, &lod.indices);
            }
            let tile = DrawTile {
                key: key.clone(),
                x: 0.0,
                y: 0.0,
                z: 0.0,
                size: t.size,
                rotation: Default::default(),
                color: t.color,
                selected: false,
                kind: DrawKind::Tile,
            };
            if self.thumb_target.as_ref().is_none_or(|tt| tt.size != [px, px]) {
                self.thumb_target = Some(self.renderer.create_target([px, px]));
            }
            let Some(target) = self.thumb_target.as_mut() else { break };
            self.renderer.render_thumbnail(target, &tile, palette);
            let texture = self.render_state.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("thumbnail"),
                size: wgpu::Extent3d { width: px, height: px, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: mtp_render::renderer::COLOR_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let mut encoder = self.render_state.device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_texture(
                target.output.as_image_copy(),
                texture.as_image_copy(),
                wgpu::Extent3d { width: px, height: px, depth_or_array_layers: 1 },
            );
            self.render_state.queue.submit([encoder.finish()]);
            let view = texture.create_view(&Default::default());
            let existing = self.library.tile(&key).and_then(|t| t.thumbnail);
            let mut egui_renderer = self.render_state.renderer.write();
            let id = match existing {
                Some(id) => {
                    egui_renderer.update_egui_texture_from_wgpu_texture(
                        &self.render_state.device,
                        &view,
                        wgpu::FilterMode::Linear,
                        id,
                    );
                    id
                }
                None => {
                    egui_renderer.register_native_texture(&self.render_state.device, &view, wgpu::FilterMode::Linear)
                }
            };
            drop(egui_renderer);
            self.thumb_textures.insert(key.clone(), texture);
            if let Some(t) = self.library.tile_mut(&key) {
                t.thumbnail = Some(id);
            }
            ctx.request_repaint();
        }
    }

    /// Frees GPU memory of tiles no longer placed (they keep their thumbnails).
    fn evict_unused(&mut self) {
        let mut keep: HashSet<TileKey> = self.editor.doc.tiles().iter().map(|t| t.key.clone()).collect();
        if let Tool::Place { key, .. } = &self.editor.tool {
            keep.insert(key.clone());
        }
        if let Some(c) = &self.editor.clipboard {
            keep.extend(c.items.iter().map(|p| p.key.clone()));
        }
        keep.extend(self.thumbnails.iter().cloned());
        let unused: Vec<(TileKey, Vec<u32>)> = self
            .renderer
            .registered()
            .filter(|k| !keep.contains(*k))
            .filter(|k| {
                let n = self.renderer.tiers(k).map_or(0, <[u32]>::len);
                (0..n).any(|i| self.renderer.resident(k, i))
            })
            .map(|k| (k.clone(), self.renderer.tiers(k).map(<[u32]>::to_vec).unwrap_or_default()))
            .collect();
        for (key, tiers) in unused {
            self.renderer.register_mesh(key, tiers);
        }
    }

    fn apply_settings(&mut self, ctx: &egui::Context) {
        let wanted = (self.settings.theme, (self.settings.ui_scale * 100.0).round() as u32);
        if self.applied == Some(wanted) {
            return;
        }
        self.applied = Some(wanted);
        ctx.set_theme(match self.settings.theme {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        });
        ctx.set_zoom_factor(self.settings.ui_scale);
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let star = if self.editor.doc.is_dirty() { "*" } else { "" };
        let title = format!("{}{star} - {APP_NAME}", self.editor.title());
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn remember_session(&mut self) {
        self.settings.level_view = self.editor.level_view;
        self.settings.session_folders = self
            .library
            .folders()
            .iter()
            .map(|f| SessionFolder { path: f.path.clone(), recursive: f.recursive })
            .collect();
        if let Err(e) = self.settings.save(&self.paths) {
            tracing::warn!("couldn't save settings: {e}");
        }
    }

    // ---- UI -----------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut chosen = None;
        egui::MenuBar::new().ui(ui, |ui| {
            let item = |ui: &mut egui::Ui, command: Command, enabled: bool, chosen: &mut Option<Command>| {
                let s = spec(command);
                let mut button = egui::Button::new(s.label);
                if let Some(k) = s.shortcut {
                    button = button.shortcut_text(ctx.format_shortcut(&k));
                }
                if ui.add_enabled(enabled, button).clicked() {
                    *chosen = Some(command);
                    ui.close();
                }
            };
            ui.menu_button("File", |ui| {
                item(ui, Command::New, true, &mut chosen);
                item(ui, Command::Open, true, &mut chosen);
                ui.menu_button("Open recent", |ui| {
                    let recent: Vec<String> = self.settings.recent_projects.clone();
                    if recent.is_empty() {
                        ui.label(RichText::new("Nothing yet").weak());
                    }
                    for p in recent {
                        let exists = Path::new(&p).is_file();
                        if ui.add_enabled(exists, egui::Button::new(&p)).clicked() {
                            self.guard(After::Open(Some(PathBuf::from(p))));
                            ui.close();
                        }
                    }
                });
                item(ui, Command::Save, true, &mut chosen);
                item(ui, Command::SaveAs, true, &mut chosen);
                ui.separator();
                item(ui, Command::AddFolder, true, &mut chosen);
                item(ui, Command::AddFolderWithSubfolders, true, &mut chosen);
                ui.separator();
                let any = !self.editor.doc.tiles().is_empty();
                item(ui, Command::ExportPdf, any, &mut chosen);
                item(ui, Command::ExportPng, any, &mut chosen);
                item(ui, Command::ExportCsv, any, &mut chosen);
                ui.separator();
                item(ui, Command::Quit, true, &mut chosen);
            });
            ui.menu_button("Edit", |ui| {
                let undo = self.editor.doc.undo_label().map(|l| format!("Undo {}", l.to_lowercase()));
                let redo = self.editor.doc.redo_label().map(|l| format!("Redo {}", l.to_lowercase()));
                let s = spec(Command::Undo);
                if ui
                    .add_enabled(
                        undo.is_some(),
                        egui::Button::new(undo.unwrap_or_else(|| "Undo".into())).shortcut_text(ctx.format_shortcut(
                            &s.shortcut.unwrap_or(egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::Z)),
                        )),
                    )
                    .clicked()
                {
                    chosen = Some(Command::Undo);
                    ui.close();
                }
                if ui.add_enabled(redo.is_some(), egui::Button::new(redo.unwrap_or_else(|| "Redo".into()))).clicked() {
                    chosen = Some(Command::Redo);
                    ui.close();
                }
                ui.separator();
                let sel = !self.editor.selection.is_empty();
                item(ui, Command::Cut, sel, &mut chosen);
                item(ui, Command::Copy, sel, &mut chosen);
                item(ui, Command::Paste, self.editor.clipboard.is_some(), &mut chosen);
                item(ui, Command::Delete, sel, &mut chosen);
                item(ui, Command::SelectAll, true, &mut chosen);
                item(ui, Command::SelectSame, sel, &mut chosen);
                ui.separator();
                item(ui, Command::TurnLeft, true, &mut chosen);
                item(ui, Command::TurnRight, true, &mut chosen);
                ui.separator();
                item(ui, Command::GridSize, true, &mut chosen);
                item(ui, Command::SetBattleMap, true, &mut chosen);
                item(ui, Command::ClearBattleMap, self.editor.doc.ground_image().is_some(), &mut chosen);
            });
            ui.menu_button("View", |ui| {
                item(ui, Command::Toggle3d, true, &mut chosen);
                item(ui, Command::Frame, true, &mut chosen);
                item(ui, Command::ResetView, true, &mut chosen);
                item(ui, Command::ToggleGrid, true, &mut chosen);
                ui.separator();
                item(ui, Command::Settings, true, &mut chosen);
            });
            ui.menu_button("Help", |ui| {
                item(ui, Command::Shortcuts, true, &mut chosen);
                item(ui, Command::OpenLogFolder, true, &mut chosen);
                item(ui, Command::About, true, &mut chosen);
            });
            ui.separator();
            for (tool, label, command) in
                [(Tool::Select, "Select", Command::ToolSelect), (Tool::Erase, "Erase", Command::ToolErase)]
            {
                let on = self.editor.tool == tool;
                let s = spec(command);
                let tip = s.shortcut.map(|k| ctx.format_shortcut(&k)).unwrap_or_default();
                if ui.selectable_label(on, label).on_hover_text(tip).clicked() {
                    chosen = Some(command);
                }
            }
            let three_d = self.viewport.camera.projection == Projection::Perspective;
            if ui
                .selectable_label(three_d, if three_d { "3D" } else { "2D" })
                .on_hover_text("Switch 2D / 3D (T)")
                .clicked()
            {
                chosen = Some(Command::Toggle3d);
            }
        });
        if let Some(c) = chosen {
            self.run(&ctx, c);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let tool = self.editor.tool.name();
            let hint = match &self.editor.tool {
                Tool::Select => "Click to select · drag to move · Shift adds · Alt+click picks up a tile",
                Tool::Place { .. } => {
                    "Click or drag to place · Shift+drag fills · Shift+click draws a line · Q/E turn · Ctrl places freely · Esc stops"
                }
                Tool::Erase => "Click or drag over tiles to remove them · Esc stops",
                Tool::Paste => "Click to paste · Q/E turn · Esc stops",
            };
            ui.label(RichText::new(tool).strong());
            ui.label(RichText::new(format!("Level {}", self.editor.level)).weak());
            let notice = self.editor.notice.as_ref().filter(|n| n.at.elapsed() < Duration::from_secs(6));
            if let Some(r) = &self.viewport.refusal {
                ui.label(RichText::new(r.to_string()).color(ui.visuals().warn_fg_color));
            } else if let Some(n) = notice {
                let color = if n.warning { ui.visuals().warn_fg_color } else { ui.visuals().text_color() };
                ui.label(RichText::new(&n.text).color(color));
            } else {
                ui.label(RichText::new(hint).weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = self.problems.len();
                if n > 0
                    && ui
                        .button(RichText::new(format!("⚠ {n}")).color(ui.visuals().warn_fg_color))
                        .on_hover_text("Show problems")
                        .clicked()
                    && let Some(tab) = self.dock.find_tab(&Tab::Problems)
                {
                    let _ = self.dock.set_active_tab(tab);
                }
                if let Some((done, total, name)) = self.library.progress() {
                    if ui.small_button("Cancel").clicked() {
                        self.library.cancel_all();
                    }
                    ui.label(format!("Loading {name}: {done} of {total}"));
                    ui.spinner();
                }
                if self.settings.show_stats {
                    let s = &self.viewport.stats;
                    ui.label(
                        RichText::new(format!(
                            "{:.1} M triangles · {} draws · {:.1} ms · {} MB",
                            s.triangles as f64 / 1e6,
                            s.draw_calls,
                            s.cull_ms + s.encode_ms,
                            self.renderer.gpu_bytes() >> 20
                        ))
                        .weak()
                        .monospace(),
                    );
                }
                if let Some(h) = self.viewport.hover {
                    ui.label(format!(
                        "{:.0}, {:.0} ({:.1}\", {:.1}\")",
                        h.point.x.floor(),
                        h.point.y.floor(),
                        h.point.x / 2.0,
                        h.point.y / 2.0
                    ));
                }
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let mut then: Option<Then> = None;
        let dialog = match dialog {
            Dialog::Unsaved(after) => {
                let title = self.editor.title();
                egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
                    ui.heading(format!("Save changes to {title}?"));
                    ui.label("Your changes are lost if you don't save them.");
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            let after = after.clone();
                            then = Some(Box::new(move |app: &mut Self| {
                                if app.save(false) {
                                    app.proceed(after);
                                }
                            }));
                            keep = false;
                        }
                        if ui.button("Don't save").clicked() {
                            let after = after.clone();
                            // The project is only replaced if the next step goes
                            // through, so it stays unsaved until then
                            then = Some(Box::new(move |app: &mut Self| app.proceed(after)));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                Dialog::Unsaved(after)
            }
            Dialog::MissingFolders(mut pending) => {
                let dir = pending.project_dir();
                egui::Modal::new(egui::Id::new("missing")).show(ctx, |ui| {
                    ui.heading("Some tile folders weren't found");
                    ui.label("Point to where they are now, or open without them: their tiles are kept as grey boxes and saved as they were.");
                    ui.add_space(6.0);
                    for &i in &pending.missing {
                        let f = &pending.file.folders[i];
                        let shown = f.absolute.clone().unwrap_or_else(|| mtp_core::project::join(&dir, &f.path));
                        ui.horizontal(|ui| {
                            match &pending.found[i] {
                                Some(p) => ui.label(format!("✔ {shown} → {p}")),
                                None => ui.label(&shown),
                            };
                            if ui.button("Locate…").clicked()
                                && let Some(p) = rfd::FileDialog::new().pick_folder()
                            {
                                pending.found[i] = Some(TileKey::from_path(&p).as_str().to_owned());
                            }
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() {
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            pending.missing.clear();
                            pending.found.clear();
                            keep = false;
                        }
                    });
                });
                if !keep && !pending.found.is_empty() {
                    then = Some(Box::new(move |app: &mut Self| app.finish_open(pending)));
                    Dialog::Message { title: String::new(), text: String::new() }
                } else {
                    Dialog::MissingFolders(pending)
                }
            }
            Dialog::GridSize { mut cols, mut rows, mut from_left } => {
                let doc_grid = self.editor.doc.grid();
                egui::Modal::new(egui::Id::new("grid")).show(ctx, |ui| {
                    ui.heading("Grid size");
                    egui::Grid::new("grid size").num_columns(2).show(ui, |ui| {
                        ui.label("Width");
                        ui.add(egui::DragValue::new(&mut cols).range(1..=GridSize::MAX).suffix(" cells"));
                        ui.end_row();
                        ui.label("Depth");
                        ui.add(egui::DragValue::new(&mut rows).range(1..=GridSize::MAX).suffix(" cells"));
                        ui.end_row();
                    });
                    ui.label(
                        RichText::new(format!(
                            "{}\" × {}\" ({} × {} mm)",
                            cols as f32 / 2.0,
                            rows as f32 / 2.0,
                            cols as f64 * 12.5,
                            rows as f64 * 12.5
                        ))
                        .weak(),
                    );
                    ui.horizontal(|ui| {
                        ui.label("Grow or shrink on the");
                        ui.selectable_value(&mut from_left, false, "right and top");
                        ui.selectable_value(&mut from_left, true, "left and bottom");
                    });
                    let grid = GridSize { cols, rows };
                    // Worked out from the sizes now, so changing either keeps to the chosen side
                    let anchor = if from_left {
                        [cols as i32 - doc_grid.cols as i32, rows as i32 - doc_grid.rows as i32]
                    } else {
                        [0, 0]
                    };
                    let outside = self.editor.doc.outside_after_resize(grid, anchor).len();
                    if outside > 0 {
                        ui.label(
                            RichText::new(format!("{outside} tiles don't fit and would be removed (you can undo it)."))
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked() {
                            then = Some(Box::new(move |app: &mut Self| {
                                let removed = app.editor.doc.resize(grid, anchor);
                                app.editor.changed();
                                if removed > 0 {
                                    app.editor.tell(format!("Removed {removed} tiles outside the new grid."));
                                }
                            }));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                Dialog::GridSize { cols, rows, from_left }
            }
            Dialog::RemoveFolder { id, in_use } => {
                let name = self.library.folder(id).map(|f| f.name.clone()).unwrap_or_default();
                egui::Modal::new(egui::Id::new("remove folder")).show(ctx, |ui| {
                    ui.heading(format!("Remove {name} from the library?"));
                    ui.label(format!("{} placed tiles come from it.", in_use.len()));
                    ui.horizontal(|ui| {
                        if ui.button(format!("Remove the {} tiles too", in_use.len())).clicked() {
                            let ids = in_use.clone();
                            then = Some(Box::new(move |app: &mut Self| {
                                app.editor.delete(&ids);
                                app.remove_folder(id);
                            }));
                            keep = false;
                        }
                        if ui
                            .button("Keep the tiles")
                            .on_hover_text("They stay as grey boxes and are saved as they are")
                            .clicked()
                        {
                            then = Some(Box::new(move |app: &mut Self| app.remove_folder(id)));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                Dialog::RemoveFolder { id, in_use }
            }
            Dialog::Recovery(list) => {
                egui::Modal::new(egui::Id::new("recovery")).show(ctx, |ui| {
                    ui.heading("Recover unsaved work?");
                    ui.label("The app closed with unsaved changes. These were saved automatically:");
                    for a in &list {
                        let name = a.original.as_ref().map_or_else(|| "Untitled".into(), |p| p.display().to_string());
                        let age = a.saved_at.elapsed().map(|d| d.as_secs() / 60).unwrap_or(0);
                        ui.horizontal(|ui| {
                            ui.label(format!("{name} ({age} minutes ago)"));
                            if ui.button("Recover").clicked() {
                                let a = a.clone();
                                then = Some(Box::new(move |app: &mut Self| app.proceed(After::Recover(a))));
                                keep = false;
                            }
                        });
                    }
                    if ui.button("Discard all").clicked() {
                        let dir = self.paths.autosave();
                        then = Some(Box::new(move |_: &mut Self| files::discard_autosaves(&dir)));
                        keep = false;
                    }
                });
                Dialog::Recovery(list)
            }
            Dialog::Crash(report) => {
                egui::Modal::new(egui::Id::new("crash")).show(ctx, |ui| {
                    ui.heading("The app closed unexpectedly last time");
                    ui.label("A report was saved. Attaching it to a bug report helps fix the cause.");
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        ui.monospace(report.lines().take(12).collect::<Vec<_>>().join("\n"));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Open log folder").clicked() {
                            open_in_file_manager(&self.paths.logs());
                        }
                        if ui.button("Close").clicked() {
                            keep = false;
                            let autosaves = files::autosaves(&self.paths.autosave());
                            if !autosaves.is_empty() {
                                then = Some(Box::new(move |app: &mut Self| {
                                    app.dialog = Some(Dialog::Recovery(autosaves))
                                }));
                            }
                        }
                    });
                });
                Dialog::Crash(report)
            }
            Dialog::Message { title, text } => {
                egui::Modal::new(egui::Id::new("message")).show(ctx, |ui| {
                    ui.heading(&title);
                    ui.label(&text);
                    if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        keep = false;
                    }
                });
                Dialog::Message { title, text }
            }
        };
        if keep {
            self.dialog = Some(dialog);
        }
        if let Some(f) = then {
            f(self);
        }
    }

    fn remove_folder(&mut self, id: FolderId) {
        let thumbs: Vec<egui::TextureId> = self
            .library
            .folder(id)
            .map(|f| f.tiles.iter().filter_map(|k| self.library.tile(k).and_then(|t| t.thumbnail)).collect())
            .unwrap_or_default();
        for key in self.library.remove_folder(id) {
            self.renderer.release_mesh(&key);
            self.thumb_textures.remove(&key);
        }
        let mut egui_renderer = self.render_state.renderer.write();
        for t in thumbs {
            egui_renderer.free_texture(&t);
        }
        if let Tool::Place { key, .. } = &self.editor.tool
            && self.library.tile(key).is_none()
        {
            self.editor.tool = Tool::Select;
        }
    }

    fn handle_requests(&mut self) {
        for r in std::mem::take(&mut self.requests) {
            match r {
                Request::AddFolder { recursive } => self.add_folder(recursive),
                Request::ReloadFolder(id) => {
                    if let Some(f) = self.library.folder(id) {
                        let (path, recursive) = (PathBuf::from(&f.path), f.recursive);
                        self.remove_folder(id);
                        self.library.add_folder(&path, recursive);
                    }
                }
                Request::RemoveFolder(id) => {
                    let keys: HashSet<TileKey> =
                        self.library.folder(id).map(|f| f.tiles.iter().cloned().collect()).unwrap_or_default();
                    let in_use: Vec<PlacedId> =
                        self.editor.doc.tiles().iter().filter(|t| keys.contains(&t.key)).map(|t| t.id).collect();
                    if in_use.is_empty() {
                        self.remove_folder(id);
                    } else {
                        self.dialog = Some(Dialog::RemoveFolder { id, in_use });
                    }
                }
                Request::SetBattleMap => self.set_battle_map(),
                Request::ClearBattleMap => {
                    self.editor.doc.set_ground_image(None, false);
                    self.editor.changed();
                    self.renderer.set_ground_image(None);
                }
                Request::GridSize => {
                    let g = self.editor.doc.grid();
                    self.dialog = Some(Dialog::GridSize { cols: g.cols, rows: g.rows, from_left: false });
                }
                Request::SampleTiles => match crate::samples::write(&self.paths.data.join("Sample tiles")) {
                    Ok(dir) => {
                        self.library.add_folder(&dir, false);
                    }
                    Err(e) => self.problem(format!("The sample tiles couldn't be written: {e}")),
                },
            }
        }
    }

    fn windows(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new("Settings").open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("settings").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                ui.label("Theme");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.settings.theme, ThemeChoice::System, "Follow system");
                    ui.selectable_value(&mut self.settings.theme, ThemeChoice::Light, "Light");
                    ui.selectable_value(&mut self.settings.theme, ThemeChoice::Dark, "Dark");
                });
                ui.end_row();
                ui.label("Interface size");
                ui.add(
                    egui::Slider::new(&mut self.settings.ui_scale, 0.75..=2.0)
                        .step_by(0.05)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.end_row();
                ui.label("Thumbnails");
                ui.add(egui::Slider::new(&mut self.settings.thumbnail_size, 48.0..=192.0).suffix(" pt"));
                ui.end_row();
                ui.label("3D detail");
                ui.add(egui::Slider::new(&mut self.settings.detail, 0.1..=4.0).logarithmic(true))
                    .on_hover_text("Lower is faster on weak graphics cards");
                ui.end_row();
                ui.label("Start in");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.settings.start_in_3d, false, "2D");
                    ui.selectable_value(&mut self.settings.start_in_3d, true, "3D");
                });
                ui.end_row();
                ui.label("Autosave every");
                ui.add(egui::DragValue::new(&mut self.settings.autosave_minutes).range(0..=60).suffix(" min"))
                    .on_hover_text("0 turns autosave off");
                ui.end_row();
                ui.label("Graphics");
                egui::ComboBox::from_id_salt("backend").selected_text(self.settings.backend.label()).show_ui(
                    ui,
                    |ui| {
                        for b in [Backend::Auto, Backend::Vulkan, Backend::Dx12, Backend::Metal, Backend::OpenGl] {
                            ui.selectable_value(&mut self.settings.backend, b, b.label());
                        }
                    },
                );
                ui.end_row();
                ui.label("");
                ui.label(RichText::new("Takes effect after a restart.").weak().small());
                ui.end_row();
                ui.label("Mesh cache");
                ui.horizontal(|ui| {
                    let used = self.library.cache().map_or(0, |c| c.size_bytes() >> 20);
                    ui.add(egui::DragValue::new(&mut self.settings.mesh_cache_mb).range(64..=65536).suffix(" MB"));
                    ui.label(RichText::new(format!("{used} MB used")).weak());
                    if ui.button("Clear").clicked()
                        && let Some(c) = self.library.cache()
                        && let Err(e) = c.clear()
                    {
                        tracing::warn!("couldn't clear the mesh cache: {e}");
                    }
                });
                ui.end_row();
                ui.label("Build plan paper");
                egui::ComboBox::from_id_salt("paper").selected_text(self.settings.paper.clone()).show_ui(ui, |ui| {
                    for (name, _) in exports::PAPERS {
                        ui.selectable_value(&mut self.settings.paper, name.to_owned(), name);
                    }
                });
                ui.end_row();
                ui.label("Filament density");
                ui.add(
                    egui::DragValue::new(&mut self.settings.filament_density)
                        .range(0.5..=3.0)
                        .speed(0.01)
                        .suffix(" g/cm³"),
                )
                .on_hover_text("For filament weight estimates. PLA is about 1.24, PETG 1.27");
                ui.end_row();
                ui.label("Print lists");
                ui.checkbox(&mut self.settings.csv_full_paths, "Write full file paths (they include your user name)");
                ui.end_row();
                ui.label("Statistics");
                ui.checkbox(&mut self.settings.show_stats, "Show drawing statistics in the status bar");
                ui.end_row();
            });
        });
        self.show_settings = open;

        let mut open = self.show_about;
        let info = self.render_state.adapter.get_info();
        egui::Window::new(format!("About {APP_NAME}")).open(&mut open).resizable(false).show(ctx, |ui| {
            ui.heading(format!("{APP_NAME} {VERSION}"));
            ui.label("Plan modular terrain from your STL tiles, then print exactly what the layout needs.");
            ui.hyperlink_to("github.com/HaXrDEV/modular-terrain-planner", "https://github.com/HaXrDEV/modular-terrain-planner");
            ui.separator();
            egui::Grid::new("about").num_columns(2).show(ui, |ui| {
                ui.label("Graphics");
                ui.label(format!("{} ({:?}{})", info.name, info.backend, if info.driver.is_empty() { String::new() } else { format!(", {}", info.driver) }));
                ui.end_row();
                ui.label("Settings");
                ui.label(self.paths.settings_file().display().to_string());
                ui.end_row();
                ui.label("Logs");
                ui.label(self.paths.logs().display().to_string());
                ui.end_row();
                ui.label("Mesh cache");
                ui.label(self.paths.mesh_cache().display().to_string());
                ui.end_row();
            });
            ui.separator();
            ui.label(RichText::new("MIT licence. Uses egui, wgpu and meshoptimizer (MIT/Apache-2.0); build plans use the Cantarell font (SIL Open Font Licence 1.1). The full list ships with the app.").small());
        });
        self.show_about = open;

        let mut open = self.show_shortcuts;
        egui::Window::new("Keyboard and mouse").open(&mut open).vscroll(true).show(ctx, |ui| {
            egui::Grid::new("mouse").num_columns(2).striped(true).show(ui, |ui| {
                for (input, what) in MOUSE {
                    ui.label(RichText::new(*input).strong());
                    ui.label(*what);
                    ui.end_row();
                }
            });
            let mut group = "";
            egui::Grid::new("keys").num_columns(2).striped(true).show(ui, |ui| {
                for s in COMMANDS {
                    let Some(k) = s.shortcut else { continue };
                    if s.group != group {
                        group = s.group;
                        ui.label(RichText::new(group).heading());
                        ui.end_row();
                    }
                    ui.label(RichText::new(ctx.format_shortcut(&k)).strong());
                    ui.label(s.label.trim_end_matches('…'));
                    ui.end_row();
                }
            });
        });
        self.show_shortcuts = open;
    }
}

impl App {
    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        if self.screenshot.is_none() {
            return;
        }
        if let Some(Dialog::MissingFolders(_)) = &self.dialog
            && let Some(Dialog::MissingFolders(pending)) = self.dialog.take()
        {
            self.finish_open(pending);
        }
        let Some((path, settled)) = self.screenshot.as_mut() else { return };
        ctx.request_repaint();
        if self.library.progress().is_some() || !self.thumbnails.is_empty() {
            *settled = 0;
            return;
        }
        *settled += 1;
        if *settled == 20 {
            if let Some(Tool::Select) = Some(&self.editor.tool)
                && self.editor.selection.is_empty()
                && let Some(t) = self.editor.doc.tiles().first()
            {
                self.editor.selection.insert(t.id);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::RgbaImage::from_raw(w as u32, h as u32, rgba).map(|img| img.save(&*path)) {
                Some(Ok(())) => println!("wrote {}", path.display()),
                other => eprintln!("screenshot failed: {other:?}"),
            }
            self.quitting = true;
            self.editor.doc.mark_saved();
        }
    }
}

/// What the app was started with.
#[derive(Default)]
pub struct Startup {
    pub open: Vec<PathBuf>,
    pub problems: Vec<String>,
    pub crash_report: Option<String>,
    /// `--screenshot <file>`: once the library has loaded, capture the window
    /// to a PNG and quit (for documentation and checking the UI in CI).
    pub screenshot: Option<PathBuf>,
    /// Files sent by later launches.
    pub incoming: Option<crate::instance::Incoming>,
}

pub fn open_in_file_manager(path: &Path) {
    let target = if path.is_file() { path.parent().unwrap_or(path) } else { path };
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(target).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(target).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(target).spawn();
    if let Err(e) = result {
        tracing::warn!("couldn't open {}: {e}", target.display());
    }
}

impl TabViewer for App {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Library => "Library".into(),
            Tab::View => "Layout".into(),
            Tab::Inspector => "Inspector".into(),
            Tab::PrintList => "Print list".into(),
            Tab::Problems if self.problems.is_empty() => "Problems".into(),
            Tab::Problems => format!("Problems ({})", self.problems.len()).into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        if *tab == Tab::View {
            self.tab_ui(ui, *tab);
        } else {
            egui::Frame::new().inner_margin(8).show(ui, |ui| self.tab_ui(ui, *tab));
        }
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }

    fn clear_background(&self, tab: &Tab) -> bool {
        *tab != Tab::View
    }
}

impl App {
    fn tab_ui(&mut self, ui: &mut egui::Ui, tab: Tab) {
        match tab {
            Tab::Library => {
                let requests = self.library_panel.ui(ui, &self.library, &mut self.editor, self.settings.thumbnail_size);
                self.requests.extend(requests);
            }
            Tab::View => self.viewport.ui(
                ui,
                ViewCtx {
                    editor: &mut self.editor,
                    library: &self.library,
                    renderer: &mut self.renderer,
                    render_state: &self.render_state,
                    detail: self.settings.detail,
                    keys_blocked: self.dialog.is_some(),
                },
            ),
            Tab::Inspector => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let requests = panels::inspector(ui, &mut self.editor, &self.library);
                    self.requests.extend(requests);
                });
            }
            Tab::PrintList => {
                let problems = panels::print_list(
                    ui,
                    &self.editor,
                    &self.library,
                    &mut self.inventory,
                    self.settings.filament_density,
                );
                for p in problems {
                    self.problem(p);
                }
            }
            Tab::Problems => panels::problems(ui, &mut self.problems),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frames += 1;
        self.apply_settings(&ctx);
        self.library_events();
        self.make_thumbnails(&ctx);
        if self.frames.is_multiple_of(300) {
            self.evict_unused();
        }
        self.handle_drops(&ctx);
        self.shortcuts(&ctx);
        // Undo and redo can bring back, remove or swap the battle map
        if self.editor.doc.ground_image().map(|i| &i.path) != self.image_shown.as_ref() {
            self.load_ground_image();
        }
        self.autosave_tick();

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        let mut dock = std::mem::replace(&mut self.dock, DockState::new(Vec::new()));
        let mut style = egui_dock::Style::from_egui(ui.style());
        // The view fills its tab edge to edge; the panels add their own margin
        style.tab.tab_body.inner_margin = egui::Margin::ZERO;
        style.tab.tab_body.stroke = egui::Stroke::NONE;
        style.main_surface_border_stroke = egui::Stroke::NONE;
        DockArea::new(&mut dock)
            .style(style)
            .show_close_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, self);
        self.dock = dock;
        self.handle_requests();
        self.dialogs(&ctx);
        self.windows(&ctx);
        self.update_title(&ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.editor.doc.is_dirty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.dialog = Some(Dialog::Unsaved(After::Quit));
            } else {
                self.quitting = true;
            }
        }
        if self.quitting {
            // Screenshot runs leave the user's settings and autosaves alone
            if self.screenshot.is_none() {
                self.remember_session();
                self.discard_own_autosave();
            }
            self.library.cancel_all();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.dialog.is_none() && !self.deferred_open.is_empty() {
            let paths = std::mem::take(&mut self.deferred_open);
            if let Some(path) = paths.first() {
                self.guard(After::Open(Some(path.clone())));
            }
        }
        self.screenshot_tick(&ctx);
        // Keep autosave and notices ticking without burning a core
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn on_exit(&mut self) {
        if self.screenshot.is_some() {
            return;
        }
        self.remember_session();
    }
}
