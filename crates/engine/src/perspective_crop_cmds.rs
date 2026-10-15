//! Perspective Crop Tool, as in Photoshop: four corners placed over something seen at an angle (a
//! painting on a wall, a page on a desk) become the corners of the new, rectangular canvas, and
//! the image is straightened to fit in one "Perspective Crop" step.
//!
//! The map is the homography taking the quad onto `0..width × 0..height`. Every pixel surface
//! goes through it: layer pixels, masks, alpha channels and the Quick Mask. Smart objects keep it
//! as their placement, so they stay editable. Type and shape layers can't hold a perspective
//! (Free Transform's Perspective refuses them too), so they become pixel layers, as their
//! rendered pixels. Paths, vector masks, notes and Count/Ruler marks have their points mapped;
//! guides are cleared, as for any map that isn't axis-aligned. The selection is dropped, as by
//! Crop. Fill and adjustment layers cover the new canvas as they did the old one.
//!
//! The size is the options bar's W × H when given, otherwise the mean lengths of the quad's
//! opposite sides, so the straightened area keeps roughly its pixel count.

use photocraft_algo::transform::{Homography, Interp, warp_surface};
use photocraft_doc::{Document, Layer, LayerContent, Path};
use photocraft_geom::{Point, Rect};
use photocraft_raster::{Surface, from_rgba_into};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const CMD: &str = "image.perspectiveCrop";
/// As for Crop.
const MAX_SIDE: f64 = 300_000.0;

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: CMD.into(), msg: msg.into() }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

/// The quad's corners, clockwise from top-left on screen (document px).
fn quad(p: &Value) -> Result<[[f64; 2]; 4]> {
    let a = p.get("quad").and_then(Value::as_array).filter(|a| a.len() == 4).ok_or_else(|| bad("`quad` must be four [x, y] corners"))?;
    let mut q = [[0.0; 2]; 4];
    for (c, v) in q.iter_mut().zip(a) {
        let xy = v.as_array().filter(|xy| xy.len() == 2).and_then(|xy| Some([xy[0].as_f64()?, xy[1].as_f64()?]));
        *c = xy.filter(|xy| xy.iter().all(|v| v.is_finite() && v.abs() <= 1e7)).ok_or_else(|| bad("each corner must be [x, y] in document pixels"))?;
    }
    Ok(q)
}

/// A quad the crop can straighten: convex, its corners in clockwise order on screen, with some area.
pub fn valid(q: &[[f64; 2]; 4]) -> bool {
    let cross = |i: usize| {
        let (a, b, c) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4]);
        (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])
    };
    // y grows downwards: clockwise on screen turns right, a positive cross product.
    (0..4).all(|i| cross(i) > 1e-9) && area(q) >= 1.0
}

fn area(q: &[[f64; 2]; 4]) -> f64 {
    (0..4).map(|i| q[i][0] * q[(i + 1) % 4][1] - q[(i + 1) % 4][0] * q[i][1]).sum::<f64>().abs() / 2.0
}

/// The straightened size: the mean lengths of opposite sides.
pub fn natural_size(q: &[[f64; 2]; 4]) -> (f64, f64) {
    let len = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]).hypot(b[1] - a[1]);
    ((len(q[0], q[1]) + len(q[3], q[2])) / 2.0, (len(q[0], q[3]) + len(q[1], q[2])) / 2.0)
}

fn side(p: &Value, key: &str) -> Result<Option<u32>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|n| (1.0..=MAX_SIDE).contains(n))
            .map(|n| Some(n.round() as u32))
            .ok_or_else(|| bad(format!("`{key}` = {v} must be from 1 to {MAX_SIDE} px"))),
    }
}

fn map_point(h: &Homography, p: Point) -> Point {
    let (x, y) = h.apply(p.x, p.y);
    Point::new(x, y)
}

/// The path's anchors and handles through `h` (exact for the anchors; the curves between them
/// follow closely for the short segments paths are made of).
fn map_path(h: &Homography, path: &Path) -> Path {
    let mut out = path.clone();
    for k in out.subpaths.iter_mut().flat_map(|s| s.knots.iter_mut()) {
        k.anchor = map_point(h, k.anchor);
        k.in_ctrl = map_point(h, k.in_ctrl);
        k.out_ctrl = map_point(h, k.out_ctrl);
    }
    out
}

fn map_pt(h: &Homography, p: [f64; 2]) -> [f64; 2] {
    let (x, y) = h.apply(p[0], p[1]);
    [x, y]
}

/// Warp a surface whose pixels outside its content read as its default (masks, channels).
fn warp_plane(s: &Surface, h: &Homography, interp: Interp) -> Result<Surface> {
    crate::transform_cmds::warp_gray(s, h, interp)
}

fn warp_pixels(s: &Surface, h: &Homography, interp: Interp) -> Result<Surface> {
    Ok(warp_surface(s, s.content_bounds(), h, interp)?)
}

