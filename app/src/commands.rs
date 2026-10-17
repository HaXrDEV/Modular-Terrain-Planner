//! Every action the app has, with its name and keyboard shortcut. Menus, the
//! shortcut list in Help and key handling all read this table, so a shortcut
//! can't work without being listed or be listed without working. (In the
//! Python version keys only worked while the 3D view had focus, and none
//! were in a menu.)

use eframe::egui::{Key, KeyboardShortcut, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    New,
    Open,
    Save,
    SaveAs,
    AddFolder,
    AddFolderWithSubfolders,
    ExportPdf,
    ExportPng,
    ExportCsv,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
    SelectSame,
    NextVariant,
    PreviousVariant,
    RandomiseVariants,
    TurnLeft,
    TurnRight,
    ToolSelect,
    ToolErase,
    ToolMeasure,
    Escape,
    GridSize,
    SetBattleMap,
    ClearBattleMap,
    Toggle3d,
    Frame,
    ResetView,
    ToggleGrid,
    Settings,
    Shortcuts,
    About,
    OpenLogFolder,
}

pub struct Spec {
    pub command: Command,
    pub label: &'static str,
    pub shortcut: Option<KeyboardShortcut>,
    /// Shown in the shortcut list for keys that aren't a single shortcut.
    pub group: &'static str,
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

const CMD: Modifiers = Modifiers::COMMAND;
const NONE: Modifiers = Modifiers::NONE;
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::COMMAND };

pub const COMMANDS: &[Spec] = &[
    Spec { command: Command::New, label: "New project", shortcut: key(CMD, Key::N), group: "File" },
    Spec { command: Command::Open, label: "Open project…", shortcut: key(CMD, Key::O), group: "File" },
    Spec { command: Command::Save, label: "Save", shortcut: key(CMD, Key::S), group: "File" },
    Spec { command: Command::SaveAs, label: "Save as…", shortcut: key(CMD_SHIFT, Key::S), group: "File" },
    Spec { command: Command::AddFolder, label: "Add tile folder…", shortcut: key(CMD, Key::L), group: "File" },
    Spec {
        command: Command::AddFolderWithSubfolders,
        label: "Add tile folder with subfolders…",
        shortcut: None,
        group: "File",
    },
    Spec {
        command: Command::ExportPdf, label: "Export build plan (PDF)…", shortcut: key(CMD, Key::E), group: "File"
    },
    Spec { command: Command::ExportPng, label: "Export assembly map (PNG)…", shortcut: None, group: "File" },
    Spec {
        command: Command::ExportCsv,
        label: "Export print list (CSV)…",
        shortcut: key(CMD_SHIFT, Key::E),
        group: "File",
    },
    Spec { command: Command::Quit, label: "Quit", shortcut: key(CMD, Key::Q), group: "File" },
    Spec { command: Command::Undo, label: "Undo", shortcut: key(CMD, Key::Z), group: "Edit" },
    Spec { command: Command::Redo, label: "Redo", shortcut: key(CMD_SHIFT, Key::Z), group: "Edit" },
    Spec { command: Command::Cut, label: "Cut", shortcut: key(CMD, Key::X), group: "Edit" },
    Spec { command: Command::Copy, label: "Copy", shortcut: key(CMD, Key::C), group: "Edit" },
    Spec { command: Command::Paste, label: "Paste", shortcut: key(CMD, Key::V), group: "Edit" },
    Spec { command: Command::Delete, label: "Delete", shortcut: key(NONE, Key::Delete), group: "Edit" },
    Spec { command: Command::SelectAll, label: "Select all", shortcut: key(CMD, Key::A), group: "Edit" },
    Spec { command: Command::SelectSame, label: "Select all of this tile", shortcut: None, group: "Edit" },
    Spec {
        command: Command::NextVariant,
        label: "Next tile of the same size",
        shortcut: key(NONE, Key::C),
        group: "Edit",
    },
    Spec {
        command: Command::PreviousVariant,
        label: "Previous tile of the same size",
        shortcut: key(Modifiers::SHIFT, Key::C),
        group: "Edit",
    },
    Spec { command: Command::RandomiseVariants, label: "Randomise variants", shortcut: None, group: "Edit" },
    Spec { command: Command::TurnLeft, label: "Turn counter-clockwise", shortcut: key(NONE, Key::Q), group: "Edit" },
    Spec { command: Command::TurnRight, label: "Turn clockwise", shortcut: key(NONE, Key::E), group: "Edit" },
    Spec { command: Command::ToolSelect, label: "Select tool", shortcut: key(NONE, Key::V), group: "Tools" },
    Spec { command: Command::ToolErase, label: "Erase tool", shortcut: key(NONE, Key::X), group: "Tools" },
    Spec { command: Command::ToolMeasure, label: "Measure tool", shortcut: key(NONE, Key::M), group: "Tools" },
    Spec {
        command: Command::Escape,
        label: "Cancel, then back to Select",
        shortcut: key(NONE, Key::Escape),
        group: "Tools",
    },
    Spec { command: Command::GridSize, label: "Grid size…", shortcut: None, group: "Edit" },
    Spec { command: Command::SetBattleMap, label: "Set battle map…", shortcut: None, group: "Edit" },
    Spec { command: Command::ClearBattleMap, label: "Remove battle map", shortcut: None, group: "Edit" },
    Spec { command: Command::Toggle3d, label: "Switch 2D / 3D view", shortcut: key(NONE, Key::T), group: "View" },
    Spec { command: Command::Frame, label: "Frame selection or layout", shortcut: key(NONE, Key::F), group: "View" },
    Spec { command: Command::ResetView, label: "Reset view", shortcut: key(NONE, Key::Home), group: "View" },
    Spec { command: Command::ToggleGrid, label: "Show grid", shortcut: key(NONE, Key::G), group: "View" },
    Spec { command: Command::Settings, label: "Settings…", shortcut: key(CMD, Key::Comma), group: "View" },
    Spec { command: Command::Shortcuts, label: "Keyboard and mouse", shortcut: key(NONE, Key::F1), group: "Help" },
    Spec { command: Command::About, label: "About", shortcut: None, group: "Help" },
    Spec { command: Command::OpenLogFolder, label: "Open log folder", shortcut: None, group: "Help" },
];

