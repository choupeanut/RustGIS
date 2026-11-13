//! `table.statistics`: a column's numbers, without an output table or an undo step.
use super::*;
use rustgis_data::{Dataset, Date, Field, FieldType, Geometry, Value as V};

/// A layer "parcels" of five points: `area` (decimals, one null, one NaN), `n` (whole numbers),
/// `name` (text, one empty, one null) and `built` (dates).
fn parcels() -> Session {
    let mut s = Session::new(Project::default());
    let fields = vec![
        Field::new("area", FieldType::Double),
        Field::new("n", FieldType::Integer),
        Field::new("name", FieldType::Text),
        Field::new("built", FieldType::Date),
    ];
    let mut ds = Dataset::new("parcels", rustgis_data::GeometryType::Point, fields);
    let date = |t: &str| V::Date(Date::parse(t).unwrap());
    let rows = [
        (V::Float(2.0), V::Int(10), V::Text("b".into()), date("2020-05-01")),
        (V::Float(4.0), V::Int(20), V::Text("a".into()), date("1999-12-31")),
        (V::Float(6.0), V::Int(30), V::Text("b".into()), V::Null),
        (V::Null, V::Int(40), V::Text(String::new()), date("2026-01-01")),
        (V::Float(f64::NAN), V::Null, V::Null, V::Null),
    ];
    for (i, (a, n, t, d)) in rows.into_iter().enumerate() {
        ds.push(Some(Geometry::Point(rustgis_geom::Coord::new(i as f64, 0.0))), vec![a, n, t, d]);
    }
    crate::cmd::layer::add_dataset(&mut s, ds);
    s
}

fn stats(s: &mut Session, p: Value) -> Value {
    s.run("table.statistics", &p).unwrap()
}

#[test]
fn a_numeric_column_gives_its_numbers_over_all_rows() {
    let mut s = parcels();
    let mark = s.notice_mark();
    let v = stats(&mut s, json!({"layer": "parcels", "field": "area"}));
    assert_eq!(v["rows"], 5);
    assert_eq!(v["kind"], "number");
    assert_eq!((v["count"].clone(), v["nulls"].clone(), v["skipped"].clone()), (json!(3), json!(1), json!(1)));
    assert_eq!(v["sum"], 12.0);
    assert_eq!(v["mean"], 4.0);
    assert_eq!((v["min"].clone(), v["max"].clone()), (json!(2.0), json!(6.0)));
    // Population: sqrt(((2-4)^2 + 0 + (6-4)^2) / 3).
    assert!((v["stdDev"].as_f64().unwrap() - (8.0f64 / 3.0).sqrt()).abs() < 1e-12, "{v}");
    let said: Vec<&str> = s.notices_since(mark).iter().map(|n| n.text.as_str()).collect();
    assert_eq!(said, ["parcels: 1 value(s) in 'area' that aren't finite numbers (NaN, infinity or other text) left out of the statistics"]);
    // Without a layer, the active one (the added layer).
    s.active_layer = s.map().layer_by_name("parcels").map(|l| l.id);
    assert_eq!(stats(&mut s, json!({"field": "N"}))["sum"], 100.0);
}

#[test]
fn a_selection_limits_the_rows_and_a_clean_column_says_nothing() {
    let mut s = parcels();
    s.run("select.rows", &json!({"layer": "parcels", "rows": [0, 1, 1, 99], "mode": "new"})).unwrap();
    let mark = s.notice_mark();
    let v = stats(&mut s, json!({"layer": "parcels", "field": "n", "selected": true}));
    assert_eq!(
        (v["rows"].clone(), v["count"].clone(), v["sum"].clone(), v["mean"].clone()),
        (json!(2), json!(2), json!(30.0), json!(15.0))
    );
    assert_eq!(v["stdDev"], 5.0);
    assert_eq!(v["selected"], true);
    assert!(s.notices_since(mark).is_empty(), "a clean column gives no notice");
    // Nothing selected, selected only: no rows, no mean.
    s.run("select.clear", &Value::Null).unwrap();
    let v = stats(&mut s, json!({"layer": "parcels", "field": "n", "selected": true}));
    assert_eq!((v["rows"].clone(), v["count"].clone(), v["sum"].clone(), v["mean"].clone()), (json!(0), json!(0), json!(0.0), Value::Null));
}

#[test]
fn text_and_date_columns_count_distinct_and_order() {
    let mut s = parcels();
    let v = stats(&mut s, json!({"layer": "parcels", "field": "name"}));
    assert_eq!(v["kind"], "text");
    assert_eq!((v["count"].clone(), v["nulls"].clone(), v["distinct"].clone()), (json!(3), json!(2), json!(2)));
    assert_eq!((v["min"].clone(), v["max"].clone()), (json!("a"), json!("b")));
    assert!(v.get("sum").is_none() && v.get("mean").is_none());
    let v = stats(&mut s, json!({"layer": "parcels", "field": "built"}));
    assert_eq!(v["kind"], "date");
    assert_eq!((v["count"].clone(), v["nulls"].clone(), v["distinct"].clone()), (json!(3), json!(2), json!(3)));
    assert_eq!((v["min"].clone(), v["max"].clone()), (json!("1999-12-31"), json!("2026-01-01")));
}

#[test]
fn bad_requests_are_errors_and_nothing_is_recorded() {
    let mut s = parcels();
    let err = s.run("table.statistics", &json!({"layer": "parcels", "field": "nope"})).unwrap_err();
    assert!(err.contains("'nope'"), "{err}");
    assert!(s.run("table.statistics", &json!({"layer": "parcels"})).is_err());
    assert!(s.run("table.statistics", &json!({"layer": "missing", "field": "area"})).is_err());
    let (depth, dirty) = (s.undo_depth(), s.dirty());
    stats(&mut s, json!({"layer": "parcels", "field": "area"}));
    stats(&mut s, json!({"layer": "parcels", "field": "name", "selected": true}));
    assert_eq!((s.undo_depth(), s.dirty()), (depth, dirty), "read only: no undo step, project not marked changed");
}
