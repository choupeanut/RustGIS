//! `ui.captureLayout` copies the open layout's page as an image, never silently shrunk.
use rustgis_engine::{Project, Session};
use rustgis_ui_egui::RustGisApp;
use serde_json::json;

fn app() -> RustGisApp {
    RustGisApp::new(Session::new(Project::default()))
}

fn said(app: &RustGisApp, part: &str) -> bool {
    app.session.notices.iter().any(|n| n.text.contains(part))
}

#[test]
fn a_layout_is_copied_at_its_dpi() {
    let mut app = app();
    app.run("layout.new", json!({"name": "Copy", "width": 210.0, "height": 297.0, "orientation": "portrait"})).unwrap();
    app.run("ui.captureLayout", json!(null)).unwrap();
    let img = app.ui.copy_image.take().expect("an image to copy");
    // 192 dpi on A4.
    let expected = [(210.0_f64 / 25.4 * 192.0) as i64, (297.0_f64 / 25.4 * 192.0) as i64];
    for (got, want) in img.size.iter().zip(expected) {
        assert!((*got as i64 - want).abs() <= 2, "{:?} vs {expected:?}", img.size);
    }
    assert!(said(&app, "Copied the layout to the clipboard"));
    assert!(!said(&app, " dpi"));
}

#[test]
fn a_page_over_the_pixel_cap_is_copied_smaller_and_says_so() {
    let mut app = app();
    app.run("layout.new", json!({"name": "Big", "width": 841.0, "height": 1189.0, "orientation": "portrait"})).unwrap();
    app.run("ui.captureLayout", json!(null)).unwrap();
    let img = app.ui.copy_image.take().expect("an image to copy");
    assert!(img.size[0] * img.size[1] <= 16_100_000, "{:?}", img.size);
    assert!(img.size[0] < 841 * 192 / 25);
    // The page's proportions are kept.
    let (page, got) = (841.0 / 1189.0, img.size[0] as f64 / img.size[1] as f64);
    assert!((page - got).abs() < 0.01, "{page} vs {got}");
    assert!(said(&app, "dpi, not 192"));
}

#[test]
fn without_a_layout_it_is_an_error() {
    let mut app = app();
    let e = app.run("ui.captureLayout", json!(null)).unwrap_err();
    assert!(e.contains("open a layout first"), "{e}");
    assert!(app.ui.copy_image.is_none());
    assert!(said(&app, "open a layout first"));
}
