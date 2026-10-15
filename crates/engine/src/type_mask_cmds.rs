//! Horizontal and Vertical Type Mask tools, as in Photoshop: the text is typed and styled like
//! type, then becomes a selection instead of a layer. The UI types into an invisible type layer
//! (`type.create` with `"mask": true`, opacity 0, so every type edit works on it) under one
//! coalescing key; `type.maskToSelection` ends that gesture: the layer's coverage becomes the
//! selection and the layer goes, so the whole gesture is one "Type Mask" history step that leaves
//! only the new selection behind.

use photocraft_algo::selection::{self as sel, SelectionMode};
use photocraft_doc::{LayerContent, LayerId};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// The History step a committed type mask shows as.
pub const LABEL: &str = "Type Mask";

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn to_selection(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "type.maskToSelection";
    let id = LayerId(p.get("layer").and_then(Value::as_u64).ok_or_else(|| bad(CMD, "missing `layer`"))?);
    let activate = p.get("activate").and_then(Value::as_u64).map(LayerId);
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let l = st.doc.layer(id).ok_or(EngineError::NoLayer(id))?;
    if !matches!(l.content, LayerContent::Text(_)) {
        return Err(bad(CMD, "`layer` is not a type layer"));
    }
    let selected = s.edit(LABEL, |doc, active| {
        let area = doc.bounds();
        let mut px = vec![[0.0f32; 4]; area.width() as usize * area.height() as usize];
        if let Some(surface) = doc.layer(id).and_then(|l| l.surface()) {
            surface.read_rgba_into(area, &mut px);
        }
        let plane: Vec<f32> = px.iter().map(|p| p[3]).collect();
        doc.selection = sel::combine(None, &plane, area, SelectionMode::Replace);
        doc.remove(id);
        *active = activate.filter(|a| doc.layer(*a).is_some());
        Ok(doc.selection.is_some())
    })?;
    if let Some(st) = s.active_mut() {
        st.history.relabel_current(LABEL);
    }
    Ok(json!({ "selected": selected }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "type.maskToSelection",
        label: LABEL,
        menu: &[],
        shortcut: None,
        params: r##"{"layer":id (the Type Mask tools' type layer),"activate":id? (the layer active again afterwards)} → {selected} (the text's coverage replaces the selection; the layer is removed; the step is "Type Mask")"##,
        enabled: has_doc,
        run: to_selection,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_typed_text_becomes_the_selection_in_one_type_mask_step() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 200, "height": 80, "background": "white"})).unwrap();
        s.execute("select.rect", json!({"x": 0, "y": 0, "width": 5, "height": 5})).unwrap();
        let bg = s.active().unwrap().active_layer;
        let steps = s.active().unwrap().history.entries().len();
        let before = s.active().unwrap().doc.clone();
        let key = json!("mask-test");
        let id = s.execute("type.create", json!({"x": 10, "y": 60, "text": "Hi", "size": 40, "mask": true, "coalesce": key})).unwrap()["layer"].as_u64().unwrap();
        assert_eq!(s.active().unwrap().doc.layer(LayerId(id)).unwrap().opacity, 0.0, "the mask's type layer is invisible");
        s.execute("type.edit", json!({"layer": id, "text": "HI", "coalesce": key})).unwrap();
        let r = s.execute("type.maskToSelection", json!({"layer": id, "activate": bg.unwrap().0, "coalesce": key})).unwrap();
        assert_eq!(r, json!({"selected": true}));
        let st = s.active().unwrap();
        assert!(st.doc.layer(LayerId(id)).is_none(), "no layer is left behind");
        assert_eq!(st.doc.layers.len(), before.layers.len());
        assert_eq!(st.active_layer, bg);
        let b = st.doc.selection.as_ref().unwrap().content_bounds();
        assert!(b.x0 >= 8 && b.x1 > 40 && b.y0 > 10 && b.y1 <= 70, "the selection is the text, not the old rectangle: {b:?}");
        let entries = st.history.entries();
        assert_eq!(entries.len(), steps + 1, "one step");
        assert_eq!(entries.last().map(String::as_str), Some(LABEL));
        s.execute("edit.undo", json!({})).unwrap();
        let b = s.active().unwrap().doc.selection.as_ref().unwrap().content_bounds();
        assert_eq!([b.x0, b.y0, b.x1, b.y1], [0, 0, 5, 5], "undo brings the old selection back");
    }

    #[test]
    fn only_type_layers_turn_into_a_mask() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 20, "height": 20, "background": "white"})).unwrap();
        let bg = s.active().unwrap().active_layer.unwrap().0;
        assert!(s.execute("type.maskToSelection", json!({"layer": bg})).is_err());
        assert!(s.execute("type.maskToSelection", json!({})).is_err());
    }
}