pub fn spec(command: Command) -> &'static Spec {
    COMMANDS.iter().find(|s| s.command == command).unwrap_or(&COMMANDS[0])
}

/// Mouse and keys that aren't commands, for the shortcut list.
pub const MOUSE: &[(&str, &str)] = &[
    ("Left click", "Use the tool: select, place or erase"),
    ("Left drag", "Move the selection, box-select, or paint tiles"),
    ("Shift + drag (placing)", "Fill a rectangle with the tile"),
    ("Shift + click (placing)", "Place a line of tiles from the last one"),
    ("1 to 9", "Pick one of the recently placed tiles"),
    ("Shift + click", "Add to or remove from the selection"),
    ("Ctrl + click or drag", "Place or move freely, off the grid (only the grid edge is checked)"),
    ("Alt + click", "Pick up a tile: choose it and its turn for placing"),
    ("Right drag", "Orbit in 3D, pan in 2D"),
    ("Middle drag, or Space + drag", "Pan"),
    ("Wheel", "Zoom toward the pointer"),
    ("Right click", "Menu for the tile under the pointer"),
    ("Arrow keys", "Nudge the selection a cell (Shift: 1\"), or pan"),
    ("W A S D", "Pan"),
    ("Page Up, Page Down", "Edit the level above or below"),
    ("M, then drag", "Measure a distance in cells, inches and millimetres"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn shortcuts_are_unique() {
        let mut seen = HashSet::new();
        for s in COMMANDS {
            if let Some(k) = s.shortcut {
                assert!(
                    seen.insert((k.logical_key, k.modifiers.command, k.modifiers.shift)),
                    "{} reuses a key",
                    s.label
                );
            }
        }
    }

    #[test]
    fn every_command_is_listed_once() {
        let mut seen = HashSet::new();
        for s in COMMANDS {
            assert!(seen.insert(s.command), "{:?} listed twice", s.command);
        }
        assert_eq!(spec(Command::About).label, "About");
    }
}
