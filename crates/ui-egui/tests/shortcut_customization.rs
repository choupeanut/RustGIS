//! Interaction tests for customizable shortcuts.
use egui_kittest::kittest::Queryable;
use rustgis_engine::{Project, Session};
use rustgis_ui_egui::{RustGisApp, Services};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

fn harness(prefs: Arc<Mutex<HashMap<String, String>>>) -> egui_kittest::Harness<'static, RustGisApp> {
    let reads = prefs.clone();
    let writes = prefs;
    let services = Services {
        load_pref: Some(Box::new(move |key| reads.lock().unwrap().get(key).cloned())),
        save_pref: Some(Box::new(move |key, value| {
            writes.lock().unwrap().insert(key.to_string(), value.to_string());
        })),
        ..Services::default()
    };
    let app = RustGisApp::new(Session::new(Project::default())).with_services(services);
    egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
}

#[test]
fn assigns_a_key_confirms_conflicts_and_restores_saved_preferences() {
    let prefs = Arc::new(Mutex::new(HashMap::new()));
    let mut h = harness(prefs.clone());
    h.state_mut().run("ui.customizeShortcuts", Value::Null).unwrap();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search commands").focus();
    h.event(egui::Event::Text("ui.theme".into()));
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Add…").click();
    h.step();
    h.key_press(egui::Key::T);
    h.run_steps(3);
    assert!(prefs.lock().unwrap().get("shortcut_changes").is_some_and(|p| p.contains("ui.theme")));

    h.get_by_role_and_label(egui::accesskit::Role::Button, "Add…").click();
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::T);
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Cancel").click();
    h.run_steps(3);
    let before = prefs.lock().unwrap().get("shortcut_changes").cloned().unwrap();
    assert!(!before.contains("Mod+T"));

    let restored = harness(prefs);
    assert!(restored.state().ui.keymap.bindings.iter().any(|b| b.command == "ui.theme" && b.chord.key == egui::Key::T));
    assert!(
        restored.state().ui.keymap.bindings.iter().any(|b| b.command == "table.open" && b.chord.key == egui::Key::T && b.chord.command)
    );
}
