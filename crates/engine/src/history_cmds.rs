//! History panel actions beyond stepping (#1117): Delete (the trash button) and New Document (the
//! "Create new document from current state" button). Stepping to a state is Edit › Undo / Redo.
//!
//! States are indexed like [`photocraft_ops::History::entries`]: 0 is the oldest state still
//! held, the last index is the current document.
//!
//! Snapshots, as in Photoshop: the document as it was when one was taken, kept for the whole
//! session however far the history moves on or is trimmed (not saved with the file). The first is
//! taken when the document opens and is named after it; "Create new snapshot" adds "Snapshot 1",
//! "Snapshot 2", … of the full document, its merged layers or the current layer. Selecting one
//! brings the document back to it as one undoable step named after it (Photoshop drops the
//! later states instead; here they stay, and Undo returns). Snapshots are indexed oldest first.
//!
//! The History Brush (and the Eraser's Erase to History) paints from the source set with
//! `history.setSource`: a snapshot or a state, by default the first snapshot.

use std::sync::Arc;

use photocraft_doc::{Document, Layer, LayerContent};
use photocraft_ops::LayerTarget;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, int};
use crate::image_cmds::reidentify_copied_layers;
use crate::{DocState, EngineError, Result, Session};

/// A History panel snapshot.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub name: String,
    pub doc: Arc<Document>,
    /// The layers targeted when it was taken; selecting it targets them again.
    pub layers: LayerTarget,
}

/// The History Brush's source as chosen: a snapshot or a state, by the document it holds (indices
/// move as snapshots are deleted and old states trimmed). A snapshot taken at a state holds the
/// same document as the state, so the kind says which row was picked.
#[derive(Clone, Debug)]
pub enum Source {
    Snapshot(Arc<Document>),
    State(Arc<Document>),
}

/// Where the History Brush paints from: `{"snapshot": i}` or `{"state": i}` (the chosen source
/// while it is still held, else the first snapshot, else the oldest state), or `None` with no
/// document history at all.
fn source_ref(st: &DocState) -> Option<(bool, usize)> {
    let state_of = |d: &Arc<Document>| {
        let past = st.history.past_len();
        if Arc::ptr_eq(&st.doc, d) {
            return Some(past);
        }
        (0..past).find(|&i| st.history.state(i).is_some_and(|h| Arc::ptr_eq(&h, d)))
    };
    let chosen = match &st.history_source {
        Some(Source::Snapshot(d)) => st.snapshots.iter().position(|s| Arc::ptr_eq(&s.doc, d)).map(|i| (true, i)),
        Some(Source::State(d)) => state_of(d).map(|i| (false, i)),
        None => None,
    };
    chosen.or_else(|| (!st.snapshots.is_empty()).then_some((true, 0))).or(Some((false, 0)))
}

/// The History Brush's source document (see [`source_ref`]).
pub fn brush_source(st: &DocState) -> Arc<Document> {
    let past = st.history.past_len();
    match source_ref(st) {
        Some((true, i)) => st.snapshots.get(i).map(|s| s.doc.clone()),
        Some((false, i)) if i == past => Some(st.doc.clone()),
        Some((false, i)) => st.history.state(i),
        None => None,
    }
    .unwrap_or_else(|| st.doc.clone())
}

/// Where the History Brush paints from, for the History panel: `{"snapshot": i}`, `{"state": i}`.
pub fn source_json(st: &DocState) -> Value {
    match source_ref(st) {
        Some((true, i)) => json!({ "snapshot": i }),
        Some((false, i)) => json!({ "state": i }),
        None => Value::Null,
    }
}

/// The snapshot `history.selectSnapshot` went back to, while the document is as it left it.
pub fn selected_snapshot(st: &DocState) -> Option<usize> {
    st.selected_snapshot.filter(|(i, rev)| *rev == st.revision && *i < st.snapshots.len()).map(|(i, _)| i)
}

fn snapshot_index(s: &Session, cmd: &str, p: &Value) -> Result<usize> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let v = int(p, "snapshot").ok_or_else(|| bad(cmd, "missing `snapshot` index"))?;
    usize::try_from(v).ok().filter(|i| *i < st.snapshots.len()).ok_or_else(|| bad(cmd, format!("`snapshot` = {v} is not a snapshot (0..{})", st.snapshots.len())))
}

