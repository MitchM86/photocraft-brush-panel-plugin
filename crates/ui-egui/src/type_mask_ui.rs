//! Horizontal and Vertical Type Mask tools: while the text is edited, the canvas shows Photoshop's
//! Quick Mask overlay (the Quick Mask Options colour, red at 50% by default) with the letters left
//! clear, since they are what will be selected. The text itself lives in an invisible type layer
//! (`type_mask_cmds` in the engine), so every Type tool edit works on it; committing it turns it
//! into the selection (`type_tool::commit`). The layer is left out of the Layers panel.

use egui::{Color32, Mesh, Pos2, TextureOptions, pos2};
use photocraft_doc::LayerId;
use photocraft_geom::Rect as DRect;

use crate::PhotocraftApp;
use crate::canvas::ViewXform;

/// Largest overlay texture side; bigger text is sampled every few pixels.
const MAX_SIDE: u32 = 2048;

#[derive(Clone)]
struct Cached {
    key: (u64, u64, u64, [u8; 4]),
    bounds: DRect,
    tex: egui::TextureHandle,
}

/// The layer the Type Mask tool is typing into, which the Layers panel leaves out.
pub fn hidden_layer(app: &PhotocraftApp) -> Option<LayerId> {
    app.ui.text_edit.as_ref().filter(|ed| ed.mask.is_some()).map(|ed| LayerId(ed.layer))
}

/// Overlay colour for text coverage `a` (0 = none, 1 = full): the mask colour where nothing is typed.
fn overlay(color: [u8; 3], opacity: f32, a: f32) -> Color32 {
    let alpha = (opacity * (1.0 - a.clamp(0.0, 1.0))).clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied(color[0], color[1], color[2], (alpha * 255.0).round() as u8)
}

/// Paint the mask overlay over the document: solid around the text's bounds, the text's coverage
/// cut out of it inside them.
pub fn draw(app: &mut PhotocraftApp, painter: &egui::Painter, xf: &ViewXform, id: LayerId) {
    let Some(st) = app.session.active() else { return };
    let o = &app.session.quick_mask_options;
    let rgb = o.color.to_rgb().map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    let opacity = o.opacity;
    let fill = overlay(rgb, opacity, 0.0);
    let page = st.doc.bounds();
    let Some(surface) = st.doc.layer(id).and_then(|l| l.surface()) else { return };
    let bounds = surface.content_bounds().intersect(&page);
    let ctx = painter.ctx();
    let key = (st.doc.id.0, st.revision, id.0, [rgb[0], rgb[1], rgb[2], (opacity * 255.0) as u8]);
    let store = egui::Id::new("type-mask-overlay");
    let cached = ctx.data(|d| d.get_temp::<Cached>(store)).filter(|c| c.key == key);
    let cached = match cached {
        Some(c) => Some(c),
        None if bounds.is_empty() => None,
        None => {
            let factor = bounds.width().max(bounds.height()).div_ceil(MAX_SIDE).max(1);
            let mut px = vec![[0.0f32; 4]; bounds.width() as usize * bounds.height() as usize];
            surface.read_rgba_into(bounds, &mut px);
            let (w, h) = (bounds.width() as usize, bounds.height() as usize);
            let f = factor as usize;
            let (tw, th) = (w.div_ceil(f), h.div_ceil(f));
            let pixels = (0..th).flat_map(|y| (0..tw).map(move |x| (x * f, y * f))).map(|(x, y)| overlay(rgb, opacity, px[y * w + x][3])).collect();
            let img = egui::ColorImage::new([tw, th], pixels);
            let tex = ctx.load_texture("type-mask-overlay", img, TextureOptions::LINEAR);
            let c = Cached { key, bounds, tex };
            ctx.data_mut(|d| d.insert_temp(store, c.clone()));
            Some(c)
        }
    };
    // Around the text: the page minus the text's bounds, as up to four bands.
    let b = cached.as_ref().map_or(DRect::EMPTY, |c| c.bounds);
    let bands = if b.is_empty() {
        vec![page]
    } else {
        vec![
            DRect::new(page.x0, page.y0, page.x1, b.y0),
            DRect::new(page.x0, b.y1, page.x1, page.y1),
            DRect::new(page.x0, b.y0, b.x0, b.y1),
            DRect::new(b.x1, b.y0, page.x1, b.y1),
        ]
    };
    // One untextured mesh: no anti-aliased seams between the bands.
    let mut mesh = Mesh::default();
    for r in bands.into_iter().filter(|r| !r.is_empty()) {
        quad(&mut mesh, xf, r, fill, |_, _| egui::epaint::WHITE_UV);
    }
    painter.add(egui::Shape::mesh(mesh));
    if let Some(c) = cached {
        let mut mesh = Mesh::with_texture(c.tex.id());
        quad(&mut mesh, xf, c.bounds, Color32::WHITE, pos2);
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// Document rect `r` as two triangles; `uv` gives each corner's texture coordinate from its
/// position in the rect (0..1 across, 0..1 down).
fn quad(mesh: &mut Mesh, xf: &ViewXform, r: DRect, color: Color32, uv: impl Fn(f32, f32) -> Pos2) {
    let corners = [(r.x0, r.y0, 0.0, 0.0), (r.x1, r.y0, 1.0, 0.0), (r.x1, r.y1, 1.0, 1.0), (r.x0, r.y1, 0.0, 1.0)];
    let base = mesh.vertices.len() as u32;
    for (x, y, u, v) in corners {
        let pos = xf.to_screen(x as f32, y as f32);
        mesh.vertices.push(egui::epaint::Vertex { pos, uv: uv(u, v), color });
    }
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_clear_and_everything_else_takes_the_mask_colour() {
        assert_eq!(overlay([255, 0, 0], 0.5, 1.0).a(), 0, "the letters are clear");
        assert_eq!(overlay([255, 0, 0], 0.5, 0.0), Color32::from_rgba_unmultiplied(255, 0, 0, 128));
        assert_eq!(overlay([255, 0, 0], 0.5, 0.5).a(), 64, "soft edges fade");
    }
}
