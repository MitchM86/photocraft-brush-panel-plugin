//! The Brush HUD (ArtForge): a 3-tier concentric radial brush picker drawn as a modal overlay.
//!
//! - **Tier 1** (inner ring): category hubs (Sketch, Paint, Surface, Flora, Atmosphere, and
//!   Custom when some names match no keyword), routed by [`categorize::categorize_brush`].
//! - **Tier 2** (middle band): the active hub's families, fanned out in a wedge.
//! - **Tier 3** (outer spine): hovering or clicking a family opens its variations along a curved
//!   spine on the outer ring.
//!
//! Hovering any brush node previews its stroke in the right-hand panel (rendered by the real
//! brush engine, [`crate::brush_preview`]); a click picks it with `tools.setBrush {preset}`. The
//! panel edits the brush in hand through `tools.setBrush` gestures, like the Brush Settings
//! panel, whose section editors it reuses. Edit HUD reorders nodes by drag and removes them from
//! the HUD by dropping them on the bin or right-clicking (the presets are never deleted). Search
//! opens the best match's hub, family and spine, pulses the matching hubs and draws a tracer
//! from the search field to the brush.
//!
//! Open it with Window › Brush HUD (Alt+B), a right-click or pen barrel press on the canvas when
//! Preferences › Tools › Right-click with painting tools is "Brush HUD", or the control channel
//! (`ui.set {"brushHud": {...}}`). Its state is [`HudState`] in `UiState::brush_hud`; the layout,
//! recents and favourites are remembered in `Preferences::brush_hud`.

pub mod categorize;
pub mod geometry;
pub mod model;
mod panel;
mod view;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use model::{HudLayout, Library};

/// The command that shows and hides the HUD (Window › Brush HUD).
pub const COMMAND: &str = "window.brushHud";
/// Its default shortcut.
pub const SHORTCUT: &str = "Alt+B";

/// Most brushes remembered as recent, and as favourites.
pub const RECENT_KEEP: usize = 12;
pub const FAVORITES_KEEP: usize = 64;
/// Layout undo depth.
const UNDO_DEPTH: usize = 100;

/// What the small arc above the master hub lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ring {
    #[default]
    Recent,
    Favorites,
}

/// The HUD's state (`UiState::brush_hud`): what the control channel reads and sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HudState {
    pub open: bool,
    /// The active Tier 1 hub.
    pub category: String,
    /// The family whose spine is open (a `fam:<Category>/<Family>` id).
    pub family: Option<String>,
    pub fam_page: usize,
    pub var_page: usize,
    /// The preset under the pointer, previewed in the panel.
    pub hover: Option<String>,
    pub query: String,
    /// The search match the tracer points at.
    pub target: Option<String>,
    /// Edit HUD.
    pub edit: bool,
    pub ring: Ring,
    pub recent: Vec<String>,
    pub favorites: Vec<String>,
    pub layout: HudLayout,
    /// Close the HUD after a pick.
    pub close_on_pick: bool,
    #[serde(skip)]
    pub undo: Vec<HudLayout>,
    #[serde(skip)]
    pub redo: Vec<HudLayout>,
    /// When the current search target was set (seconds, egui time): the tracer draws in.
    #[serde(skip)]
    pub target_since: f64,
    /// Focus the search field next frame.
    #[serde(skip)]
    pub focus_search: bool,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            open: false,
            category: "Paint".into(),
            family: None,
            fam_page: 0,
            var_page: 0,
            hover: None,
            query: String::new(),
            target: None,
            edit: false,
            ring: Ring::Recent,
            recent: Vec::new(),
            favorites: Vec::new(),
            layout: HudLayout::default(),
            close_on_pick: false,
            undo: Vec::new(),
            redo: Vec::new(),
            target_since: 0.0,
            focus_search: false,
        }
    }
}

impl HudState {
    /// Replace the layout, keeping the old one for undo.
    pub fn set_layout(&mut self, next: HudLayout) {
        if next == self.layout {
            return;
        }
        let prev = std::mem::replace(&mut self.layout, next);
        self.undo.push(prev);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.layout, prev));
        true
    }
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.layout, next));
        true
    }
    pub fn push_recent(&mut self, name: &str) {
        self.recent.retain(|n| n != name);
        self.recent.insert(0, name.to_string());
        self.recent.truncate(RECENT_KEEP);
    }
    /// Toggles a favourite; true when it is now a favourite.
    pub fn toggle_favorite(&mut self, name: &str) -> bool {
        if let Some(i) = self.favorites.iter().position(|n| n == name) {
            self.favorites.remove(i);
            false
        } else {
            self.favorites.insert(0, name.to_string());
            self.favorites.truncate(FAVORITES_KEEP);
            true
        }
    }
    /// Clears the search (field, target, pulses and tracer).
    pub fn clear_search(&mut self) {
        self.query.clear();
        self.target = None;
    }
}