/// History panel › Create new snapshot (⌥-click names it and picks what it holds).
fn new_snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.newSnapshot";
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    let from = p.get("from").and_then(Value::as_str).unwrap_or("fullDocument");
    let doc = match from {
        "fullDocument" => st.doc.clone(),
        "mergedLayers" => {
            let mut d = (*st.doc).clone();
            let px = crate::file_cmds::flattened(&d, d.pixel_format());
            d.layers = vec![Layer::new("Background", LayerContent::Raster(px))];
            Arc::new(d)
        }
        "currentLayer" => {
            let id = st.active_layer.ok_or_else(|| bad(CMD, "no layer is selected"))?;
            let layer = st.doc.layer(id).cloned().ok_or(EngineError::NoLayer(id))?;
            let mut d = (*st.doc).clone();
            d.layers = vec![layer];
            Arc::new(d)
        }
        other => return Err(bad(CMD, format!("`from` must be fullDocument, mergedLayers or currentLayer, not {other:?}"))),
    };
    let name = match p.get("name").and_then(Value::as_str).map(str::trim) {
        Some(n) if !n.is_empty() => n.chars().take(255).collect(),
        _ => {
            // Snapshot 1, 2, …: one past the highest number used so far.
            let n = st.snapshots.iter().filter_map(|s| s.name.strip_prefix("Snapshot ")?.parse::<u64>().ok()).max().unwrap_or(0);
            format!("Snapshot {}", n.saturating_add(1))
        }
    };
    let layers = LayerTarget { active: st.active_layer, selected: st.selected_layers.clone() };
    st.snapshots.push(Snapshot { name: name.clone(), doc, layers });
    Ok(json!({ "snapshot": st.snapshots.len() - 1, "name": name }))
}

/// Click a snapshot: the document goes back to it, as one undoable step named after it.
fn select_snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.selectSnapshot";
    let i = snapshot_index(s, CMD, p)?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let snap = st.snapshots.get(i).cloned().ok_or_else(|| bad(CMD, "no such snapshot"))?;
    if snap.doc.id != st.doc.id {
        return Err(bad(CMD, "the snapshot belongs to another document"));
    }
    s.edit(&snap.name, |doc, active| {
        *doc = (*snap.doc).clone();
        *active = snap.layers.active.filter(|id| doc.layer(*id).is_some()).or_else(|| doc.top_layer());
        Ok(())
    })?;
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    // The step replaced the whole document: recomposite everything.
    st.last_damage = None;
    st.selected_snapshot = Some((i, st.revision));
    Ok(json!({ "snapshot": i, "name": snap.name }))
}

fn rename_snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.renameSnapshot";
    let i = snapshot_index(s, CMD, p)?;
    let name: String = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(CMD, "missing `name`"))?.chars().take(255).collect();
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    if let Some(snap) = st.snapshots.get_mut(i) {
        snap.name = name.clone();
    }
    Ok(json!({ "snapshot": i, "name": name }))
}

fn delete_snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.deleteSnapshot";
    let i = snapshot_index(s, CMD, p)?;
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    let gone = st.snapshots.remove(i);
    if matches!(&st.history_source, Some(Source::Snapshot(d)) if Arc::ptr_eq(d, &gone.doc)) {
        st.history_source = None;
    }
    st.selected_snapshot = match st.selected_snapshot {
        Some((k, _)) if k == i => None,
        Some((k, rev)) if k > i => Some((k - 1, rev)),
        other => other,
    };
    Ok(json!({ "snapshots": st.snapshots.len() }))
}

/// The History Brush's source: `{"snapshot": i}` or `{"state": i}` (a History entry, 0 = oldest,
/// up to the current state).
fn set_source(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.setSource";
    let source = if p.get("snapshot").is_some() {
        let i = snapshot_index(s, CMD, p)?;
        s.active().and_then(|st| st.snapshots.get(i)).map(|snap| Source::Snapshot(snap.doc.clone()))
    } else if p.get("state").is_some() {
        let (i, current) = state_index(s, CMD, p)?;
        s.active().and_then(|st| if i == current { Some(st.doc.clone()) } else { st.history.state(i) }).map(Source::State)
    } else {
        return Err(bad(CMD, "give `snapshot` or `state`"));
    };
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    st.history_source = Some(source.ok_or_else(|| bad(CMD, "that state is not held"))?);
    Ok(source_json(st))
}

