//! Select By Attributes' count of the rows a clause matches (#2946). Counting is a pass over every
//! row of the layer, so it is not done in each frame: once per clause and set of rows, on a thread
//! of its own on the desktop (the browser has none, so it counts inline, still once per change),
//! with the dialog saying "Counting matching rows…" until it lands.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use rustgis_engine::data::raster::Crs;
use rustgis_engine::data::{Dataset, Feature, query};

use crate::Slot;

/// How many rows are counted between two looks at the stop flag.
const CHUNK: usize = 4096;

/// What the dialog shows under the clause.
#[derive(Debug, PartialEq)]
pub(crate) enum Shown {
    /// The count is on its way.
    Counting,
    /// How many rows match.
    Rows(usize),
    /// Why the clause doesn't read.
    Invalid(String),
}

/// What a count was made for: it holds only while the same layer has the same rows and fields. The
/// rows are held by their allocation, which any edit, reload, Undo or next view of a streamed
/// layer ends ([`rustgis_engine::data::Features::allocation_key`]).
struct Key {
    layer: String,
    text: String,
    rows: Weak<Vec<Feature>>,
    fields: u64,
    windowed: bool,
    held_in: Crs,
}

impl Key {
    fn is(&self, text: &str, layer: &str, ds: &Dataset) -> bool {
        self.text == text
            && self.layer == layer
            && self.windowed == ds.windowed
            && self.held_in == ds.held_in
            && self.rows.ptr_eq(&ds.features.allocation_key())
            && self.fields == crate::exprcomplete::fields_key(ds)
    }
}

enum State {
    Counting(Slot<Result<usize, String>>),
    Done(Result<usize, String>),
}

/// The count between frames, held by the dialog (`Dialogs::matching`). Dropping it (the dialog
/// closed or another opened) stops a count still running.
#[derive(Default)]
pub struct Count {
    key: Option<Key>,
    state: Option<State>,
    stop: Arc<AtomicBool>,
    /// How many counts were started (a clause that doesn't read starts none): once per change, not
    /// once per frame.
    #[cfg(test)]
    pub(crate) starts: usize,
}

impl Drop for Count {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Count {
    /// What to show for `text` over layer `layer`'s rows `ds`, each frame it is asked for. A new
    /// clause, layer or set of rows starts a count (and stops the one before); the answer is kept
    /// while they stay as they were. `off_frame`: count on a thread of its own, else here.
    pub(crate) fn of(&mut self, ctx: &egui::Context, text: &str, layer: &str, ds: &Dataset, off_frame: bool) -> Shown {
        let current = self.key.as_ref().is_some_and(|k| k.is(text, layer, ds));
        if !current {
            self.start(text, layer, ds, off_frame);
        } else if let Some(State::Counting(slot)) = &self.state {
            let landed = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(r) = landed {
                self.state = Some(State::Done(r));
            }
        }
        match &self.state {
            Some(State::Done(Ok(n))) => Shown::Rows(*n),
            Some(State::Done(Err(e))) => Shown::Invalid(e.clone()),
            _ => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                Shown::Counting
            }
        }
    }

    fn start(&mut self, text: &str, layer: &str, ds: &Dataset, off_frame: bool) {
        // Whatever was counting is of a clause or rows no longer asked about.
        self.stop.store(true, Ordering::Relaxed);
        self.stop = Arc::default();
        self.key = Some(Key {
            layer: layer.to_string(),
            text: text.to_string(),
            rows: ds.features.allocation_key(),
            fields: crate::exprcomplete::fields_key(ds),
            windowed: ds.windowed,
            held_in: ds.held_in,
        });
        // The words and fields only: no geometry is read (`parse` reads them all for `$area`), so
        // a clause that doesn't read is said at once and costs nothing. The full `parse` is the
        // count's first step, so what it refuses is said as it always was.
        if let Err(e) = query::parse_fields(text, ds) {
            self.state = Some(State::Done(Err(e)));
            return;
        }
        #[cfg(test)]
        {
            self.starts += 1;
        }
        if !off_frame {
            self.state = Some(State::Done(count_inline(text, ds)));
            return;
        }
        let slot: Slot<Result<usize, String>> = Slot::default();
        let land = slot.clone();
        let (text, ds, stop) = (text.to_string(), ds.clone(), self.stop.clone());
        crate::work_off_frame("rustgis-select-count", &slot, move || {
            if let Some(counted) = count(&text, &ds, &stop) {
                *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(counted);
            }
        });
        self.state = Some(State::Counting(slot));
    }

    /// Whether a count is on its way.
    #[cfg(test)]
    pub(crate) fn counting(&self) -> bool {
        matches!(self.state, Some(State::Counting(_)))
    }
}