fn library_id() -> egui::Id {
    egui::Id::new("brush-hud-library")
}

/// The tagged library for the session's presets, rebuilt only when they change.
pub fn library(ctx: &egui::Context, presets: &[photocraft_engine::paint::BrushPreset]) -> Arc<Library> {
    let sig = model::signature(presets);
    if let Some(lib) = ctx.data(|d| d.get_temp::<Arc<Library>>(library_id())).filter(|l| l.signature == sig) {
        return lib;
    }
    let lib = Arc::new(Library::build(presets));
    ctx.data_mut(|d| d.insert_temp(library_id(), lib.clone()));
    lib
}

/// Show the HUD. The active hub follows the brush in hand.
pub fn open(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let st = &mut app.ui.brush_hud;
    if st.open {
        return;
    }
    st.open = true;
    st.hover = None;
    if let Some(name) = app.session.tools.current_preset.clone() {
        let lib = library(ctx, &app.session.tools.presets);
        reveal(&mut app.ui.brush_hud, &lib, &name);
    }
    // The Brush Preset picker and the HUD are alternatives: never both.
    crate::brush_picker::close(&mut app.ui);
}

pub fn close(app: &mut PhotocraftApp) {
    let st = &mut app.ui.brush_hud;
    st.open = false;
    st.hover = None;
    st.edit = false;
}

pub fn toggle(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if app.ui.brush_hud.open { close(app) } else { open(app, ctx) }
}

/// Window › Brush HUD (and its `open` / `close` params for scripts).
pub fn menu(app: &mut PhotocraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != COMMAND {
        return None;
    }
    match params.get("open").and_then(Value::as_bool) {
        Some(true) => open(app, ctx),
        Some(false) => close(app),
        None => toggle(app, ctx),
    }
    Some(Ok(json!({ "open": app.ui.brush_hud.open })))
}

/// Opens the hub, family page, spine and variation page that hold preset `name`.
pub fn reveal(st: &mut HudState, lib: &Library, name: &str) {
    let Some(e) = lib.entry(name) else { return };
    st.category = e.category.to_string();
    let fams = lib.families(e.category, &st.layout);
    let fi = fams.iter().position(|f| f.id == e.family_id);
    st.fam_page = match fi {
        Some(i) if fams.len() > geometry::FAM_MAX => i / (geometry::FAM_MAX - 1),
        _ => 0,
    };
    st.family = Some(e.family_id.clone());
    let vi = fi.and_then(|i| fams.get(i)).and_then(|f| f.members.iter().position(|&m| lib.entries.get(m).is_some_and(|x| x.name == name)));
    let len = fi.and_then(|i| fams.get(i)).map_or(0, |f| f.members.len());
    st.var_page = match vi {
        Some(i) if len > geometry::VAR_MAX => i / (geometry::VAR_MAX - 1),
        _ => 0,
    };
}

/// Runs the search in `st.query`: targets the best match and opens its place on the wheel.
pub fn run_search(st: &mut HudState, lib: &Library, presets: &[photocraft_engine::paint::BrushPreset], now: f64) {
    let hits = lib.search(&st.query, presets, &st.layout, 1);
    let target = hits.first().and_then(|&i| lib.entries.get(i)).map(|e| e.name.clone());
    if target != st.target {
        st.target_since = now;
    }
    st.target = target.clone();
    if let Some(name) = target {
        reveal(st, lib, &name);
    }
}

/// Picks preset `name` for the tool in hand.
pub fn pick(app: &mut PhotocraftApp, name: &str) {
    if app.session.tools.current_preset.as_deref() != Some(name) {
        crate::brush_panel::run_or_status(app, "tools.setBrush", json!({ "preset": name }));
    }
    if app.session.tools.current_preset.as_deref() == Some(name) {
        let st = &mut app.ui.brush_hud;
        st.push_recent(name);
        st.hover = None;
        if st.close_on_pick {
            close(app);
        }
    }
}

