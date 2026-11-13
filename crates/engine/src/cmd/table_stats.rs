//! `table.statistics`: the quick numbers of one column of a layer's attribute table, without an
//! output table (Summary Statistics writes one).
//!
//! One pass over the rows, nothing copied but the distinct texts of a text column. The sum, mean,
//! minimum and maximum are the ones Summary Statistics gives; the standard deviation is the
//! population one (divided by n), as its STD is.

use std::borrow::Cow;
use std::collections::HashSet;

use rustgis_data::{Dataset, Value as Cell};
use serde_json::{Value, json};

use super::{b, ps};
use crate::{CmdResult, Session};

/// The statistics of a numeric column, gathered as the rows go by.
#[derive(Default)]
struct Numbers {
    count: usize,
    nulls: usize,
    skipped: usize,
    sum: f64,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
}

impl Numbers {
    fn add(&mut self, x: f64) {
        self.count += 1;
        self.sum += x;
        // Welford's running mean and sum of squares, so one pass gives the variance.
        let d = x - self.mean;
        self.mean += d / self.count as f64;
        self.m2 += d * (x - self.mean);
        if self.count == 1 {
            self.min = x;
            self.max = x;
        } else {
            self.min = self.min.min(x);
            self.max = self.max.max(x);
        }
    }
}

/// The statistics of a text, date or other column.
#[derive(Default)]
struct Texts {
    count: usize,
    nulls: usize,
    distinct: HashSet<String>,
    min: Option<String>,
    max: Option<String>,
    /// For a date column: the earliest and latest by instant.
    earliest: Option<(i64, String)>,
    latest: Option<(i64, String)>,
}

/// The selected rows, each once, leaving out numbers past the layer's last row; empty without a
/// selection.
fn covered(ds: &Dataset, selection: Option<&[usize]>) -> Vec<usize> {
    match selection {
        None => Vec::new(),
        Some(sel) => {
            let mut rows: Vec<usize> = sel.iter().copied().filter(|&r| r < ds.features.len()).collect();
            rows.sort_unstable();
            rows.dedup();
            rows
        }
    }
}

fn is_empty_text(v: &Cell) -> bool {
    matches!(v, Cell::Text(t) if t.trim().is_empty())
}

pub(super) fn run(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let selected = b(p, "selected").unwrap_or(false);
    let field = ps(p, "field").map(str::trim).filter(|f| !f.is_empty()).ok_or("give the field to describe (\"field\")")?;
    let (answer, notice) = {
        let l = s.map().layer(id).ok_or("no such layer")?;
        if l.is_raster() || l.graphics {
            return Err(format!("{}: statistics are for the fields of a feature layer", l.name));
        }
        let ds = &l.dataset;
        if ds.windowed {
            return Err(format!("{}: load all its features first (Layers › Load All Features), so the statistics cover them all", l.name));
        }
        let fi = ds.field_index(field).ok_or_else(|| format!("{}: no field named '{field}'", l.name))?;
        let f = ds.fields.get(fi).ok_or_else(|| format!("{}: no field named '{field}'", l.name))?;
        let picked = covered(ds, selected.then_some(l.selection.as_slice()));
        let rows = if selected { picked.len() } else { ds.features.len() };
        let rows_at = |i: usize| if selected { picked.get(i).copied() } else { Some(i) };
        let mut out = json!({"layer": l.name, "field": f.name, "selected": selected, "rows": rows});
        let mut notice = None;
        let map = out.as_object_mut().ok_or("no answer")?;
        if f.ty.is_numeric() {
            let mut n = Numbers::default();
            for i in 0..rows {
                let Some(r) = rows_at(i) else { continue };
                match ds.value(r, fi) {
                    Cell::Null => n.nulls += 1,
                    v if is_empty_text(v) => n.nulls += 1,
                    v => match v.as_f64() {
                        Some(x) => n.add(x),
                        None => n.skipped += 1,
                    },
                }
            }
            let some = n.count > 0;
            let pick = |x: f64| if some { json!(x) } else { Value::Null };
            map.insert("kind".into(), json!("number"));
            map.insert("count".into(), json!(n.count));
            map.insert("nulls".into(), json!(n.nulls));
            map.insert("skipped".into(), json!(n.skipped));
            map.insert("sum".into(), json!(n.sum));
            map.insert("mean".into(), pick(n.sum / n.count.max(1) as f64));
            map.insert("min".into(), pick(n.min));
            map.insert("max".into(), pick(n.max));
            map.insert("stdDev".into(), pick((n.m2 / n.count.max(1) as f64).sqrt()));
            if n.skipped > 0 {
                notice = Some(format!(
                    "{}: {} value(s) in '{}' that aren't finite numbers (NaN, infinity or other text) left out of the statistics",
                    l.name, n.skipped, f.name
                ));
            }
        } else {
            let temporal = f.ty.is_temporal();
            let mut t = Texts::default();
            for i in 0..rows {
                let Some(r) = rows_at(i) else { continue };
                let v = ds.value(r, fi);
                if v.is_null() || is_empty_text(v) {
                    t.nulls += 1;
                    continue;
                }
                t.count += 1;
                let text: Cow<str> = match v {
                    Cell::Text(x) => Cow::Borrowed(x.as_str()),
                    other => Cow::Owned(other.to_string()),
                };
                if !t.distinct.contains(text.as_ref()) {
                    t.distinct.insert(text.to_string());
                }
                if temporal && let Some(k) = v.temporal_key() {
                    if t.earliest.as_ref().is_none_or(|(e, _)| k < *e) {
                        t.earliest = Some((k, text.to_string()));
                    }
                    if t.latest.as_ref().is_none_or(|(e, _)| k > *e) {
                        t.latest = Some((k, text.to_string()));
                    }
                } else {
                    if t.min.as_deref().is_none_or(|m| text.as_ref() < m) {
                        t.min = Some(text.to_string());
                    }
                    if t.max.as_deref().is_none_or(|m| text.as_ref() > m) {
                        t.max = Some(text.to_string());
                    }
                }
            }
            let (min, max) = if temporal { (t.earliest.map(|e| e.1), t.latest.map(|e| e.1)) } else { (t.min, t.max) };
            map.insert("kind".into(), json!(if temporal { "date" } else { "text" }));
            map.insert("count".into(), json!(t.count));
            map.insert("nulls".into(), json!(t.nulls));
            map.insert("distinct".into(), json!(t.distinct.len()));
            map.insert("min".into(), json!(min));
            map.insert("max".into(), json!(max));
        }
        (out, notice)
    };
    if let Some(n) = notice {
        s.notify(n, false);
    }
    Ok(answer)
}
