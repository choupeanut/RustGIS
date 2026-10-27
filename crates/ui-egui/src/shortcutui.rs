//! User customization of keyboard shortcuts.
use crate::{
    RustGisApp,
    keymap::{self, Chord, Keymap},
};
use egui::{Context, Event};
use serde_json::{Value, json};

const PREF: &str = "shortcut_changes";

#[derive(Clone)]
struct Target {
    command: String,
    params: Value,
}

#[derive(Default)]
pub struct Editor {
    pub open: bool,
    pub filter: String,
    capture: Option<Target>,
    pending: Option<(Chord, Target)>,
    notice_clipboard: bool,
}

impl Editor {
    pub fn capturing(&self) -> bool {
        self.capture.is_some() || self.pending.is_some()
    }
}

fn save(app: &RustGisApp) {
    let defaults = Keymap::new(&app.session.registry);
    if let Some(save) = &app.services.save_pref {
        save(PREF, &app.ui.keymap.preferences(&defaults));
    }
}

pub fn load(app: &mut RustGisApp) {
    if let Some(changes) = app.services.load_pref.as_ref().and_then(|load| load(PREF)) {
        app.ui.keymap.apply_preferences(&changes);
    }
}

fn assign(app: &mut RustGisApp, editor: &mut Editor, chord: Chord, target: Target) {
    if keymap::is_clipboard(chord) {
        editor.notice_clipboard = true;
    } else if app.ui.keymap.command_for(chord).is_some_and(|b| !b.runs(&target.command, &target.params)) {
        editor.pending = Some((chord, target));
    } else if app.ui.keymap.assign(chord, &target.command, target.params) {
        save(app);
    }
}

fn rows(app: &RustGisApp) -> Vec<(String, Value, String)> {
    let mut rows = Vec::new();
    for c in &app.session.registry.commands {
        if !c.needs_input() && !matches!(c.id, "edit.copy" | "edit.cut" | "edit.paste") {
            rows.push((c.id.to_string(), Value::Null, tl!(c.label).to_string()));
        }
    }
    for &(id, label, _) in crate::chrome::UI_COMMANDS {
        rows.push((id.to_string(), Value::Null, tl!(label).to_string()));
    }
    for c in rustgis_engine::plugins::commands() {
        let label = c.plugin.manifest.translate(app.lang().code(), &c.decl.label).to_string();
        rows.push((c.id, Value::Null, label));
    }
    for &(tool, label) in keymap::MAP_TOOLS {
        rows.push(("tool.set".to_string(), json!({"tool": tool}), crate::i18n::fmt("Map Tool: {tool}", &[("tool", tl_raw!(label))])));
    }
    for (command, params, _) in app.ui.keymap.grouped() {
        if !rows.iter().any(|(c, p, _)| c == command && p == params) {
            rows.push((command.to_string(), params.clone(), keymap::label(app, command, params)));
        }
    }
    let mut seen = std::collections::HashSet::new();
    rows.retain(|(command, params, _)| seen.insert((command.clone(), params.to_string())));
    rows.sort_by_cached_key(|a| a.2.to_lowercase());
    rows
}

/// Whether Command Search can open a matching row in the editor.
pub fn can_customize(app: &RustGisApp, id: &str) -> bool {
    if matches!(id, "edit.copy" | "edit.cut" | "edit.paste") {
        return false;
    }
    crate::chrome::UI_COMMANDS.iter().any(|(command, _, _)| *command == id)
        || app.session.registry.commands.iter().any(|c| c.id == id && !c.needs_input())
        || rustgis_engine::plugins::command(id).is_some()
        || app.ui.keymap.bindings.iter().any(|b| b.command == id)
}