/// Keys while the HUD is open. Returns true when it owns the keyboard this frame (it is modal:
/// the canvas and menu shortcuts wait until it closes, except the HUD's own toggle).
pub fn keys(app: &mut PhotocraftApp, ctx: &egui::Context) -> bool {
    use egui::{Key, Modifiers};
    if !app.ui.brush_hud.open {
        return false;
    }
    let typing = ctx.text_edit_focused();
    let st = &mut app.ui.brush_hud;
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        if typing || !st.query.is_empty() {
            st.clear_search();
            ctx.memory_mut(|m| m.request_focus(egui::Id::NULL));
        } else if st.edit {
            st.edit = false;
        } else {
            close(app);
        }
        return true;
    }
    if typing {
        // The search field keeps its keys; Enter picks the target.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
            && let Some(name) = st.target.clone()
        {
            pick(app, &name);
        }
        return true;
    }
    let toggle_key = crate::shortcut_dispatch::bindings(app).into_iter().find(|(id, _)| id == COMMAND).map(|(_, sc)| sc);
    if let Some(sc) = toggle_key
        && ctx.input_mut(|i| i.consume_shortcut(&sc))
    {
        close(app);
        return true;
    }
    let st = &mut app.ui.brush_hud;
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y) {
            st.redo();
        } else if i.consume_key(Modifiers::COMMAND, Key::Z) {
            st.undo();
        } else if i.consume_key(Modifiers::COMMAND, Key::F) || i.consume_key(Modifiers::NONE, Key::Slash) {
            st.focus_search = true;
        } else if i.consume_key(Modifiers::NONE, Key::E) {
            st.edit = !st.edit;
        } else if i.consume_key(Modifiers::NONE, Key::Tab) {
            st.open = false;
            st.hover = None;
            st.edit = false;
        }
    });
    let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6];
    for (n, k) in digits.into_iter().enumerate() {
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k)) {
            let lib = library(ctx, &app.session.tools.presets);
            let st = &mut app.ui.brush_hud;
            if let Some((c, _)) = lib.categories(&st.layout).get(n) {
                set_category(st, c);
            }
        }
    }
    // Everything else is swallowed: no tool keys or document shortcuts under a modal HUD.
    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. })));
    true
}

pub fn set_category(st: &mut HudState, name: &str) {
    if st.category != name {
        st.category = name.to_string();
        st.family = None;
        st.fam_page = 0;
        st.var_page = 0;
    }
}

// ------------------------------------------------------------------ control channel

/// A `ui.set {"brushHud": …}` request, validated before anything changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlPatch {
    pub open: Option<bool>,
    pub category: Option<String>,
    /// `Some(None)` closes the spine.
    pub family: Option<Option<String>>,
    pub query: Option<String>,
    pub edit: Option<bool>,
    pub hover: Option<Option<String>>,
    pub ring: Option<Ring>,
}

const CONTROL_FIELDS: [&str; 7] = ["open", "category", "family", "query", "edit", "hover", "ring"];

/// Parses `brushHud`: `null` closes the HUD; an object sets any of `open`, `category`, `family`
/// (a `fam:<Category>/<Family>` id or null), `query` (runs the search), `edit`, `hover` (a preset
/// name or null) and `ring` (`recent` / `favorites`).
pub fn parse_control(v: &Value) -> Result<ControlPatch, String> {
    if v.is_null() {
        return Ok(ControlPatch { open: Some(false), ..Default::default() });
    }
    let Some(o) = v.as_object() else { return Err("brushHud must be an object or null".into()) };
    if let Some(k) = o.keys().find(|k| !CONTROL_FIELDS.contains(&k.as_str())) {
        return Err(format!("brushHud: unknown field `{k}` (fields: {})", CONTROL_FIELDS.join(", ")));
    }
    let boolean = |k: &str| o.get(k).map(|x| x.as_bool().ok_or(format!("brushHud.{k} must be a boolean"))).transpose();
    let text = |k: &str| o.get(k).map(|x| x.as_str().map(str::to_string).ok_or(format!("brushHud.{k} must be a string"))).transpose();
    let opt_text = |k: &str| {
        o.get(k)
            .map(|x| if x.is_null() { Ok(None) } else { x.as_str().map(|s| Some(s.to_string())).ok_or(format!("brushHud.{k} must be a string or null")) })
            .transpose()
    };
    let category = text("category")?;
    if let Some(c) = &category
        && !categorize::MACRO_ORDER.contains(&c.as_str())
        && c != categorize::CUSTOM
    {
        return Err(format!("brushHud.category must be one of {}, Custom", categorize::MACRO_ORDER.join(", ")));
    }
    let ring =
        o.get("ring").map(|r| serde_json::from_value::<Ring>(r.clone()).map_err(|_| "brushHud.ring must be recent or favorites".to_string())).transpose()?;
    Ok(ControlPatch {
        open: boolean("open")?,
        category,
        family: opt_text("family")?,
        query: text("query")?,
        edit: boolean("edit")?,
        hover: opt_text("hover")?,
        ring,
    })
}

