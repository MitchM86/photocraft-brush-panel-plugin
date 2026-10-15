//! Frames. The Frame tool's `layer.new.frame` makes a frame: a group marked as one
//! ([`photocraft_doc::Group::frame`]) whose vector mask is the frame's rectangle or ellipse, so
//! whatever is inside is clipped to it. Drawn over a selected image layer, that layer goes in;
//! File › Place with a frame selected (`file.placeEmbedded` with `"frame"`) puts the image in,
//! scaled to fill the frame and centred, replacing what was there. Saved to PSD a frame is a
//! group with a vector mask, which looks the same.
//!
//! Layer › New › Frame from Layers groups the selected layers and clips the group with a
//! rectangular layer mask the size of their bounds. Headless and scriptable like every other
//! command.

use photocraft_doc::{FrameShape, Layer, LayerContent, LayerId, LayerMask, PixelFormat, VectorMask};
use photocraft_geom::Rect;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

fn has_layer(s: &Session) -> std::result::Result<(), String> {
    match s.active() {
        Some(d) if !d.selected_layers.is_empty() || d.active_layer.is_some() => Ok(()),
        Some(_) => Err("select a layer".into()),
        None => Err("no document".into()),
    }
}

/// A reveal mask that is on inside `rect` and off (hidden) everywhere else.
fn clip_mask(rect: Rect) -> LayerMask {
    let mut mask = LayerMask::hide_all();
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    if w > 0 && h > 0 {
        mask.surface.write_region(rect, &vec![1.0f32; w * h]);
        mask.surface.prune();
    }
    // Default to GRAY8 like the other mask constructors.
    debug_assert_eq!(mask.surface.format(), PixelFormat::GRAY8);
    mask
}

/// Groups the selected layers, then turns the group into a frame, as one history step: one undo
/// takes the whole command back (#496).
fn frame_from_layers(s: &mut Session, p: &Value) -> Result<Value> {
    let mut gp = serde_json::Map::new();
    if let Some(name) = p.get("name") {
        gp.insert("name".into(), name.clone());
    }
    let grouped = s.execute("layer.new.groupFromLayers", Value::Object(gp))?;
    match frame_group(s, &grouped) {
        Ok(r) => {
            // Fold the grouping step into the frame step.
            if let Some(st) = s.active_mut() {
                st.history.purge_last();
            }
            Ok(r)
        }
        Err(e) => {
            // The group couldn't be framed: take the grouping back, leaving nothing to redo.
            s.undo();
            if let Some(st) = s.active_mut() {
                st.history.clear_redo();
            }
            Err(e)
        }
    }
}

/// Clips the group `layer.new.groupFromLayers` returned (`grouped`) to its contents' bounds.
fn frame_group(s: &mut Session, grouped: &Value) -> Result<Value> {
    let gid = LayerId(grouped.get("layer").and_then(Value::as_u64).ok_or_else(|| EngineError::Other("grouping did not return a layer".into()))?);
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let rect =
        crate::layer_multi_cmds::layer_bounds(d.doc.layer(gid).ok_or(EngineError::NoLayer(gid))?).filter(|r| !r.is_empty()).unwrap_or_else(|| d.doc.bounds());

    s.edit("Frame from Layers", |doc, _| {
        let g = doc.layer_mut(gid).ok_or(EngineError::NoLayer(gid))?;
        g.mask = Some(clip_mask(rect));
        Ok(())
    })?;
    Ok(json!({"layer": gid.0, "frame": [rect.x0, rect.y0, rect.width(), rect.height()]}))
}

/// The frame's box (document px) when `l` is a frame: the bounds of its vector mask.
pub fn frame_rect(l: &Layer) -> Option<[f64; 4]> {
    let LayerContent::Group(g) = &l.content else { return None };
    g.frame?;
    let knots = l.vector_mask.as_ref()?.path.subpaths.iter().flat_map(|s| s.knots.iter());
    let r = knots.fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |r, k| [r[0].min(k.anchor.x), r[1].min(k.anchor.y), r[2].max(k.anchor.x), r[3].max(k.anchor.y)]);
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// The frame `id` is, or the frame it is directly inside.
pub fn frame_of(doc: &photocraft_doc::Document, id: LayerId) -> Option<LayerId> {
    if doc.layer(id).and_then(frame_rect).is_some() {
        return Some(id);
    }
    let path = doc.path_of(id)?;
    let parent = doc.layer_at(path.get(..path.len().checked_sub(1)?)?)?;
    frame_rect(parent).map(|_| parent.id)
}

fn next_frame_name(doc: &photocraft_doc::Document) -> String {
    let names: Vec<String> = doc.walk().into_iter().map(|(_, _, l)| l.name.clone()).collect();
    (1..).map(|n| format!("Frame {n}")).find(|n| !names.contains(n)).unwrap_or_else(|| "Frame".into())
}

