//! Brush HUD UI tests: opening paths, hover preview, picking, Edit HUD (remove, reorder, undo),
//! search, modal keys, persistence and the control channel.

use egui::{Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use super::*;
use crate::state::Tool;

fn harness() -> Harness<'static, PhotocraftApp> {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 400, "height": 300})).unwrap();
    app.run("layer.new.layer", json!({})).unwrap();
    app.ui.tool = Tool::Brush;
    app.sync_views();
    let mut h = Harness::builder().with_size(vec2(1500.0, 920.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            let ctx = ui.ctx().clone();
            if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            crate::shortcuts::handle(app, &ctx);
            egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(4);
    h
}

fn invoke(h: &mut Harness<'static, PhotocraftApp>, id: &str) {
    let ctx = h.ctx.clone();
    crate::menus::invoke(h.state_mut(), &ctx, id, json!({})).unwrap();
    h.run_steps(3);
}

fn control(h: &mut Harness<'static, PhotocraftApp>, v: serde_json::Value) {
    let ctx = h.ctx.clone();
    let patch = parse_control(&v).unwrap();
    apply_control(h.state_mut(), &ctx, patch);
    h.run_steps(3);
}

fn center_of(h: &Harness<'static, PhotocraftApp>, label: &str) -> Pos2 {
    h.get_all_by_label(label).next().unwrap_or_else(|| panic!("no node {label:?}")).rect().center()
}

fn press(h: &mut Harness<'static, PhotocraftApp>, pos: Pos2, button: PointerButton, pressed: bool) {
    h.event(egui::Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn click(h: &mut Harness<'static, PhotocraftApp>, pos: Pos2, button: PointerButton) {
    h.event(egui::Event::PointerMoved(pos));
    h.run_steps(1);
    press(h, pos, button, true);
    press(h, pos, button, false);
}

fn drag(h: &mut Harness<'static, PhotocraftApp>, from: Pos2, to: Pos2) {
    h.event(egui::Event::PointerMoved(from));
    h.run_steps(1);
    press(h, from, PointerButton::Primary, true);
    for i in 1..=10 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 10.0)));
        h.run_steps(1);
    }
    press(h, to, PointerButton::Primary, false);
}