/// How many rows `text` matches, as `query::select(text, ds)` has them, all at once (the
/// browser's, on its one thread).
fn count_inline(text: &str, ds: &Dataset) -> Result<usize, String> {
    let e = query::parse(text, ds)?;
    Ok((0..ds.features.len()).filter(|r| query::eval(&e, ds, *r)).count())
}

/// [`count_inline`] on a thread of its own: looks at `stop` between chunks of [`CHUNK`] rows, and
/// gives nothing once it is set. One row's evaluation is not interrupted.
fn count(text: &str, ds: &Dataset, stop: &AtomicBool) -> Option<Result<usize, String>> {
    let e = match query::parse(text, ds) {
        Ok(e) => e,
        Err(e) => return Some(Err(e)),
    };
    let rows = ds.features.len();
    let mut n = 0;
    let mut from = 0;
    while from < rows {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let to = from.saturating_add(CHUNK).min(rows);
        n += (from..to).filter(|r| query::eval(&e, ds, *r)).count();
        from = to;
    }
    Some(Ok(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_engine::data::{Field, FieldType, GeometryType, Value};

    /// `rows` points with an integer field `n` counting up from 0.
    fn layer(rows: usize) -> Dataset {
        let mut ds = Dataset::new("t", GeometryType::Point, vec![Field::new("n", FieldType::Integer)]);
        for i in 0..rows {
            ds.push(None, vec![Value::Int(i as i64)]);
        }
        ds
    }

    /// The count is the number of rows `select` gives, and a clause that doesn't read gives the
    /// error `select` gives.
    #[test]
    fn the_count_is_what_select_gives() {
        let ds = layer(10_000);
        let never = AtomicBool::new(false);
        for (clause, expected) in [("n >= 9000", 1000), ("n < 0", 0), ("n >= 0", 10_000), ("n IN (1, 2, 3)", 3), ("n IS NULL", 0)] {
            assert_eq!(count(clause, &ds, &never), Some(Ok(expected)), "{clause}");
            assert_eq!(count_inline(clause, &ds), Ok(expected), "{clause} inline");
            assert_eq!(query::select(clause, &ds).map(|r| r.len()), Ok(expected), "{clause} by select");
        }
        for bad in ["nope = 1", "", "n >"] {
            assert_eq!(count(bad, &ds, &never), Some(query::select(bad, &ds).map(|r| r.len())), "{bad:?}");
            assert_eq!(count_inline(bad, &ds), query::select(bad, &ds).map(|r| r.len()), "{bad:?} inline");
        }
        assert_eq!(count_inline("nope = 1", &ds), Err("no field named 'nope'".to_string()));
    }

    /// A stop that is set leaves no answer.
    #[test]
    fn a_stopped_count_gives_nothing() {
        let ds = layer(3 * CHUNK);
        assert_eq!(count("n >= 0", &ds, &AtomicBool::new(true)), None);
        assert_eq!(count("n >= 0", &ds, &AtomicBool::new(false)), Some(Ok(3 * CHUNK)));
    }

    /// The browser's path counts at once, on the first ask, and starts no thread.
    #[test]
    fn the_browser_counts_inline() {
        let ds = layer(100);
        let mut c = Count::default();
        let ctx = egui::Context::default();
        assert_eq!(c.of(&ctx, "n >= 90", "t", &ds, false), Shown::Rows(10));
        assert!(!c.counting());
        assert_eq!(c.starts, 1);
        assert_eq!(c.of(&ctx, "n >= 90", "t", &ds, false), Shown::Rows(10), "kept");
        assert_eq!(c.starts, 1, "not counted again");
    }

    /// Dropping the count stops one on its way.
    #[test]
    fn dropping_the_count_stops_it() {
        let c = Count::default();
        let stop = c.stop.clone();
        assert!(!stop.load(Ordering::Relaxed));
        drop(c);
        assert!(stop.load(Ordering::Relaxed));
    }

    /// Edited rows, another clause or another layer's name are counted again, not answered with
    /// the number for the rows before.
    #[test]
    fn rows_clause_and_layer_changed_are_counted_again() {
        let mut ds = layer(100);
        let mut c = Count::default();
        let ctx = egui::Context::default();
        assert_eq!(c.of(&ctx, "n >= 90", "t", &ds, false), Shown::Rows(10));
        ds.push(None, vec![Value::Int(500)]);
        assert_eq!(c.of(&ctx, "n >= 90", "t", &ds, false), Shown::Rows(11), "a row added");
        assert_eq!(c.of(&ctx, "n >= 95", "t", &ds, false), Shown::Rows(6), "another clause");
        assert_eq!(c.of(&ctx, "n >= 95", "u", &ds, false), Shown::Rows(6), "another layer");
        assert_eq!(c.starts, 4);
        assert_eq!(
            c.of(&ctx, "n >", "u", &ds, false),
            Shown::Invalid("expected a value, found the end".into()),
            "no count for a clause that doesn't read"
        );
        assert_eq!(c.starts, 4);
    }
}