/// One layer and its children through `h`; `canvas` is the new canvas, `background` the
/// Background colour (RGBA) for the parts of the Background layer nothing maps to.
fn warp_layer(doc: &Document, l: &mut Layer, h: &Homography, interp: Interp, canvas: Rect, background: [f32; 4]) -> Result<()> {
    crate::allocation::checkpoint("straightening pixels")?;
    if matches!(l.content, LayerContent::Text(_) | LayerContent::Shape(_)) {
        let px = crate::extra_cmds::content_pixels(doc, l);
        l.content = LayerContent::Raster(px);
    }
    let background_layer = crate::extra_cmds::is_background(l);
    match &mut l.content {
        LayerContent::Group(g) => {
            for c in &mut g.children {
                warp_layer(doc, c, h, interp, canvas, background)?;
            }
        }
        LayerContent::Smart(sm) => {
            crate::smart_cmds::set_placement(sm, h.mul(&crate::smart_cmds::placement(sm)));
            if let Some(c) = &mut sm.cache {
                *c = warp_pixels(c, h, interp)?;
            }
            if let Some(m) = &mut sm.filter_mask {
                m.surface = warp_plane(&m.surface, h, interp)?;
            }
        }
        LayerContent::Raster(surf) => {
            let fmt = surf.format();
            let warped = warp_pixels(surf, h, interp)?;
            *surf = if background_layer {
                // The Background stays opaque: what the quad doesn't cover takes the Background colour.
                let mut base = Surface::new(warped.format());
                let mut px = [0.0f32; 8];
                let n = warped.format().channels();
                from_rgba_into(&warped.format(), background, &mut px);
                base.fill_rect(canvas, &px[..n]);
                crate::transform_cmds::composite_over(&mut base, &warped);
                base.convert(fmt)
            } else {
                warped
            };
        }
        _ => {}
    }
    // Fill caches re-render from the fill's model on the new canvas.
    l.fill_cache = None;
    if let Some(m) = &mut l.mask {
        m.surface = warp_plane(&m.surface, h, interp)?;
    }
    if let Some(vm) = &mut l.vector_mask {
        vm.path = map_path(h, &vm.path);
    }
    if let Some((x, y)) = l.effects.reference {
        let p = map_pt(h, [x, y]);
        l.effects.reference = Some((p[0], p[1]));
    }
    Ok(())
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let q = quad(p)?;
    if !valid(&q) {
        return Err(bad("the corners must make a convex shape, clockwise from the top-left (a crossed or folded frame can't be straightened)"));
    }
    let (nw, nh) = natural_size(&q);
    let (w, h) = match (side(p, "width")?, side(p, "height")?) {
        (Some(w), Some(h)) => (w, h),
        (None, None) => {
            let (w, h) = (nw.round().max(1.0), nh.round().max(1.0));
            if w > MAX_SIDE || h > MAX_SIDE {
                return Err(bad(format!("{w} x {h} exceeds the {MAX_SIDE} px limit per side")));
            }
            (w as u32, h as u32)
        }
        _ => return Err(bad("`width` and `height` go together")),
    };
    let resolution = match p.get("resolution") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|r| (1.0..=10_000.0).contains(r)).ok_or_else(|| bad("`resolution` must be 1..10000 ppi"))? as f32),
    };
    let interp = p.get("interpolation").and_then(Value::as_str).map_or(Interp::Bicubic, Interp::parse);
    let st = s.active().ok_or(EngineError::NoDocument)?;
    if st.doc.has_artboards() {
        return Err(EngineError::Other("Perspective Crop doesn't work on documents with artboards".into()));
    }
    crate::image_cmds::check_resample_budget(CMD, &st.doc, w, h, "")?;
    // Quad → new canvas.
    let to_quad = Homography::rect_to_quad([0.0, 0.0, f64::from(w), f64::from(h)], q).ok_or_else(|| bad("the corners don't make a shape that can be straightened"))?;
    let map = to_quad.inverse().ok_or_else(|| bad("the corners don't make a shape that can be straightened"))?;
    let background = s.tools.background;
    s.edit("Perspective Crop", |doc, _| {
        let canvas = Rect::new(0, 0, w as i32, h as i32);
        let snapshot = doc.clone();
        for l in &mut doc.layers {
            warp_layer(&snapshot, l, &map, interp, canvas, background)?;
        }
        for c in &mut doc.channels {
            c.surface = warp_plane(&c.surface, &map, interp)?;
        }
        if let Some(c) = &mut doc.quick_mask {
            c.surface = warp_plane(&c.surface, &map, interp)?;
        }
        for path in &mut doc.paths {
            path.path = map_path(&map, &path.path);
        }
        if let Some(wp) = &mut doc.work_path {
            *wp = map_path(&map, wp);
        }
        for n in &mut doc.notes {
            n.position = map_pt(&map, n.position);
            let [x0, y0] = map_pt(&map, [n.popup[0], n.popup[1]]);
            n.popup = [x0, y0, x0 + (n.popup[2] - n.popup[0]), y0 + (n.popup[3] - n.popup[1])];
        }
        for g in &mut doc.measurement.count_groups {
            for pt in &mut g.points {
                *pt = map_pt(&map, *pt);
            }
        }
        if let Some(r) = &mut doc.measurement.ruler {
            r.start = map_pt(&map, r.start);
            r.end = map_pt(&map, r.end);
            if let Some(pt) = &mut r.protractor {
                *pt = map_pt(&map, *pt);
            }
        }
        doc.slices.list.clear();
        doc.guides.horizontal.clear();
        doc.guides.vertical.clear();
        doc.selection = None;
        doc.size.width = w;
        doc.size.height = h;
        if let Some(r) = resolution {
            doc.resolution_dpi = r;
        }
        crate::canvas_geom::refresh(doc, crate::canvas_geom::Refresh::All);
        Ok(())
    })?;
    if let Some(st) = s.active_mut() {
        st.color_samplers.clear();
    }
    Ok(json!({ "documentWidth": w, "documentHeight": h, "resolution": s.active().map(|d| d.doc.resolution_dpi) }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Perspective Crop",
        menu: &[],
        shortcut: None,
        params: r##"{"quad":[[x,y]×4] (document px, clockwise from the top-left corner; convex),"width":px?,"height":px? (both or neither; default the mean lengths of opposite sides),"resolution":ppi?,"interpolation":"bicubic|bilinear|nearest"="bicubic"} → {documentWidth,documentHeight,resolution}"##,
        enabled: has_doc,
        run,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 200, "height": 160, "background": "white"})).unwrap();
        s
    }

    fn rgba(s: &Session, x: i32, y: i32) -> [f32; 4] {
        let st = s.active().unwrap();
        let l = st.doc.layer(st.active_layer.unwrap()).unwrap();
        photocraft_raster::to_rgba(&l.surface().unwrap().format(), &l.surface().unwrap().pixel(x, y))
    }

    #[test]
    fn a_tilted_quad_becomes_the_whole_rectangular_canvas() {
        let mut s = session();
        // A red quadrilateral (a "painting seen at an angle") on white.
        let q = [[40.0, 30.0], [170.0, 50.0], [160.0, 130.0], [30.0, 120.0]];
        let pts: Vec<Value> = q.iter().map(|p| json!([p[0], p[1]])).collect();
        s.execute("select.lasso", json!({"points": pts})).unwrap();
        s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
        s.execute("select.deselect", json!({})).unwrap();
        let steps = s.active().unwrap().history.entries().len();
        let r = s.execute(CMD, json!({"quad": q})).unwrap();
        let (w, h) = (r["documentWidth"].as_u64().unwrap() as i32, r["documentHeight"].as_u64().unwrap() as i32);
        assert_eq!((w, h), (131, 86), "the mean side lengths");
        // Red everywhere inside (away from the anti-aliased border).
        for (x, y) in [(3, 3), (w - 4, 3), (w - 4, h - 4), (3, h - 4), (w / 2, h / 2)] {
            let c = rgba(&s, x, y);
            assert!(c[0] > 0.9 && c[1] < 0.15 && c[3] > 0.99, "{x},{y}: {c:?}");
        }
        let st = s.active().unwrap();
        assert_eq!((st.doc.size.width, st.doc.size.height), (131, 86));
        assert!(crate::extra_cmds::is_background(st.doc.layer(st.active_layer.unwrap()).unwrap()), "the Background stays the Background");
        let entries = st.history.entries();
        assert_eq!((entries.len(), entries.last().map(String::as_str)), (steps + 1, Some("Perspective Crop")));
    }

    #[test]
    fn size_resolution_type_layers_and_bad_frames() {
        let mut s = session();
        s.execute("type.create", json!({"x": 20, "y": 80, "text": "Hi", "size": 40})).unwrap();
        let q = json!([[0, 0], [200, 0], [200, 160], [0, 160]]);
        let r = s.execute(CMD, json!({"quad": q, "width": 100, "height": 50, "resolution": 300})).unwrap();
        assert_eq!((r["documentWidth"].clone(), r["documentHeight"].clone(), r["resolution"].clone()), (json!(100), json!(50), json!(300.0)));
        let st = s.active().unwrap();
        assert!(st.doc.walk().iter().all(|(_, _, l)| !matches!(l.content, LayerContent::Text(_))), "type became pixels");
        // Crossed (bow-tie), counter-clockwise and too few corners are refused.
        for q in [json!([[0, 0], [50, 0], [0, 50], [50, 50]]), json!([[0, 0], [0, 50], [50, 50], [50, 0]]), json!([[0, 0], [10, 0], [10, 10]])] {
            assert!(s.execute(CMD, json!({"quad": q})).is_err(), "{q}");
        }
        assert!(s.execute(CMD, json!({"quad": [[0, 0], [10, 0], [10, 10], [0, 10]], "width": 5})).is_err());
    }
}