/// The snapshots, which one is selected, and the History Brush's source.
fn list_snapshots(s: &mut Session, _: &Value) -> Result<Value> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let names: Vec<&str> = st.snapshots.iter().map(|s| s.name.as_str()).collect();
    Ok(json!({ "snapshots": names, "selected": selected_snapshot(st), "source": source_json(st) }))
}

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

/// The `state` param as an index into the active document's history entries, defaulting to the
/// current state.
fn state_index(s: &Session, cmd: &str, p: &Value) -> Result<(usize, usize)> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let current = st.history.past_len();
    let i = match p.get("state") {
        None | Some(Value::Null) => current,
        Some(_) => {
            let v = int(p, "state").ok_or_else(|| bad(cmd, "`state` must be a number"))?;
            usize::try_from(v).ok().filter(|i| *i <= current).ok_or_else(|| bad(cmd, format!("`state` = {v} is not a state (0..={current})")))?
        }
    };
    Ok((i, current))
}

/// History panel › Delete: drop the state and every state after it, as Photoshop does in linear
/// history. The document returns to the state before it, and the deleted steps can't be redone.
/// The oldest state (the document as opened) can't be deleted.
fn delete_state(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.deleteState";
    let (i, current) = state_index(s, CMD, p)?;
    if i == 0 {
        return Err(bad(CMD, "the oldest state can't be deleted"));
    }
    // Step back to the state before `i`; the steps from `i` on are then redo states.
    for _ in i..=current {
        if !s.undo() {
            return Err(EngineError::Other("the history can't step back right now".into()));
        }
    }
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    st.history.clear_redo();
    let entries = st.history.entries();
    Ok(json!({ "states": entries.len(), "current": entries.last() }))
}

/// History panel › New Document: a new untitled document holding the document as it was at a
/// state (the current one by default), named after that state, with a history of its own.
fn new_document(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "history.newDocument";
    let (src, label) = if p.get("snapshot").is_some() {
        let k = snapshot_index(s, CMD, p)?;
        let snap = s.active().and_then(|st| st.snapshots.get(k)).ok_or_else(|| bad(CMD, "no such snapshot"))?;
        (snap.doc.clone(), snap.name.clone())
    } else {
        let (i, current) = state_index(s, CMD, p)?;
        let st = s.active().ok_or(EngineError::NoDocument)?;
        let src = if i == current { Some(st.doc.clone()) } else { st.history.state(i) };
        let src = src.ok_or_else(|| bad(CMD, format!("state {i} is not held")))?;
        (src, st.history.entries().get(i).cloned().unwrap_or_default())
    };
    let mut doc = (*src).clone();
    doc.id = photocraft_doc::DocId::fresh();
    doc.name = match p.get("name").and_then(Value::as_str) {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ if !label.trim().is_empty() => label,
        _ => format!("{} copy", doc.name),
    };
    reidentify_copied_layers(&mut doc);
    let name = doc.name.clone();
    let index = s.add_document(doc, None);
    Ok(json!({ "document": index, "name": name }))
}