/// Draws the editor after the normal map and dialogs.
pub fn show(app: &mut RustGisApp, ctx: &Context) {
    let mut editor = std::mem::take(&mut app.ui.shortcut_editor);
    let was_capturing = editor.capture.is_some();
    if !editor.open {
        app.ui.shortcut_editor = editor;
        return;
    }
    let mut open = true;
    let mut remove = None;
    let mut restore = None;
    let mut reset_all = false;
    let mut assign_escape = false;
    let defaults = Keymap::new(&app.session.registry);
    let rows = rows(app);
    egui::Window::new(tl!("Customize Shortcuts")).open(&mut open).default_width(620.0).default_height(500.0).resizable(true).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                let label = ui.label(tl!("Search commands"));
                ui.add_enabled(!editor.capturing(), egui::TextEdit::singleline(&mut editor.filter)).labelled_by(label.id);
                if ui.button(tl!("Reset All")).clicked() {
                    reset_all = true;
                }
            });
            if cfg!(target_arch = "wasm32") {
                ui.label(tl!("Your browser may reserve some shortcuts."));
            }
            ui.label(tl!("While typing, shortcuts without Ctrl/⌘ are paused. First-person navigation keeps Escape, Home and Delete."));
            if editor.notice_clipboard {
                ui.label(tl!("Copy, Cut and Paste keys cannot be reassigned."));
            }
            if editor.capture.is_some() {
                ui.horizontal(|ui| {
                    ui.label(tl!("Press a key combination to assign it. Escape cancels."));
                    assign_escape = ui.button(tl!("Assign Escape")).clicked();
                    if ui.button(tl!("Cancel")).clicked() {
                        editor.capture = None;
                    }
                });
            }
            let filter = editor.filter.to_lowercase();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (command, params, label) in &rows {
                    if !filter.is_empty() && !label.to_lowercase().contains(&filter) && !command.to_lowercase().contains(&filter) {
                        continue;
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(label);
                        for chord in app.ui.keymap.keys_for(command, params).collect::<Vec<_>>() {
                            if ui
                                .small_button(format!("{} ×", chord.text(ctx.os().is_mac())))
                                .on_hover_text(tl!("Remove shortcut"))
                                .clicked()
                            {
                                remove = Some(chord);
                            }
                        }
                        if ui
                            .small_button(tl!("Add…"))
                            .on_hover_text(tl!("Tab, Enter, Space and arrow key bindings can override keyboard navigation."))
                            .clicked()
                        {
                            editor.notice_clipboard = false;
                            editor.capture = Some(Target { command: command.clone(), params: params.clone() });
                        }
                        for chord in defaults.keys_for(command, params) {
                            if app.ui.keymap.command_for(chord).is_none_or(|b| !b.runs(command, params))
                                && ui.small_button(crate::i18n::fmt("Reset {key}", &[("key", &chord.text(ctx.os().is_mac()))])).clicked()
                            {
                                restore = Some((chord, Target { command: command.clone(), params: params.clone() }));
                            }
                        }
                    });
                    ui.separator();
                }
            });
        },
    );
    if !open {
        editor = Editor::default();
    }
    if reset_all {
        app.ui.keymap = defaults;
        editor.capture = None;
        editor.pending = None;
        editor.notice_clipboard = false;
        save(app);
    }
    if let Some(chord) = remove {
        app.ui.keymap.remove(chord);
        save(app);
    }
    if let Some((chord, target)) = restore {
        assign(app, &mut editor, chord, target);
    }
    if assign_escape {
        if let Some(target) = editor.capture.take()
            && let Some(chord) = Chord::parse("Escape")
        {
            assign(app, &mut editor, chord, target);
        }
    } else if was_capturing && editor.capture.is_some() {
        let chord = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                Event::Key { key, pressed: true, repeat: false, modifiers, .. } => {
                    Some(Chord { command: modifiers.command, alt: modifiers.alt, shift: modifiers.shift, key: *key })
                }
                _ => None,
            })
        });
        if let Some(chord) = chord
            && let Some(target) = editor.capture.take()
        {
            if chord == (Chord { command: false, alt: false, shift: false, key: egui::Key::Escape }) {
                editor.capture = None;
            } else {
                assign(app, &mut editor, chord, target);
            }
        }
    }
    if let Some((chord, target)) = editor.pending.clone() {
        let existing = app.ui.keymap.command_for(chord).map(|b| keymap::label(app, &b.command, &b.params)).unwrap_or_default();
        egui::Modal::new(egui::Id::unique("shortcut_conflict")).show(ctx, |ui| {
            ui.heading(tl!("Shortcut already assigned"));
            ui.label(crate::i18n::fmt("{key} is used by {command}.", &[("key", &chord.text(ctx.os().is_mac())), ("command", &existing)]));
            ui.horizontal(|ui| {
                if ui.button(tl!("Replace")).clicked() {
                    if app.ui.keymap.assign(chord, &target.command, target.params.clone()) {
                        save(app);
                    }
                    editor.pending = None;
                }
                if ui.button(tl!("Cancel")).clicked() {
                    editor.pending = None;
                }
            });
        });
    }
    app.ui.shortcut_editor = editor;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_engine::{Project, Session};

    #[test]
    fn conflicting_and_clipboard_keys_are_not_changed_without_confirmation() {
        let mut app = RustGisApp::new(Session::new(Project::default()));
        let mut editor = Editor::default();
        let target = Target { command: "select.clear".into(), params: Value::Null };
        let chord = Chord::parse("Mod+T").unwrap();
        let before = app.ui.keymap.command_for(chord).unwrap().command.clone();
        assign(&mut app, &mut editor, chord, target.clone());
        assert!(editor.pending.is_some());
        assert_eq!(app.ui.keymap.command_for(chord).unwrap().command, before);
        editor.pending = None;
        let clipboard = Chord::parse("Mod+C").unwrap();
        assign(&mut app, &mut editor, clipboard, target);
        assert!(editor.notice_clipboard);
        assert!(app.ui.keymap.command_for(clipboard).is_none());
    }

    #[test]
    fn search_customize_requires_an_editor_row() {
        let mut app = RustGisApp::new(Session::new(Project::default()));
        assert!(can_customize(&app, "ui.theme"));
        assert!(!can_customize(&app, "layer.addData"));
        assert!(!can_customize(&app, "edit.copy"));
        assert!(!can_customize(&app, "ui.identify"));
        assert!(app.ui.keymap.assign(Chord::parse("D").unwrap(), "layer.addData", json!({"path": "example.geojson"})));
        assert!(can_customize(&app, "layer.addData"));
    }
}
