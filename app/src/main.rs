//! Modular Terrain Planner: lay out modular terrain from STL tiles on a grid
//! and export a build plan of what to print and where it goes.

mod app;
mod commands;
mod diagnostics;
mod editor;
mod exports;
mod files;
mod library;
mod panels;
mod samples;
mod selftest;
mod settings;
mod viewport;

use std::path::PathBuf;

use eframe::{egui, egui_wgpu, wgpu};

use crate::settings::{APP_NAME, Paths, Settings, VERSION};

const HELP: &str = "Usage: modular-terrain-planner [options] [project.mtp]

Options:
  --self-test [--software]  check loading, rendering and saving without a window,
                            then exit (0 when everything passed)
  --portable <folder>       keep settings, logs and caches in <folder>
  --write-samples <folder>  write the sample tiles to <folder>, then exit
  --version                 print the version
  --help                    print this text
";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version") {
        println!("{APP_NAME} {VERSION}");
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(dir) = args.iter().position(|a| a == "--write-samples").and_then(|i| args.get(i + 1)) {
        return match samples::write(std::path::Path::new(dir)) {
            Ok(d) => {
                println!("wrote sample tiles to {}", d.display());
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if args.iter().any(|a| a == "--self-test") {
        let (report, ok) = selftest::run(args.iter().any(|a| a == "--software"));
        print!("{report}");
        return if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE };
    }

    let portable = args.iter().position(|a| a == "--portable").and_then(|i| args.get(i + 1));
    let paths = portable.map_or_else(Paths::from_os, |dir| Paths::under(&PathBuf::from(dir)));
    let _log = diagnostics::init_logging(&paths.logs());
    let crash_report = diagnostics::take_crash_report(&paths.logs());
    diagnostics::install_panic_hook(paths.logs());
    let (settings, settings_problem) = Settings::load(&paths);
    // Files and folders to open: every argument that isn't an option or an option's value
    let mut open = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--portable" | "--screenshot" => i += 2,
            a if a.starts_with("--") => i += 1,
            a => {
                // Absolute, so recent files and autosave notes work from anywhere
                let path = PathBuf::from(a);
                open.push(std::path::absolute(&path).unwrap_or(path));
                i += 1;
            }
        }
    }
    let screenshot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let startup = app::Startup { open, problems: settings_problem.into_iter().collect(), crash_report, screenshot };

    match run(paths, settings, startup) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            eprintln!("{APP_NAME} couldn't start: {e}");
            show_startup_error(&e);
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(paths: Paths, settings: Settings, startup: app::Startup) -> Result<(), String> {
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    if let Some(choice) = settings.backend.env_value() {
        setup.instance_descriptor.backends = wgpu::Backends::from_comma_list(choice);
    }
    let wgpu_options =
        egui_wgpu::WgpuConfiguration { wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup), ..Default::default() };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id("modular-terrain-planner")
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        // Screenshots for the docs always come out the same size
        persist_window: startup.screenshot.is_none(),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, paths, settings, startup)?))),
    )
    .map_err(|e| match e {
        eframe::Error::Wgpu(w) => format!(
            "the graphics driver couldn't start ({w}). Updating the graphics driver usually helps. \
             For old drivers, choose OpenGL under Settings > Graphics."
        ),
        other => other.to_string(),
    })
}

/// A last resort when the window can't open: a native message box.
fn show_startup_error(message: &str) {
    rfd::MessageDialog::new()
        .set_title(APP_NAME)
        .set_level(rfd::MessageLevel::Error)
        .set_description(format!("{APP_NAME} couldn't start: {message}"))
        .show();
}