fn can_delete(s: &Session) -> std::result::Result<(), String> {
    s.active().filter(|d| d.history.can_undo()).map(|_| ()).ok_or_else(|| "the oldest state can't be deleted".into())
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "history.deleteState",
            label: "Delete State",
            menu: &[],
            shortcut: None,
            params: r##"{"state":index?} (History entry, 0 = oldest; default the current state) → {states,current}: removes the state and every later one"##,
            enabled: can_delete,
            run: delete_state,
            journal: true,
        },
        CommandSpec {
            id: "history.newDocument",
            label: "New Document from State",
            menu: &[],
            shortcut: None,
            params: r##"{"state":index?,"snapshot":index?,"name":str?} (History entry, 0 = oldest; default the current state; or a snapshot) → {document,name}"##,
            enabled: has_doc,
            run: new_document,
            journal: true,
        },
        CommandSpec {
            id: "history.newSnapshot",
            label: "New Snapshot",
            menu: &[],
            shortcut: None,
            params: r##"{"name":str?,"from":"fullDocument|mergedLayers|currentLayer"="fullDocument"} (default name Snapshot 1, 2, …) → {snapshot,name}"##,
            enabled: has_doc,
            run: new_snapshot,
            journal: true,
        },
        CommandSpec {
            id: "history.selectSnapshot",
            label: "Select Snapshot",
            menu: &[],
            shortcut: None,
            params: r##"{"snapshot":index} (0 = the opening snapshot) → {snapshot,name}: the document goes back to it, one undoable step"##,
            enabled: has_doc,
            run: select_snapshot,
            journal: true,
        },
        CommandSpec {
            id: "history.renameSnapshot",
            label: "Rename Snapshot",
            menu: &[],
            shortcut: None,
            params: r##"{"snapshot":index,"name":str} → {snapshot,name}"##,
            enabled: has_doc,
            run: rename_snapshot,
            journal: true,
        },
        CommandSpec {
            id: "history.deleteSnapshot",
            label: "Delete Snapshot",
            menu: &[],
            shortcut: None,
            params: r##"{"snapshot":index} → {snapshots}"##,
            enabled: has_doc,
            run: delete_snapshot,
            journal: true,
        },
        CommandSpec {
            id: "history.setSource",
            label: "Set History Brush Source",
            menu: &[],
            shortcut: None,
            params: r##"{"snapshot":index} | {"state":index} (History entry, 0 = oldest, up to the current state) → {snapshot}|{state}: what the History Brush and Erase to History paint from"##,
            enabled: has_doc,
            run: set_source,
            journal: true,
        },
        CommandSpec {
            id: "history.snapshots",
            label: "List Snapshots",
            menu: &[],
            shortcut: None,
            params: r##"{} → {snapshots:[name…],selected:index|null,source:{snapshot}|{state}}"##,
            enabled: has_doc,
            run: list_snapshots,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document with three steps after Open: entries Open, New Layer, Fill, Fill.
    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 20, "height": 10})).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
        s.execute("edit.fill", json!({"color": "#00ff00"})).unwrap();
        s
    }

    fn entries(s: &Session) -> Vec<String> {
        s.active().unwrap().history.entries()
    }

    #[test]
    fn delete_current_state_steps_back_and_drops_it() {
        let mut s = session();
        let before = entries(&s);
        let prev_doc = s.active().unwrap().history.state(before.len() - 2).unwrap();
        let r = s.execute("history.deleteState", json!({})).unwrap();
        assert_eq!(r["states"], json!(before.len() - 1));
        assert_eq!(entries(&s), before[..before.len() - 1].to_vec());
        assert!(std::sync::Arc::ptr_eq(&s.active().unwrap().doc, &prev_doc), "the document is the state before");
        assert!(!s.active().unwrap().history.can_redo(), "the deleted state can't be redone");
        assert!(s.execute("edit.redo", json!({})).is_err());
    }

    #[test]
    fn delete_an_earlier_state_drops_it_and_every_later_one() {
        let mut s = session();
        let first = s.active().unwrap().history.state(0).unwrap();
        s.execute("history.deleteState", json!({"state": 1})).unwrap();
        assert_eq!(entries(&s), vec!["Open".to_string()]);
        assert!(std::sync::Arc::ptr_eq(&s.active().unwrap().doc, &first));
        assert!(!s.active().unwrap().history.can_redo());
        // Redo states after an undo go too.
        let mut s = session();
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("history.deleteState", json!({})).unwrap();
        assert_eq!(entries(&s).len(), 2);
        assert!(!s.active().unwrap().history.can_redo());
    }

    #[test]
    fn delete_state_refuses_the_oldest_state_and_bad_params() {
        let mut s = Session::new();
        assert!(s.execute("history.deleteState", json!({})).is_err(), "no document");
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        assert!(!s.is_enabled("history.deleteState"), "only the opening state");
        assert!(s.execute("history.deleteState", json!({})).is_err());
        let mut s = session();
        let before = entries(&s);
        for p in [json!({"state": 0}), json!({"state": -1}), json!({"state": 99}), json!({"state": "x"}), json!({"state": f64::MAX}), json!({"state": [1]})] {
            assert!(s.execute("history.deleteState", p.clone()).is_err(), "{p}");
            assert_eq!(entries(&s), before, "{p} changed nothing");
        }
    }

    #[test]
    fn new_document_from_a_state_copies_it_into_a_fresh_document() {
        let mut s = session();
        let src = s.active().unwrap().doc.clone();
        let label = entries(&s).last().cloned().unwrap();
        let r = s.execute("history.newDocument", json!({})).unwrap();
        assert_eq!(s.documents().len(), 2);
        assert_eq!(r["document"], json!(1));
        assert_eq!(r["name"], json!(label), "named after the state");
        let new = s.active().unwrap();
        assert_eq!(new.doc.size, src.size);
        assert_eq!(new.doc.layers.len(), src.layers.len());
        assert_ne!(new.doc.id, src.id);
        assert_eq!(new.history.entries().len(), 1, "a fresh history");
        // An earlier state, with a chosen name; the source document is untouched.
        s.set_active(0);
        let before = entries(&s);
        let open = s.active().unwrap().history.state(0).unwrap();
        s.execute("history.newDocument", json!({"state": 0, "name": "Start"})).unwrap();
        let new = s.active().unwrap();
        assert_eq!(new.doc.name, "Start");
        assert_eq!(new.doc.layers.len(), open.layers.len());
        s.set_active(0);
        assert_eq!(entries(&s), before);
    }

    #[test]
    fn new_document_refuses_bad_params() {
        let mut s = Session::new();
        assert!(s.execute("history.newDocument", json!({})).is_err(), "no document");
        let mut s = session();
        for p in [json!({"state": -1}), json!({"state": 99}), json!({"state": "x"}), json!({"state": f64::NAN.to_string()})] {
            assert!(s.execute("history.newDocument", p.clone()).is_err(), "{p}");
        }
        assert_eq!(s.documents().len(), 1);
    }

    fn px(s: &Session) -> [f32; 4] {
        let st = s.active().unwrap();
        st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().rgba(5, 5)
    }

    fn names(s: &mut Session) -> Value {
        s.execute("history.snapshots", json!({})).unwrap()
    }

    /// A white layer named "Paint" on a new document, then a red fill and a green fill.
    fn painted() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 20, "height": 10, "name": "Doc"})).unwrap();
        s.execute("layer.new.layer", json!({"name": "Paint"})).unwrap();
        s.execute("edit.fill", json!({"color": "#ffffff"})).unwrap();
        s
    }

    #[test]
    fn the_opening_snapshot_exists_and_new_ones_are_numbered() {
        let mut s = painted();
        let r = names(&mut s);
        assert_eq!(r["snapshots"], json!(["Doc"]), "taken when the document opened, named after it");
        assert_eq!((r["selected"].clone(), r["source"].clone()), (Value::Null, json!({"snapshot": 0})));
        let steps = s.active().unwrap().history.entries().len();
        assert_eq!(s.execute("history.newSnapshot", json!({})).unwrap(), json!({"snapshot": 1, "name": "Snapshot 1"}));
        assert_eq!(s.execute("history.newSnapshot", json!({"name": "  Kept  "})).unwrap()["name"], json!("Kept"));
        assert_eq!(s.execute("history.newSnapshot", json!({})).unwrap()["name"], json!("Snapshot 2"));
        assert_eq!(s.active().unwrap().history.entries().len(), steps, "taking a snapshot is not a history step");
        // Merged Layers holds one flattened layer; Current Layer only the active layer.
        s.execute("history.newSnapshot", json!({"from": "mergedLayers"})).unwrap();
        s.execute("history.newSnapshot", json!({"from": "currentLayer"})).unwrap();
        let st = s.active().unwrap();
        assert_eq!((st.doc.layers.len(), st.snapshots[4].doc.layers.len(), st.snapshots[5].doc.layers.len()), (2, 1, 1));
        assert_eq!(st.snapshots[5].doc.layers[0].name, "Paint");
        assert!(s.execute("history.newSnapshot", json!({"from": "everything"})).is_err());
    }

    #[test]
    fn selecting_a_snapshot_goes_back_to_it_in_one_undoable_step() {
        let mut s = painted();
        s.execute("history.newSnapshot", json!({})).unwrap();
        s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("history.selectSnapshot", json!({"snapshot": 1})).unwrap();
        let st = s.active().unwrap();
        assert_eq!((st.doc.layers.len(), st.history.entries().last().cloned()), (2, Some("Snapshot 1".into())));
        assert_eq!(st.doc.layer(st.active_layer.unwrap()).unwrap().name, "Paint", "the snapshot's layer is targeted again");
        assert_eq!(px(&s), [1.0; 4]);
        assert_eq!(names(&mut s)["selected"], json!(1));
        // The opening snapshot: no Paint layer yet.
        s.execute("history.selectSnapshot", json!({"snapshot": 0})).unwrap();
        assert_eq!(s.active().unwrap().doc.layers.len(), 1);
        assert!(s.undo());
        assert_eq!(s.active().unwrap().doc.layers.len(), 2);
        assert_eq!(names(&mut s)["selected"], Value::Null, "undo leaves the snapshot");
        assert!(s.undo());
        assert_eq!(s.active().unwrap().doc.layers.len(), 3, "the later states are still there");
        for p in [json!({}), json!({"snapshot": 9}), json!({"snapshot": -1})] {
            assert!(s.execute("history.selectSnapshot", p.clone()).is_err(), "{p}");
        }
    }

    #[test]
    fn snapshots_outlive_a_trimmed_history_and_feed_the_history_brush() {
        let mut s = painted();
        s.active_mut().unwrap().history.max_states = 2;
        s.execute("history.newSnapshot", json!({})).unwrap();
        for c in ["#ff0000", "#00ff00", "#0000ff", "#000000"] {
            s.execute("edit.fill", json!({"color": c})).unwrap();
        }
        assert_eq!(s.active().unwrap().history.entries().len(), 3, "the oldest states were trimmed");
        // Fill › History paints from the source: Snapshot 1's white once chosen.
        s.execute("history.setSource", json!({"snapshot": 1})).unwrap();
        s.execute("edit.fill", json!({"contents": "history"})).unwrap();
        assert_eq!(px(&s), [1.0; 4]);
        // A state as the source; the History Brush also takes a snapshot directly.
        let past = s.active().unwrap().history.past_len();
        assert_eq!(s.execute("history.setSource", json!({"state": past - 1})).unwrap(), json!({"state": past - 1}));
        s.execute("edit.fill", json!({"contents": "history"})).unwrap();
        assert_eq!(px(&s), [0.0, 0.0, 0.0, 1.0], "the state before the last fill was black");
        s.execute("paint.historyBrush", json!({"points": [[5, 5], [6, 5]], "size": 9, "hardness": 100, "snapshot": 1})).unwrap();
        assert_eq!(px(&s), [1.0; 4]);
        assert!(s.execute("history.setSource", json!({})).is_err());
        assert!(s.execute("history.setSource", json!({"state": 99})).is_err());
    }

    #[test]
    fn snapshots_rename_delete_and_make_documents() {
        let mut s = painted();
        s.execute("history.newSnapshot", json!({})).unwrap();
        s.execute("history.renameSnapshot", json!({"snapshot": 1, "name": "Before"})).unwrap();
        s.execute("history.setSource", json!({"snapshot": 1})).unwrap();
        assert!(s.execute("history.renameSnapshot", json!({"snapshot": 1, "name": " "})).is_err());
        let r = s.execute("history.newDocument", json!({"snapshot": 1})).unwrap();
        assert_eq!((r["document"].clone(), r["name"].clone()), (json!(1), json!("Before")));
        s.set_active(0);
        s.execute("history.deleteSnapshot", json!({"snapshot": 1})).unwrap();
        let r = names(&mut s);
        assert_eq!((r["snapshots"].clone(), r["source"].clone()), (json!(["Doc"]), json!({"snapshot": 0})), "the source falls back to the opening snapshot");
        s.execute("history.deleteSnapshot", json!({"snapshot": 0})).unwrap();
        assert_eq!(names(&mut s)["snapshots"], json!([]));
        assert!(s.execute("history.deleteSnapshot", json!({"snapshot": 0})).is_err());
        // With no snapshot left the History Brush paints from the oldest state.
        assert_eq!(names(&mut s)["source"], json!({"state": 0}));
    }

    #[test]
    fn a_state_and_a_snapshot_of_it_are_different_sources() {
        let mut s = painted();
        s.execute("history.newSnapshot", json!({})).unwrap();
        let past = s.active().unwrap().history.past_len();
        // Snapshot 1 holds the very document of the current state; picking the state picks the state.
        assert_eq!(s.execute("history.setSource", json!({"state": past})).unwrap(), json!({"state": past}));
        s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
        assert_eq!(names(&mut s)["source"], json!({"state": past}), "the state keeps its index as later steps are added");
        assert_eq!(s.execute("history.setSource", json!({"snapshot": 1})).unwrap(), json!({"snapshot": 1}));
    }
}