pub fn apply_control(app: &mut PhotocraftApp, ctx: &egui::Context, patch: ControlPatch) {
    match patch.open {
        Some(true) => open(app, ctx),
        Some(false) => close(app),
        None => {}
    }
    let now = ctx.input(|i| i.time);
    let lib = library(ctx, &app.session.tools.presets);
    let st = &mut app.ui.brush_hud;
    if let Some(c) = patch.category {
        set_category(st, &c);
    }
    if let Some(f) = patch.family {
        st.family = f;
        st.var_page = 0;
    }
    if let Some(e) = patch.edit {
        st.edit = e;
    }
    if let Some(r) = patch.ring {
        st.ring = r;
    }
    if let Some(q) = patch.query {
        st.query = q;
        if st.query.trim().is_empty() {
            st.target = None;
        } else {
            // Scripts see the finished tracer, not its draw-in.
            run_search(st, &lib, &app.session.tools.presets, now - 1.0);
        }
    }
    if let Some(h) = patch.hover {
        st.hover = h;
    }
}

/// The HUD as `ui.inspect` reports it (`brushHud`).
pub fn inspect(app: &PhotocraftApp, ctx: &egui::Context) -> Value {
    let st = &app.ui.brush_hud;
    let lib = library(ctx, &app.session.tools.presets);
    let families: Vec<Value> = lib.families(&st.category, &st.layout).iter().map(|f| json!({"id": f.id, "name": f.name, "count": f.members.len()})).collect();
    let variations: Vec<&str> = st
        .family
        .as_ref()
        .and_then(|id| lib.family(id, &st.layout))
        .map(|f| f.members.iter().filter_map(|&i| lib.entries.get(i).map(|e| e.name.as_str())).collect())
        .unwrap_or_default();
    let variations: Vec<String> = variations.into_iter().map(str::to_string).collect();
    json!({
        "open": st.open,
        "category": st.category,
        "categories": lib.categories(&st.layout).into_iter().map(|(n, c)| json!({"name": n, "count": c})).collect::<Vec<_>>(),
        "families": families,
        "family": st.family,
        "variations": variations,
        "hover": st.hover,
        "query": st.query,
        "target": st.target,
        "edit": st.edit,
        "ring": st.ring,
        "recent": st.recent,
        "favorites": st.favorites,
        "layout": st.layout,
        "canUndo": !st.undo.is_empty(),
        "canRedo": !st.redo.is_empty(),
    })
}

// ------------------------------------------------------------------ persistence

/// What the preferences remember (`Preferences::brush_hud`).
fn saved(st: &HudState) -> Value {
    json!({
        "layout": st.layout,
        "recent": st.recent,
        "favorites": st.favorites,
        "ring": st.ring,
        "closeOnPick": st.close_on_pick,
        "category": st.category,
    })
}

/// Remember the HUD's layout, recents and favourites once the pointer is up (a small JSON
/// compare per frame). Run every frame so changes made over the control channel are kept too.
pub fn persist(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if ctx.input(|i| i.pointer.any_down()) {
        return;
    }
    let now = saved(&app.ui.brush_hud);
    if app.session.prefs().brush_hud != now {
        app.session.prefs.edit(|p| p.brush_hud = now);
    }
}

/// Restore what [`persist`] saved. Missing or malformed parts keep their defaults; lists are capped.
pub fn restore(app: &mut PhotocraftApp) {
    let v = app.session.prefs().brush_hud.clone();
    let st = &mut app.ui.brush_hud;
    if let Some(l) = v.get("layout").and_then(|l| serde_json::from_value::<HudLayout>(l.clone()).ok()) {
        st.layout = l.sanitized();
    }
    let names = |k: &str, cap: usize| -> Option<Vec<String>> {
        v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).take(cap).map(str::to_string).collect())
    };
    if let Some(r) = names("recent", RECENT_KEEP) {
        st.recent = r;
    }
    if let Some(f) = names("favorites", FAVORITES_KEEP) {
        st.favorites = f;
    }
    if let Some(r) = v.get("ring").and_then(|r| serde_json::from_value::<Ring>(r.clone()).ok()) {
        st.ring = r;
    }
    if let Some(b) = v.get("closeOnPick").and_then(Value::as_bool) {
        st.close_on_pick = b;
    }
    if let Some(c) = v.get("category").and_then(Value::as_str).filter(|c| c.len() < 64) {
        st.category = c.to_string();
    }
}

/// Draw the HUD when it is open. Called once a frame (after the canvas).
pub fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if app.ui.brush_hud.open {
        view::show(app, ctx);
    }
}

#[cfg(test)]
mod tests;
