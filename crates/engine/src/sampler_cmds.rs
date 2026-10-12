//! Color Sampler Tool (I), as in Photoshop: up to ten fixed points whose colours the Info panel
//! reads out. They are view state like guides' selection, not History steps, and are not saved
//! with the file. Points are document pixels; each reads the Eyedropper's Sample Size around it
//! from the merged image (`sample_cmds::sample_color`).

use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::sample_cmds::{SampleLayers, sample_color, size_param};
use crate::{EngineError, Result, Session};

/// Photoshop's limit.
pub const MAX_SAMPLERS: usize = 10;

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

/// `x`, `y` inside the active document.
fn point(s: &Session, cmd: &str, p: &Value) -> Result<[f64; 2]> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let (w, h) = (f64::from(st.doc.size.width), f64::from(st.doc.size.height));
    let c = |k: &str, max: f64| {
        p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && (0.0..max).contains(v)).ok_or_else(|| bad(cmd, format!("`{k}` must be a number inside the document (0..{max})")))
    };
    Ok([c("x", w)?, c("y", h)?])
}

fn index(s: &Session, cmd: &str, p: &Value) -> Result<usize> {
    let n = s.active().ok_or(EngineError::NoDocument)?.color_samplers.len();
    p.get("index").and_then(Value::as_u64).map(|v| v as usize).filter(|i| *i < n).ok_or_else(|| bad(cmd, format!("`index` must be a sampler (0..{n})")))
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sampler.add";
    let at = point(s, CMD, p)?;
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    if st.color_samplers.len() >= MAX_SAMPLERS {
        return Err(bad(CMD, format!("a document has at most {MAX_SAMPLERS} color samplers")));
    }
    st.color_samplers.push(at);
    st.last_damage = Some(photocraft_geom::Rect::EMPTY);
    Ok(json!({ "index": st.color_samplers.len() - 1 }))
}

fn move_to(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sampler.move";
    let i = index(s, CMD, p)?;
    let at = point(s, CMD, p)?;
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    if let Some(q) = st.color_samplers.get_mut(i) {
        *q = at;
    }
    Ok(json!({ "index": i }))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sampler.delete";
    let i = index(s, CMD, p)?;
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    st.color_samplers.remove(i);
    Ok(json!({ "samplers": st.color_samplers.len() }))
}

fn clear(s: &mut Session, _: &Value) -> Result<Value> {
    let st = s.active_mut().ok_or(EngineError::NoDocument)?;
    st.color_samplers.clear();
    Ok(json!({ "samplers": 0 }))
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let size = size_param("sampler.list", p)?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let out: Vec<Value> = st
        .color_samplers
        .iter()
        .map(|&[x, y]| json!({ "x": x, "y": y, "color": sample_color(&st.doc, st.active_layer, x.floor() + 0.5, y.floor() + 0.5, size, SampleLayers::All) }))
        .collect();
    Ok(json!(out))
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn has_samplers(s: &Session) -> std::result::Result<(), String> {
    s.active().filter(|st| !st.color_samplers.is_empty()).map(|_| ()).ok_or_else(|| "no color samplers".into())
}

pub fn specs() -> Vec<CommandSpec> {
    let spec = |id, label, params, enabled: fn(&Session) -> std::result::Result<(), String>, run: fn(&mut Session, &Value) -> Result<Value>, journal| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled,
        run,
        journal,
    };
    vec![
        spec("sampler.add", "Add Color Sampler", r##"{"x":px,"y":px} (inside the document; at most 10) → {index}"##, has_doc, add, true),
        spec("sampler.move", "Move Color Sampler", r##"{"index":i,"x":px,"y":px} → {index}"##, has_samplers, move_to, true),
        spec("sampler.delete", "Delete Color Sampler", r##"{"index":i} → {samplers}"##, has_samplers, delete, true),
        spec("sampler.clearAll", "Clear All Color Samplers", r##"{} → {samplers}"##, has_samplers, clear, true),
        spec(
            "sampler.list",
            "List Color Samplers",
            r##"{"size":"point"|1|3|5|11|31|51|101=1} → [{x,y,color:[r,g,b,a]}] (the merged image, averaged over the Sample Size)"##,
            has_doc,
            list,
            false,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samplers_add_move_read_delete_and_cap_at_ten() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 40, "height": 20, "background": "white"})).unwrap();
        s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 20})).unwrap();
        s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
        let steps = s.active().unwrap().history.entries().len();
        assert_eq!(s.execute("sampler.add", json!({"x": 5.5, "y": 5.0})).unwrap(), json!({"index": 0}));
        s.execute("sampler.add", json!({"x": 30, "y": 5})).unwrap();
        assert_eq!(s.active().unwrap().history.entries().len(), steps, "not a history step");
        let l = s.execute("sampler.list", json!({})).unwrap();
        assert_eq!((l[0]["color"].clone(), l[1]["color"].clone()), (json!([1.0, 0.0, 0.0, 1.0]), json!([1.0, 1.0, 1.0, 1.0])));
        s.execute("sampler.move", json!({"index": 1, "x": 2, "y": 2})).unwrap();
        assert_eq!(s.execute("sampler.list", json!({})).unwrap()[1]["color"], json!([1.0, 0.0, 0.0, 1.0]));
        s.execute("sampler.delete", json!({"index": 0})).unwrap();
        assert_eq!(s.active().unwrap().color_samplers, vec![[2.0, 2.0]]);
        for i in 0..9 {
            s.execute("sampler.add", json!({"x": i, "y": 1})).unwrap();
        }
        assert!(s.execute("sampler.add", json!({"x": 1, "y": 1})).is_err(), "ten at most");
        for p in [json!({"x": -1, "y": 1}), json!({"x": 40, "y": 1}), json!({"x": "a", "y": 1}), json!({})] {
            assert!(s.execute("sampler.add", p.clone()).is_err(), "{p}");
        }
        assert!(s.execute("sampler.move", json!({"index": 10, "x": 1, "y": 1})).is_err());
        s.execute("sampler.clearAll", json!({})).unwrap();
        assert!(s.execute("sampler.delete", json!({"index": 0})).is_err());
    }
}