fn key(h: &mut Harness<'static, PhotocraftApp>, key: egui::Key, modifiers: Modifiers) {
    h.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.event(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(2);
}

#[test]
fn the_window_command_and_its_shortcut_toggle_the_hud() {
    let mut h = harness();
    assert!(!h.state().ui.brush_hud.open);
    invoke(&mut h, COMMAND);
    assert!(h.state().ui.brush_hud.open);
    // Alt+B closes it again: the HUD lets only its own toggle past its modal key handling.
    key(&mut h, egui::Key::B, Modifiers::ALT);
    assert!(!h.state().ui.brush_hud.open);
    key(&mut h, egui::Key::B, Modifiers::ALT);
    assert!(h.state().ui.brush_hud.open, "the shortcut opens it too");
    key(&mut h, egui::Key::Escape, Modifiers::NONE);
    assert!(!h.state().ui.brush_hud.open);
}

#[test]
fn the_hud_is_modal_for_tool_keys() {
    let mut h = harness();
    invoke(&mut h, COMMAND);
    key(&mut h, egui::Key::E, Modifiers::NONE);
    assert_eq!(h.state().ui.tool, Tool::Brush, "E toggles Edit HUD, not the Eraser");
    assert!(h.state().ui.brush_hud.edit);
    key(&mut h, egui::Key::Num1, Modifiers::NONE);
    assert_eq!(h.state().ui.brush_hud.category, "Sketch");
}

#[test]
fn right_click_opens_the_hud_when_the_preference_says_so() {
    let mut h = harness();
    h.state_mut().run("prefs.set", json!({"path": "tools.rightClickWithPaintingTools", "value": "brushHud"})).unwrap();
    let c = h.state().last_canvas_rect.center();
    click(&mut h, c, PointerButton::Secondary);
    assert!(h.state().ui.brush_hud.open, "the HUD, not the Brush Preset picker");
    assert_eq!(h.state().ui.brush_picker, None);
}

#[test]
fn hovering_previews_and_clicking_picks() {
    let mut h = harness();
    invoke(&mut h, COMMAND);
    control(&mut h, json!({"category": "Paint", "family": "fam:Paint/Round"}));
    let soft = center_of(&h, "Soft Round");
    h.event(egui::Event::PointerMoved(soft));
    h.run_steps(3);
    assert_eq!(h.state().ui.brush_hud.hover.as_deref(), Some("Soft Round"), "hover previews");
    assert_ne!(h.state().session.tools.current_preset.as_deref(), Some("Soft Round"), "hovering doesn't pick");
    click(&mut h, soft, PointerButton::Primary);
    assert_eq!(h.state().session.tools.current_preset.as_deref(), Some("Soft Round"));
    assert_eq!(h.state().ui.brush_hud.recent.first().map(String::as_str), Some("Soft Round"));
}

#[test]
fn hubs_switch_category_and_families_open_their_spine() {
    let mut h = harness();
    invoke(&mut h, COMMAND);
    let sketch = center_of(&h, "Sketch");
    click(&mut h, sketch, PointerButton::Primary);
    assert_eq!(h.state().ui.brush_hud.category, "Sketch");
    assert_eq!(h.state().ui.brush_hud.family, None);
    let lib = library(&h.ctx, &h.state().session.tools.presets);
    let first = lib.families("Sketch", &HudLayout::default()).into_iter().next().unwrap();
    let at = center_of(&h, &first.name);
    click(&mut h, at, PointerButton::Primary);
    assert_eq!(h.state().ui.brush_hud.family.as_deref(), Some(first.id.as_str()));
}

#[test]
fn a_family_opens_its_variations_on_the_outer_orbit_and_esc_header_or_hub_close_them() {
    let mut h = harness();
    invoke(&mut h, COMMAND);
    control(&mut h, json!({"category": "Paint", "family": "fam:Paint/Round"}));
    assert!(h.query_all_by_label("Soft Round").next().is_some(), "the variations are on the outer orbit");
    assert!(h.query_all_by_label("Round").next().is_some(), "and the families stay on theirs");
    let fam = center_of(&h, "Round");
    let var = center_of(&h, "Soft Round");
    let wheel = center_of(&h, "Master Tool");
    assert!((var - wheel).length() > (fam - wheel).length() + 40.0, "variations ring the families");
    key(&mut h, egui::Key::Escape, Modifiers::NONE);
    assert!(h.state().ui.brush_hud.open, "Esc steps back out of the family first");
    assert_eq!(h.state().ui.brush_hud.family, None);
    assert!(h.query_all_by_label("Soft Round").next().is_none());
    let round = center_of(&h, "Round");
    click(&mut h, round, PointerButton::Primary);
    assert_eq!(h.state().ui.brush_hud.family.as_deref(), Some("fam:Paint/Round"));
    let header = h.get_all_by_label_contains("ROUND").next().expect("the family's header").rect().center();
    click(&mut h, header, PointerButton::Primary);
    assert_eq!(h.state().ui.brush_hud.family, None, "the header leads back");
    click(&mut h, round, PointerButton::Primary);
    let paint = center_of(&h, "Paint");
    click(&mut h, paint, PointerButton::Primary);
    assert_eq!(h.state().ui.brush_hud.family, None, "and so does the active hub");
    assert_eq!(h.state().ui.brush_hud.category, "Paint");
}

#[test]
fn the_hud_floats_over_the_canvas_and_a_pick_or_a_click_outside_returns_to_painting() {
    let mut h = harness();
    // The options-bar button only reaches the UI state: it asks, and the HUD opens.
    h.state_mut().ui.brush_hud.open_requested = true;
    h.run_steps(3);
    assert!(h.state().ui.brush_hud.open);
    let canvas = h.state().last_canvas_rect;
    for hub in ["Sketch", "Paint", "Flora"] {
        assert!(canvas.contains(center_of(&h, hub)), "{hub} sits over the canvas");
    }
    // A click on the canvas outside the HUD dismisses it.
    click(&mut h, canvas.left_bottom() + vec2(6.0, -6.0), PointerButton::Primary);
    assert!(!h.state().ui.brush_hud.open);
    // Picking a brush does too.
    invoke(&mut h, COMMAND);
    control(&mut h, json!({"category": "Paint", "family": "fam:Paint/Round"}));
    let soft = center_of(&h, "Soft Round");
    click(&mut h, soft, PointerButton::Primary);
    assert_eq!(h.state().session.tools.current_preset.as_deref(), Some("Soft Round"));
    assert!(!h.state().ui.brush_hud.open, "back to painting");
}

#[test]
fn edit_hud_removes_by_right_click_and_undo_restores() {
    let mut h = harness();
    invoke(&mut h, COMMAND);
    control(&mut h, json!({"category": "Paint", "family": "fam:Paint/Round", "edit": true}));
    let at = center_of(&h, "Hard Round Fine");
    click(&mut h, at, PointerButton::Secondary);
    assert!(h.state().ui.brush_hud.layout.hidden.contains("Hard Round Fine"));
    assert!(h.state().session.tools.presets.iter().any(|p| p.name == "Hard Round Fine"), "the preset itself stays");
    assert!(h.query_all_by_label("Hard Round Fine").next().is_none());
    key(&mut h, egui::Key::Z, Modifiers::COMMAND);
    assert!(h.state().ui.brush_hud.layout.hidden.is_empty(), "Ctrl+Z restores it");
    key(&mut h, egui::Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(!h.state().ui.brush_hud.layout.hidden.is_empty(), "and Ctrl+Shift+Z removes it again");
}

#[test]
fn edit_hud_drag_onto_a_sibling_reorders_and_onto_the_bin_removes() {
    let mut h = harness();
    invoke(&mut h, COMMAND);
    control(&mut h, json!({"category": "Paint", "family": "fam:Paint/Round", "edit": true}));
    let scope = model::var_scope("fam:Paint/Round");
    let lib = library(&h.ctx, &h.state().session.tools.presets);
    let keys = lib.scope_keys(&scope, &HudLayout::default());
    let (a, c) = (center_of(&h, &keys[0]), center_of(&h, &keys[2]));
    drag(&mut h, a, c);
    let after = lib.scope_keys(&scope, &h.state().ui.brush_hud.layout);
    assert_eq!(after[2], keys[0], "the dragged node takes the target's place: {after:?}");
    let victim = after[1].clone();
    let bin = view::bin_center(&h.ctx).expect("the bin is drawn in Edit HUD");
    let from = center_of(&h, &victim);
    drag(&mut h, from, bin);
    assert!(h.state().ui.brush_hud.layout.hidden.contains(&victim), "dropped on the bin");
    assert!(h.state().session.tools.presets.iter().any(|p| p.name == victim));
}

#[test]
fn search_targets_reveals_and_enter_picks() {
    let mut h = harness();
    invoke(&mut h, COMMAND);
    control(&mut h, json!({"query": "charcoal"}));
    let st = h.state().ui.brush_hud.clone();
    assert_eq!(st.target.as_deref(), Some("Charcoal"));
    assert_eq!(st.category, "Sketch", "the target's hub opens");
    assert!(st.family.is_some(), "and its family spine");
    assert!(h.get_all_by_label("Charcoal").next().is_some(), "the target node is on the wheel");
    // Enter in the focused field picks the target.
    h.state_mut().ui.brush_hud.focus_search = true;
    h.run_steps(3);
    key(&mut h, egui::Key::Enter, Modifiers::NONE);
    assert_eq!(h.state().session.tools.current_preset.as_deref(), Some("Charcoal"));
    control(&mut h, json!({"query": "zzzz-nothing"}));
    assert_eq!(h.state().ui.brush_hud.target, None);
}

#[test]
fn layout_recents_and_favorites_survive_a_restart() {
    let mut h = harness();
    invoke(&mut h, COMMAND);
    {
        let st = &mut h.state_mut().ui.brush_hud;
        st.layout.hidden.insert("Soft Round".into());
        st.push_recent("Hard Round");
        st.toggle_favorite("Charcoal");
        st.ring = Ring::Favorites;
    }
    let ctx = h.ctx.clone();
    persist(h.state_mut(), &ctx);
    let saved = h.state().session.prefs().brush_hud.clone();
    let mut fresh = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    fresh.session.prefs.edit(|p| p.brush_hud = saved);
    restore(&mut fresh);
    let st = &fresh.ui.brush_hud;
    assert!(st.layout.hidden.contains("Soft Round"));
    assert_eq!(st.recent, ["Hard Round"]);
    assert_eq!(st.favorites, ["Charcoal"]);
    assert_eq!(st.ring, Ring::Favorites);
    // An early HUD saved `closeOnPick: false`; a pick still closes the HUD.
    let mut old = fresh.session.prefs().brush_hud.clone();
    old["closeOnPick"] = json!(false);
    fresh.session.prefs.edit(|p| p.brush_hud = old);
    restore(&mut fresh);
    assert!(fresh.ui.brush_hud.close_on_pick);
    // A damaged entry keeps the defaults instead of failing.
    let mut broken = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    broken.session.prefs.edit(|p| p.brush_hud = json!({"layout": 7, "recent": "x", "ring": "sideways", "category": 3}));
    restore(&mut broken);
    assert_eq!(broken.ui.brush_hud, HudState::default());
}

#[test]
fn control_requests_are_validated_before_anything_changes() {
    for bad in [json!(3), json!({"open": "yes"}), json!({"category": "Pottery"}), json!({"ring": "both"}), json!({"zoom": 1}), json!({"family": 4})] {
        assert!(parse_control(&bad).is_err(), "{bad}");
    }
    assert_eq!(parse_control(&json!(null)).unwrap().open, Some(false));
    let mut h = harness();
    control(&mut h, json!({"open": true, "category": "Flora", "ring": "favorites"}));
    let ctx = h.ctx.clone();
    let v = inspect(h.state(), &ctx);
    assert_eq!(v["open"], json!(true));
    assert_eq!(v["category"], json!("Flora"));
    assert_eq!(v["ring"], json!("favorites"));
    assert!(v["categories"].as_array().is_some_and(|c| c.len() >= 5));
}

#[test]
fn every_builtin_preset_has_a_place_on_the_wheel() {
    let presets = photocraft_engine::paint::presets::builtin();
    let lib = model::Library::build(&presets);
    let layout = HudLayout::default();
    let shown: usize = lib.categories(&layout).iter().flat_map(|(c, _)| lib.families(c, &layout)).map(|f| f.members.len()).sum();
    assert_eq!(shown, presets.len());
}