/// A layer that can go into a frame: an image (pixels or a smart object), not the Background.
fn frameable(l: &Layer) -> bool {
    matches!(l.content, LayerContent::Raster(_) | LayerContent::Smart(_)) && !crate::extra_cmds::is_background(l)
}

fn new_frame(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.new.frame";
    let bad = |m: &str| EngineError::BadParams { cmd: CMD.into(), msg: m.into() };
    let r: Vec<f64> = p.get("rect").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let [x, y, w, h] = <[f64; 4]>::try_from(r).map_err(|_| bad("`rect` must be [x, y, width, height]"))?;
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0 && x.abs() < 1e7 && y.abs() < 1e7 && w < 1e7 && h < 1e7) {
        return Err(bad("the frame needs a finite position and a size of at least 1 x 1 px"));
    }
    let shape = match p.get("shape").and_then(Value::as_str).unwrap_or("rectangle") {
        "rectangle" => FrameShape::Rectangle,
        "ellipse" => FrameShape::Ellipse,
        other => return Err(bad(&format!("unknown shape `{other}` (rectangle or ellipse)"))),
    };
    let content = p.get("content").and_then(Value::as_u64).map(LayerId);
    let name = p.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_string);
    let id = s.edit("New Frame", |doc, active| {
        let path = match shape {
            FrameShape::Rectangle => photocraft_vector::shapes::rect(x, y, w, h),
            FrameShape::Ellipse => photocraft_vector::shapes::ellipse(x, y, w, h),
        };
        let mut g = Layer::group(name.unwrap_or_else(|| next_frame_name(doc)), vec![]);
        if let LayerContent::Group(gr) = &mut g.content {
            gr.frame = Some(shape);
        }
        // Isolated, so the vector mask clips what is inside.
        g.blend = photocraft_color::BlendMode::Normal;
        g.vector_mask = Some(VectorMask::new(path));
        let id = match content {
            Some(c) => {
                if !doc.layer(c).is_some_and(frameable) {
                    return Err(EngineError::Other("only an image layer (pixels or a smart object, not the Background) can go into a frame".into()));
                }
                let id = doc.insert_above(Some(c), g);
                let l = doc.remove(c).ok_or(EngineError::NoLayer(c))?;
                if let Some(LayerContent::Group(gr)) = doc.layer_mut(id).map(|f| &mut f.content) {
                    gr.children.push(l);
                }
                id
            }
            None => doc.insert_above(*active, g),
        };
        crate::layer_multi_cmds::check_group_depth(doc, "New Frame")?;
        *active = Some(id);
        Ok(id)
    })?;
    crate::layer_multi_cmds::set_selection(s, vec![id], Some(id), Some(id))?;
    Ok(json!({"layer": id.0, "rect": [x, y, w, h]}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "layer.new.frame",
            label: "New Frame",
            menu: &[],
            shortcut: None,
            params: r#"{"rect":[x,y,w,h] (document px),"shape":"rectangle|ellipse"="rectangle","content":id? (an image layer to put in it),"name":str?} → {layer, rect} (Frame tool)"#,
            enabled: |s| s.active().map(|_| ()).ok_or_else(|| "no document".into()),
            journal: true,
            run: new_frame,
        },
        CommandSpec {
        id: "layer.new.frameFromLayers",
        label: "Frame from Layers",
        menu: &["Layer", "New"],
        shortcut: None,
        params: r#"{"name":str?} → {layer, frame:[x,y,w,h]}: groups the selected layers and clips them to a frame rect"#,
        enabled: has_layer,
        journal: true,
        run: |s, p| frame_from_layers(s, p),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_doc::{Layer, LayerContent};
    use photocraft_geom::Rect;

    fn session() -> (Session, LayerId, LayerId) {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 80, "height": 60})).unwrap();
        let (a, b) = s
            .edit("setup", |doc, _| {
                let fmt = doc.pixel_format();
                let mut a = Layer::raster("A", fmt);
                a.surface_mut().unwrap().fill_rect(Rect::new(4, 4, 24, 24), &photocraft_raster::from_rgba(&fmt, [1.0, 0.0, 0.0, 1.0]));
                let mut b = Layer::raster("B", fmt);
                b.surface_mut().unwrap().fill_rect(Rect::new(30, 20, 50, 40), &photocraft_raster::from_rgba(&fmt, [0.0, 0.0, 1.0, 1.0]));
                let (ia, ib) = (a.id, b.id);
                doc.layers.push(a);
                doc.layers.push(b);
                Ok((ia, ib))
            })
            .unwrap();
        (s, a, b)
    }

    #[test]
    fn frames_selected_layers_into_a_masked_group() {
        let (mut s, a, b) = session();
        s.execute("layer.select", json!({"layer": a.0, "mode": "replace"})).unwrap();
        s.execute("layer.select", json!({"layer": b.0, "mode": "add"})).unwrap();
        let r = s.execute("layer.new.frameFromLayers", json!({"name": "Frame 1"})).unwrap();
        let gid = LayerId(r["layer"].as_u64().unwrap());
        // frame rect = union of the two layers' bounds (4,4)..(50,40).
        let frame = r["frame"].as_array().unwrap();
        assert_eq!(frame[2].as_i64().unwrap(), 50 - 4); // width
        assert_eq!(frame[3].as_i64().unwrap(), 40 - 4); // height
        let st = s.active().unwrap();
        let g = st.doc.layer(gid).unwrap();
        assert!(matches!(g.content, LayerContent::Group(_)), "it's a group");
        assert!(g.mask.is_some(), "the group has a clip mask");
        // Undoable (last step is the frame).
        assert_eq!(st.history.entries().last().map(|e| e.as_str()), Some("Frame from Layers"));
    }

    #[test]
    fn frame_from_layers_is_one_history_step() {
        // #496: grouping and framing undo together.
        let (mut s, a, b) = session();
        let entries = s.active().unwrap().history.entries();
        let layers = |s: &Session| s.active().unwrap().doc.layers.iter().map(|l| l.id).collect::<Vec<_>>();
        let before = layers(&s);
        s.execute("layer.select", json!({"layer": a.0, "mode": "replace"})).unwrap();
        s.execute("layer.select", json!({"layer": b.0, "mode": "add"})).unwrap();
        let gid = LayerId(s.execute("layer.new.frameFromLayers", json!({})).unwrap()["layer"].as_u64().unwrap());
        let mut framed = entries.clone();
        framed.push("Frame from Layers".into());
        assert_eq!(s.active().unwrap().history.entries(), framed);
        assert!(s.undo());
        assert_eq!(s.active().unwrap().history.entries(), entries);
        assert_eq!(layers(&s), before, "one undo leaves no group behind");
        assert!(s.redo());
        assert!(s.active().unwrap().doc.layer(gid).is_some_and(|g| g.mask.is_some()), "redo brings back the framed group");
    }

    #[test]
    fn needs_a_selected_layer() {
        let mut s = Session::new();
        assert!(s.execute("layer.new.frameFromLayers", json!({})).is_err());
    }

    fn pixel(s: &Session, x: i32, y: i32) -> [f32; 4] {
        let st = s.active().unwrap();
        photocraft_compose::render(&st.doc, Rect::new(x, y, x + 1, y + 1)).px[0]
    }

    #[test]
    fn a_frame_drawn_over_an_image_layer_clips_it_and_is_one_step() {
        let (mut s, a, _) = session();
        let steps = s.active().unwrap().history.entries().len();
        let r = s.execute("layer.new.frame", json!({"rect": [10, 10, 8, 8], "content": a.0})).unwrap();
        let f = LayerId(r["layer"].as_u64().unwrap());
        let st = s.active().unwrap();
        assert_eq!(st.history.entries().len(), steps + 1);
        assert_eq!(st.history.entries().last().map(String::as_str), Some("New Frame"));
        assert_eq!(st.active_layer, Some(f));
        let l = st.doc.layer(f).unwrap();
        assert_eq!(l.name, "Frame 1");
        assert_eq!(frame_rect(l), Some([10.0, 10.0, 18.0, 18.0]));
        assert_eq!(frame_of(&st.doc, a), Some(f), "A is inside the frame");
        // A's red square (4..24) shows only inside the frame (10..18).
        assert!(pixel(&s, 12, 12)[0] > 0.9);
        assert!(pixel(&s, 6, 6)[1] > 0.9, "clipped outside the frame: the background shows, not red");
        // An empty ellipse frame, then refusals.
        let e = s.execute("layer.new.frame", json!({"rect": [40, 5, 20, 10], "shape": "ellipse"})).unwrap();
        assert_eq!(s.active().unwrap().doc.layer(LayerId(e["layer"].as_u64().unwrap())).unwrap().name, "Frame 2");
        assert!(s.execute("layer.new.frame", json!({"rect": [0, 0, 0, 5]})).is_err());
        assert!(s.execute("layer.new.frame", json!({"rect": [0, 0, 5, 5], "shape": "star"})).is_err());
        assert!(s.execute("layer.new.frame", json!({"rect": [0, 0, 5, 5], "content": f.0})).is_err(), "a group can't go into a frame");
    }
}
