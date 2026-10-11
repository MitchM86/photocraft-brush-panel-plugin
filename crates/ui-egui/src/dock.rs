//! Right-dock layout (#88): panel groups with fixed heights, like Photoshop's dock columns.
//!
//! A group's height never follows its content: content taller than the group scrolls inside
//! it. The last expanded group (Layers by default) fills what the others leave. Drag the gap
//! between two groups to resize them, double-click a tab (or use the panel menu) to collapse a
//! group to its tab strip, and drag a tab strip to move the group up or down the column
//! (unless Window › Workspace › Lock Workspace is on).
//!
//! Every tab is a [`Panel`]. A group shows its home panels ([`Group::home_panels`]) unless the
//! layout moved panels between groups ([`DockLayout::tabs`]); either way each panel is in exactly
//! one group, and the panel bodies, menus and commands find panels by [`Panel`], not by position.
//!
//! The layout is [`DockLayout`] in `UiState::dock` (serialisable, drivable with `ui.set`), saved
//! with Window › Workspace › New Workspace…, reset by Reset Workspace, and remembered across
//! launches in the preferences (`panelLayout`) while Remember Workspace Changes is on.

use std::collections::BTreeMap;

use egui::{Rect, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::state::{DockTabs, Panels};
use crate::theme::Tokens;
use crate::widgets;

/// A dock panel group (one tab strip): a default group, or one made by dragging a panel between
/// groups ([`Group::Extra`]). Serialised as its [`Group::key`] (`"layers"`, `"extra0"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// Color | Swatches | Gradients | Patterns.
    Color,
    /// Properties | Adjustments.
    Properties,
    /// Character | Paragraph (not in the default workspace; Window › Character opens it, #150).
    Character,
    /// Navigator | Histogram | Info.
    Navigator,
    /// History | Actions | Layer Comps.
    History,
    /// Layers | Channels | Paths.
    Layers,
    /// A group the user made (by dropping a panel between groups); its panels are all in
    /// [`DockLayout::tabs`].
    Extra(u8),
}

/// Most user-made groups (more drops reuse the free ones).
pub const MAX_EXTRA: u8 = 32;

const EXTRA_KEYS: [&str; MAX_EXTRA as usize] = [
    "extra0", "extra1", "extra2", "extra3", "extra4", "extra5", "extra6", "extra7", "extra8", "extra9", "extra10", "extra11", "extra12", "extra13", "extra14",
    "extra15", "extra16", "extra17", "extra18", "extra19", "extra20", "extra21", "extra22", "extra23", "extra24", "extra25", "extra26", "extra27", "extra28",
    "extra29", "extra30", "extra31",
];

impl Serialize for Group {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.key())
    }
}

impl<'de> Deserialize<'de> for Group {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let key = String::deserialize(d)?;
        Group::from_key(&key).ok_or_else(|| serde::de::Error::custom(format!("unknown dock group `{key}`")))
    }
}

/// One dock panel: a tab of a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Panel {
    Color,
    Swatches,
    Gradients,
    Patterns,
    Properties,
    Adjustments,
    Character,
    Paragraph,
    Navigator,
    Histogram,
    Info,
    History,
    Actions,
    LayerComps,
    Layers,
    Channels,
    Paths,
}

impl Panel {
    pub const ALL: [Panel; 17] = [
        Panel::Color,
        Panel::Swatches,
        Panel::Gradients,
        Panel::Patterns,
        Panel::Properties,
        Panel::Adjustments,
        Panel::Character,
        Panel::Paragraph,
        Panel::Navigator,
        Panel::Histogram,
        Panel::Info,
        Panel::History,
        Panel::Actions,
        Panel::LayerComps,
        Panel::Layers,
        Panel::Channels,
        Panel::Paths,
    ];

    /// The tab label (English; translated where it is drawn).
    pub fn name(self) -> &'static str {
        match self {
            Panel::Color => "Color",
            Panel::Swatches => "Swatches",
            Panel::Gradients => "Gradients",
            Panel::Patterns => "Patterns",
            Panel::Properties => "Properties",
            Panel::Adjustments => "Adjustments",
            Panel::Character => "Character",
            Panel::Paragraph => "Paragraph",
            Panel::Navigator => "Navigator",
            Panel::Histogram => "Histogram",
            Panel::Info => "Info",
            Panel::History => "History",
            Panel::Actions => "Actions",
            Panel::LayerComps => "Layer Comps",
            Panel::Layers => "Layers",
            Panel::Channels => "Channels",
            Panel::Paths => "Paths",
        }
    }

    /// The panel with this tab label.
    pub fn from_name(name: &str) -> Option<Panel> {
        Panel::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The group the panel belongs to in the default workspace.
    pub fn home(self) -> Group {
        match self {
            Panel::Color | Panel::Swatches | Panel::Gradients | Panel::Patterns => Group::Color,
            Panel::Properties | Panel::Adjustments => Group::Properties,
            Panel::Character | Panel::Paragraph => Group::Character,
            Panel::Navigator | Panel::Histogram | Panel::Info => Group::Navigator,
            Panel::History | Panel::Actions | Panel::LayerComps => Group::History,
            Panel::Layers | Panel::Channels | Panel::Paths => Group::Layers,
        }
    }

    /// Panels that lay out their own scrolling list and footer (they fill the group).
    pub fn scrolls_itself(self) -> bool {
        matches!(self, Panel::Layers | Panel::History)
    }
}

impl Group {
    /// Photoshop Essentials order, top to bottom.
    pub const ALL: [Group; 6] = [Group::Color, Group::Properties, Group::Character, Group::Navigator, Group::History, Group::Layers];

    pub fn key(self) -> &'static str {
        match self {
            Group::Color => "color",
            Group::Properties => "properties",
            Group::Character => "character",
            Group::Navigator => "navigator",
            Group::History => "history",
            Group::Layers => "layers",
            Group::Extra(n) => EXTRA_KEYS.get(usize::from(n)).copied().unwrap_or("extra0"),
        }
    }

    /// Height (points, tab strip included) a group gets until the user resizes it, when the
    /// column has room (see [`DockLayout::heights_for`]).
    pub fn default_height(self) -> f32 {
        match self {
            Group::Color => 190.0,
            Group::Properties => 250.0,
            Group::Character => 270.0,
            Group::Navigator => 210.0,
            Group::History => 200.0,
            Group::Layers => 320.0,
            Group::Extra(_) => 220.0,
        }
    }

    /// What a group left at its default height gives way down to so the filler (Layers) keeps
    /// [`Group::preferred_fill`] in a short column (#147). Content taller than this scrolls.
    pub fn compact_height(self) -> f32 {
        match self {
            Group::Color => 130.0,
            Group::Properties => 160.0,
            Group::Character => 160.0,
            Group::Navigator => 140.0,
            Group::History => 130.0,
            Group::Layers => 200.0,
            Group::Extra(_) => 130.0,
        }
    }

    /// The height the filling group asks for before default-sized groups above it get their
    /// full defaults: Layers wants room for about ten rows at 900 pt (#147).
    pub fn preferred_fill(self) -> f32 {
        match self {
            Group::Layers => 500.0,
            g => g.min_height(),
        }
    }

    /// The smallest height an expanded group can be dragged or squeezed to.
    pub fn min_height(self) -> f32 {
        match self {
            Group::Layers => 140.0,
            _ => 80.0,
        }
    }

    pub fn tabs(self, pro: bool) -> &'static [&'static str] {
        match self {
            // Photoshop Essentials: Color | Swatches | Gradients | Patterns.
            Group::Color if pro => &["Color", "Swatches", "Gradients", "Patterns"],
            Group::Color => &["Swatches", "Color", "Gradients", "Patterns"],
            Group::Properties => &["Properties", "Adjustments"],
            Group::Character => &["Character", "Paragraph"],
            Group::Navigator => &["Navigator", "Histogram", "Info"],
            Group::History => &["History", "Actions", "Layer Comps"],
            Group::Layers => &["Layers", "Channels", "Paths"],
            Group::Extra(_) => &[],
        }
    }

    /// The group's panels in the default workspace, in tab order ([`Self::tabs`]' panels).
    pub fn home_panels(self, pro: bool) -> &'static [Panel] {
        match self {
            Group::Color if pro => &[Panel::Color, Panel::Swatches, Panel::Gradients, Panel::Patterns],
            Group::Color => &[Panel::Swatches, Panel::Color, Panel::Gradients, Panel::Patterns],
            Group::Properties => &[Panel::Properties, Panel::Adjustments],
            Group::Character => &[Panel::Character, Panel::Paragraph],
            Group::Navigator => &[Panel::Navigator, Panel::Histogram, Panel::Info],
            Group::History => &[Panel::History, Panel::Actions, Panel::LayerComps],
            Group::Layers => &[Panel::Layers, Panel::Channels, Panel::Paths],
            Group::Extra(_) => &[],
        }
    }

    /// Home tabs that lay out their own scrolling list and footer (they fill the group).
    pub fn scrolls_itself(self, tab: usize) -> bool {
        self.home_panels(true).get(tab).is_some_and(|p| p.scrolls_itself())
    }

    pub(crate) fn tab_mut(self, tabs: &mut DockTabs) -> &mut usize {
        match self {
            Group::Color => &mut tabs.color,
            Group::Properties => &mut tabs.properties,
            Group::Character => &mut tabs.character,
            Group::Navigator => &mut tabs.navigator,
            Group::History => &mut tabs.history,
            Group::Layers => &mut tabs.layers,
            Group::Extra(n) => tabs.extra.entry(n).or_insert(0),
        }
    }

    /// The group for a `panels` / `dockTabs` key ("color", "properties", …, "extra3").
    pub fn from_key(key: &str) -> Option<Group> {
        if let Some(n) = key.strip_prefix("extra").and_then(|n| n.parse::<u8>().ok()).filter(|n| *n < MAX_EXTRA) {
            return Some(Group::Extra(n));
        }
        Group::ALL.into_iter().find(|g| g.key() == key)
    }

    pub fn shown(self, panels: &Panels) -> bool {
        match self {
            Group::Color => panels.color,
            Group::Properties => panels.properties,
            Group::Character => panels.character,
            Group::Navigator => panels.navigator,
            Group::History => panels.history,
            Group::Layers => panels.layers,
            Group::Extra(n) => panels.extra_groups.get(&n).copied().unwrap_or(true),
        }
    }

    pub(crate) fn shown_mut(self, panels: &mut Panels) -> &mut bool {
        match self {
            Group::Color => &mut panels.color,
            Group::Properties => &mut panels.properties,
            Group::Character => &mut panels.character,
            Group::Navigator => &mut panels.navigator,
            Group::History => &mut panels.history,
            Group::Layers => &mut panels.layers,
            Group::Extra(n) => panels.extra_groups.entry(n).or_insert(true),
        }
    }
}

/// Order, heights, collapsed state and visibility of individual dock tabs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockLayout {
    /// Top-to-bottom order; a group missing here (say one added after the layout was saved)
    /// goes just below the nearest group that precedes it by default (or last).
    pub order: Vec<Group>,
    /// Heights the user dragged groups to (points, tab strip included). Unset = default.
    pub heights: BTreeMap<Group, f32>,
    /// Groups collapsed to their tab strip.
    pub collapsed: Vec<Group>,
    /// Names of tabs hidden by Close (not positions: Color/Swatches swap with the theme).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub hidden_tabs: BTreeMap<Group, Vec<String>>,
    /// Groups whose panels were rearranged: their tabs in order. A group missing here shows its
    /// home panels less any listed elsewhere (see [`DockLayout::panels`]).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tabs: BTreeMap<Group, Vec<Panel>>,
}

/// Where a dragged panel goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelDrop {
    /// Into a group's tabs at this index (counting all its tabs, hidden ones included).
    Into(Group, usize),
    /// A new group of its own, just before this one (`None` = last).
    NewGroup(Option<Group>),
}

/// Gap between groups; it is also the splitter's grab area.
pub const GAP: f32 = 6.0;
/// Upper bound on a stored height (guards against absurd values from `ui.set`).
const MAX_HEIGHT: f32 = 4000.0;

impl DockLayout {
    /// `group`'s panels in tab order (hidden ones included; their indices are the tab indices
    /// `dockTabs` and the commands use). Each panel is in exactly one group: a group's own list
    /// when it has one (first listing wins), else its home panels less those listed elsewhere;
    /// a panel listed nowhere returns to its home group.
    pub fn panels(&self, group: Group, pro: bool) -> Vec<Panel> {
        let listed_elsewhere = |p: &Panel, before_only: bool| self.tabs.iter().any(|(g, list)| *g != group && (!before_only || *g < group) && list.contains(p));
        let mut out: Vec<Panel> = Vec::new();
        match self.tabs.get(&group) {
            Some(list) => {
                for p in list {
                    if !out.contains(p) && !listed_elsewhere(p, true) {
                        out.push(*p);
                    }
                }
                for p in group.home_panels(pro) {
                    if !out.contains(p) && !listed_elsewhere(p, false) {
                        out.push(*p);
                    }
                }
            }
            None => out.extend(group.home_panels(pro).iter().copied().filter(|p| !listed_elsewhere(p, false))),
        }
        out
    }

    /// Visible panels with their tab indices.
    pub fn visible_panels(&self, group: Group, pro: bool) -> Vec<(usize, Panel)> {
        let hidden = self.hidden_tabs.get(&group);
        self.panels(group, pro).into_iter().enumerate().filter(|(_, p)| !hidden.is_some_and(|xs| xs.iter().any(|x| x.as_str() == p.name()))).collect()
    }

    /// Visible tabs, retaining their original indices for commands and panel bodies.
    pub fn visible_tabs(&self, group: Group, pro: bool) -> Vec<(usize, &'static str)> {
        self.visible_panels(group, pro).into_iter().map(|(i, p)| (i, p.name())).collect()
    }

    /// The group `p` is in and its tab index there.
    pub fn locate(&self, p: Panel, pro: bool) -> (Group, usize) {
        self.order().into_iter().find_map(|g| self.panels(g, pro).iter().position(|x| *x == p).map(|i| (g, i))).unwrap_or((p.home(), 0))
    }

    /// A group number free for a new user-made group.
    fn free_extra(&self) -> Option<u8> {
        (0..MAX_EXTRA).find(|n| {
            let g = Group::Extra(*n);
            !self.tabs.get(&g).is_some_and(|l| !l.is_empty()) && !self.order.contains(&g)
        })
    }

    /// Move panel `p` to `to` (its tab order there, or a new group). Returns the group it
    /// ended up in and its tab index there, `None` when nothing changed (no free group slot, or
    /// dropped where it already is). Groups left empty stay in place for their other state;
    /// user-made ones are removed.
    pub fn move_panel(&mut self, p: Panel, to: PanelDrop, pro: bool) -> Option<(Group, usize)> {
        let (from, at) = self.locate(p, pro);
        let mut src = self.panels(from, pro);
        let (group, index) = match to {
            PanelDrop::Into(g, i) => {
                if g == from {
                    // Reorder in place: the index counts the tabs before the panel left.
                    let i = if i > at { i - 1 } else { i };
                    let i = i.min(src.len().saturating_sub(1));
                    if i == at {
                        return None;
                    }
                    src.retain(|x| *x != p);
                    src.insert(i, p);
                    self.tabs.insert(from, src);
                    return Some((from, i));
                }
                let mut dst = self.panels(g, pro);
                let i = i.min(dst.len());
                dst.insert(i, p);
                self.tabs.insert(g, dst);
                (g, i)
            }
            PanelDrop::NewGroup(before) => {
                if src.len() == 1 && before.is_some_and(|b| b == from) {
                    return None;
                }
                let g = Group::Extra(self.free_extra()?);
                self.tabs.insert(g, vec![p]);
                let mut order = self.order();
                order.retain(|x| *x != g);
                let pos = before.and_then(|b| order.iter().position(|x| *x == b)).unwrap_or(order.len());
                order.insert(pos, g);
                self.order = order;
                (g, 0)
            }
        };
        src.retain(|x| *x != p);
        self.tabs.insert(from, src);
        self.prune_extra();
        Some((group, index))
    }

    /// Forget user-made groups that have no panels left.
    fn prune_extra(&mut self) {
        let empty: Vec<Group> = self.tabs.iter().filter(|(g, l)| matches!(g, Group::Extra(_)) && l.is_empty()).map(|(g, _)| *g).collect();
        for g in empty {
            self.tabs.remove(&g);
            self.order.retain(|x| *x != g);
            self.heights.remove(&g);
            self.collapsed.retain(|x| *x != g);
            self.hidden_tabs.remove(&g);
        }
    }

    /// Hide one tab; it can be reopened through Window › Panel.
    pub fn hide_tab(&mut self, group: Group, tab: usize, pro: bool) {
        let Some(name) = self.panels(group, pro).get(tab).map(|p| p.name()) else { return };
        let hidden = self.hidden_tabs.entry(group).or_default();
        if !hidden.iter().any(|x| x.as_str() == name) {
            hidden.push(name.to_owned());
        }
    }

    /// Restore the tab requested by the Window menu or icon rail.
    pub fn show_tab(&mut self, group: Group, tab: usize, pro: bool) {
        let Some(name) = self.panels(group, pro).get(tab).map(|p| p.name()) else { return };
        if let Some(hidden) = self.hidden_tabs.get_mut(&group) {
            hidden.retain(|n| n != name);
            if hidden.is_empty() {
                self.hidden_tabs.remove(&group);
            }
        }
    }

    /// Every group once, in display order.
    pub fn order(&self) -> Vec<Group> {
        let mut out: Vec<Group> = Vec::with_capacity(Group::ALL.len());
        for g in &self.order {
            if !out.contains(g) {
                out.push(*g);
            }
        }
        // Missing groups slot in below their default predecessor, so a group new to an old
        // saved layout (Character) never lands below Layers and takes over as the filler.
        for (i, g) in Group::ALL.iter().enumerate() {
            if out.contains(g) {
                continue;
            }
            let prev = Group::ALL.iter().take(i).rev().find_map(|p| out.iter().position(|x| x == p));
            match prev {
                Some(at) => out.insert(at + 1, *g),
                None => out.push(*g),
            }
        }
        out
    }

    pub fn is_collapsed(&self, g: Group) -> bool {
        self.collapsed.contains(&g)
    }

    pub fn set_collapsed(&mut self, g: Group, on: bool) {
        self.collapsed.retain(|c| *c != g);
        if on {
            self.collapsed.push(g);
        }
    }

    /// The stored (or default) height, sanitised.
    pub fn height(&self, g: Group) -> f32 {
        match self.heights.get(&g) {
            Some(h) if h.is_finite() => h.clamp(g.min_height(), MAX_HEIGHT),
            _ => g.default_height(),
        }
    }

    /// Move `g` so it is drawn just before `before` (or last when `None`).
    pub fn move_group(&mut self, g: Group, before: Option<Group>) {
        if before == Some(g) {
            return;
        }
        let mut order = self.order();
        order.retain(|x| *x != g);
        let at = before.and_then(|b| order.iter().position(|x| *x == b)).unwrap_or(order.len());
        order.insert(at, g);
        self.order = order;
    }

    /// Lay out the `shown` groups (in display order) in a column `avail` points tall with
    /// `strip`-high tab strips. Returns each group's height. The last expanded group fills the
    /// rest. Groups the user never resized give way first, down to their compact heights, so
    /// the filler gets its preferred height (Layers: ~10 rows, #147); when the column is still
    /// too short every group gives way down to its minimum height.
    pub fn heights_for(&self, shown: &[Group], avail: f32, strip: f32) -> Vec<(Group, f32)> {
        let avail = if avail.is_finite() { avail.max(0.0) } else { 0.0 };
        let filler = shown.iter().rposition(|g| !self.is_collapsed(*g));
        let mut hs: Vec<f32> = shown
            .iter()
            .enumerate()
            .map(|(i, g)| {
                if self.is_collapsed(*g) {
                    strip
                } else if Some(i) == filler {
                    0.0
                } else {
                    self.height(*g)
                }
            })
            .collect();
        if let Some(f) = filler {
            let gaps = GAP * shown.len().saturating_sub(1) as f32;
            let min_fill = shown.get(f).map_or(0.0, |g| g.min_height());
            let pref_fill = shown.get(f).map_or(0.0, |g| g.preferred_fill());
            let used: f32 = hs.iter().sum::<f32>() + gaps;
            let mut deficit = (used + pref_fill - avail).max(0.0);
            for i in (0..f).rev() {
                if deficit <= 0.0 {
                    break;
                }
                let Some(g) = shown.get(i) else { continue };
                if self.is_collapsed(*g) || self.heights.contains_key(g) {
                    continue;
                }
                if let Some(h) = hs.get_mut(i) {
                    let give = (*h - g.compact_height()).max(0.0).min(deficit);
                    *h -= give;
                    deficit -= give;
                }
            }
            let used: f32 = hs.iter().sum::<f32>() + gaps;
            let mut deficit = (used + min_fill - avail).max(0.0);
            // Squeeze the expanded groups nearest the filler first.
            for i in (0..f).rev() {
                if deficit <= 0.0 {
                    break;
                }
                let Some(g) = shown.get(i) else { continue };
                if self.is_collapsed(*g) {
                    continue;
                }
                if let Some(h) = hs.get_mut(i) {
                    let give = (*h - g.min_height()).max(0.0).min(deficit);
                    *h -= give;
                    deficit -= give;
                }
            }
            let rest = avail - hs.iter().sum::<f32>() - gaps;
            if let Some(h) = hs.get_mut(f) {
                *h = rest.max(min_fill);
            }
        }
        shown.iter().copied().zip(hs).collect()
    }
}

/// Show `g` and expand it (Window › <panel>, the icon rail): a panel asked for is always
/// brought back, whatever state it was left in (#129).
pub fn reveal(app: &mut PhotocraftApp, g: Group) {
    *g.shown_mut(&mut app.ui.panels) = true;
    let selected = *g.tab_mut(&mut app.ui.dock_tabs);
    let pro = matches!(app.ui.theme, crate::theme::ThemeKind::Pro | crate::theme::ThemeKind::ProMedium);
    app.ui.dock.show_tab(g, selected, pro);
    app.ui.dock.set_collapsed(g, false);
}

/// Icon rail click: a hidden group is shown, a collapsed one expanded and an expanded one
/// collapsed to its tab strip. A docked group is never hidden from the rail (it used to
/// toggle visibility, so one stray click made a panel vanish: #129); `docked` is false for
/// Studio's floating Properties card, which the rail shows and hides.
pub fn rail_click(app: &mut PhotocraftApp, g: Group, docked: bool) {
    if !g.shown(&app.ui.panels) {
        reveal(app, g);
    } else if !docked {
        *g.shown_mut(&mut app.ui.panels) = false;
    } else {
        let collapse = !app.ui.dock.is_collapsed(g);
        app.ui.dock.set_collapsed(g, collapse);
    }
}

fn is_pro(app: &PhotocraftApp) -> bool {
    matches!(app.ui.theme, crate::theme::ThemeKind::Pro | crate::theme::ThemeKind::ProMedium)
}

/// Where `p` is in the live layout: its group and tab index.
pub fn locate(app: &PhotocraftApp, p: Panel) -> (Group, usize) {
    app.ui.dock.locate(p, is_pro(app))
}

/// Is `p` its group's selected tab, with the group open (collapsed counts when `expanded` is false)?
pub fn panel_selected(app: &PhotocraftApp, p: Panel, expanded: bool) -> bool {
    let (g, i) = locate(app, p);
    let mut tabs = app.ui.dock_tabs.clone();
    g.shown(&app.ui.panels) && *g.tab_mut(&mut tabs) == i && (!expanded || !app.ui.dock.is_collapsed(g))
}

/// Make `p` its group's selected tab (wherever it is docked).
pub fn select_tab(app: &mut PhotocraftApp, p: Panel) {
    let (g, i) = locate(app, p);
    *g.tab_mut(&mut app.ui.dock_tabs) = i;
}

/// Bring `p` up wherever it is docked: its group shown and expanded, `p` selected and unhidden.
pub fn show_panel(app: &mut PhotocraftApp, p: Panel) {
    let (g, i) = locate(app, p);
    *g.tab_mut(&mut app.ui.dock_tabs) = i;
    reveal(app, g);
}

/// Drop a dragged panel: move it, select it where it lands (shown and expanded), and keep the
/// tab its old group had selected.
pub fn drop_panel(app: &mut PhotocraftApp, p: Panel, to: PanelDrop) -> bool {
    let pro = is_pro(app);
    let (from, _) = locate(app, p);
    let keep = app.ui.dock.panels(from, pro).get(*from.tab_mut(&mut app.ui.dock_tabs)).copied();
    let Some((group, index)) = app.ui.dock.move_panel(p, to, pro) else { return false };
    if let Some(k) = keep.filter(|k| *k != p)
        && let Some(i) = app.ui.dock.panels(from, pro).iter().position(|x| *x == k)
    {
        *from.tab_mut(&mut app.ui.dock_tabs) = i;
    } else if from != group {
        *from.tab_mut(&mut app.ui.dock_tabs) = 0;
    }
    *group.tab_mut(&mut app.ui.dock_tabs) = index;
    *group.shown_mut(&mut app.ui.panels) = true;
    app.ui.dock.set_collapsed(group, false);
    // Per-group state of user-made groups that are gone.
    let live: Vec<Group> = app.ui.dock.order();
    app.ui.dock_tabs.extra.retain(|n, _| live.contains(&Group::Extra(*n)));
    app.ui.panels.extra_groups.retain(|n, _| live.contains(&Group::Extra(*n)));
    true
}

/// A panel being dragged by its tab (egui memory): it and the group it came from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PanelDrag {
    panel: Panel,
}

fn panel_drag_id() -> egui::Id {
    egui::Id::new("dock-panel-drag")
}

/// Where a panel dragged to `p` would land: a group's tab strip (at the tab under the pointer),
/// a group's body (its last tab), or the gap between groups (a new group there).
fn panel_drop_at(rects: &[(Group, Rect)], strips: &[StripRects], strip_h: f32, layout: &DockLayout, pro: bool, p: egui::Pos2) -> Option<PanelDrop> {
    let first = rects.first()?;
    let last = rects.last()?;
    let column = Rect::from_min_max(pos2(first.1.left(), first.1.top() - GAP), pos2(last.1.right(), last.1.bottom() + 3.0 * GAP));
    if !column.contains(p) {
        return None;
    }
    // Edges: within this of a group's top or bottom border is a gap between groups.
    let edge = GAP / 2.0 + 4.0;
    for (k, (g, r)) in rects.iter().enumerate() {
        if p.y < r.top() + edge.min(r.height() / 3.0) && p.y >= r.top() - GAP - 1.0 {
            return Some(PanelDrop::NewGroup(Some(*g)));
        }
        let last_one = k + 1 == rects.len();
        if last_one && p.y > r.bottom() - edge.min(r.height() / 3.0) {
            return Some(PanelDrop::NewGroup(None));
        }
        if !r.contains(p) {
            continue;
        }
        let all = layout.panels(*g, pro).len();
        let strip = Rect::from_min_size(r.min, vec2(r.width(), strip_h));
        if strip.contains(p)
            && let Some(s) = strips.iter().find(|s| s.group == *g)
        {
            // Before the first tab whose middle is right of the pointer.
            let at = s.tabs.iter().find(|(_, tr)| p.x < tr.center().x).map_or(all, |(i, _)| *i);
            return Some(PanelDrop::Into(*g, at));
        }
        return Some(PanelDrop::Into(*g, all));
    }
    None
}

/// Per-group interactions collected while drawing, applied afterwards.
enum Action {
    ToggleCollapse(Group),
    Close(Group),
    CloseTab(Group, usize),
    Move(Group, Option<Group>),
}

/// Rects of the groups drawn last frame (screen points), for tests and automation.
pub fn last_rects(ctx: &egui::Context) -> Vec<(Group, Rect)> {
    ctx.data(|d| d.get_temp::<Vec<(Group, Rect)>>(rects_id())).unwrap_or_default()
}

fn rects_id() -> egui::Id {
    egui::Id::new("dock-group-rects")
}

/// A group's tab strip as drawn last frame (screen points), for tests and automation.
#[derive(Clone, Debug, PartialEq)]
pub struct StripRects {
    pub group: Group,
    /// `(tab index, rect)` of the tabs on the strip (the others are in the chevron menu).
    pub tabs: Vec<(usize, Rect)>,
    /// The panel menu button.
    pub menu: Rect,
    /// The » overflow button, when some tabs didn't fit.
    pub chevron: Option<Rect>,
}

/// The tab strips drawn last frame.
pub fn last_strips(ctx: &egui::Context) -> Vec<StripRects> {
    ctx.data(|d| d.get_temp::<Vec<StripRects>>(strips_id())).unwrap_or_default()
}

fn strips_id() -> egui::Id {
    egui::Id::new("dock-strip-rects")
}

/// Draw the `shown` groups (any order; the layout decides) filling `ui`. `body` draws one
/// group's tab content.
pub fn show(app: &mut PhotocraftApp, ui: &mut egui::Ui, shown: &[Group], mut body: impl FnMut(&mut PhotocraftApp, &mut egui::Ui, Panel)) {
    let t = Tokens::get(ui.ctx());
    let strip = if t.pro { 28.0 } else { 40.0 };
    // Groups with no visible panel (all moved away or closed) take no room.
    let order: Vec<Group> = app.ui.dock.order().into_iter().filter(|g| shown.contains(g) && !app.ui.dock.visible_panels(*g, t.pro).is_empty()).collect();
    let area = ui.available_rect_before_wrap();
    let heights = app.ui.dock.heights_for(&order, area.height(), strip);
    let locked = app.session.prefs().workspace_locked;
    let rects = rects_after_layout(&heights, area);
    let mut actions: Vec<Action> = Vec::new();
    let mut dragging: Option<Group> = None;
    // A panel dragged by its tab (Photoshop): it moves alone, the strip background moves the group.
    let mut panel_drag: Option<PanelDrag> = ui.ctx().data(|d| d.get_temp::<Option<PanelDrag>>(panel_drag_id())).flatten();
    let mut panel_dropped: Option<(Panel, egui::Pos2)> = None;
    let mut strips: Vec<StripRects> = Vec::with_capacity(rects.len());
    for (i, (g, rect)) in rects.iter().copied().enumerate() {
        let collapsed = app.ui.dock.is_collapsed(g);
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("dock-group", g.key())).max_rect(rect));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        child.spacing_mut().item_spacing.y = if t.pro { 0.0 } else { 6.0 };
        let before = *g.tab_mut(&mut app.ui.dock_tabs);
        let visible = app.ui.dock.visible_panels(g, t.pro);
        if visible.is_empty() {
            continue;
        }
        let indices: Vec<usize> = visible.iter().map(|(i, _)| *i).collect();
        let panels: Vec<Panel> = visible.iter().map(|(_, p)| *p).collect();
        let tabs: Vec<&str> = panels.iter().map(|p| p.name()).collect();
        let mut sel = indices.iter().position(|i| *i == before).unwrap_or(0);
        let resp = widgets::card_ex(&mut child, g.key(), &tabs, &mut sel, collapsed, |ui, shown_tab| {
            let Some(&panel) = panels.get(shown_tab) else { return };
            let inner = ui.available_height().max(0.0);
            if panel.scrolls_itself() {
                ui.set_min_height(inner);
                body(app, ui, panel);
            } else {
                // The pro Color field stretches to the group's height instead of a fixed 120 pt.
                if t.pro && panel == Panel::Color {
                    ui.data_mut(|d| d.insert_temp(crate::panels::color_field_fill_id(), inner));
                }
                egui::ScrollArea::vertical()
                    .id_salt(("dock-scroll", g.key(), panel.name()))
                    .max_height(inner)
                    .auto_shrink([false, false])
                    .show(ui, |ui| body(app, ui, panel));
            }
        });
        strips.push(StripRects {
            group: g,
            tabs: resp.tabs.iter().filter_map(|(i, r)| indices.get(*i).map(|original| (*original, *r))).collect(),
            menu: resp.menu.rect,
            chevron: resp.chevron,
        });
        // The strip uses visible indices; dockTabs and the panel bodies use original indices.
        if let Some(&picked) = indices.get(sel)
            && picked != before
        {
            *g.tab_mut(&mut app.ui.dock_tabs) = picked;
        }
        if let Some(context) = resp.tab_context {
            match context {
                crate::tab_strip::TabContextAction::Close(i) => {
                    if let Some(&tab) = indices.get(i) {
                        actions.push(Action::CloseTab(g, tab));
                    }
                }
                crate::tab_strip::TabContextAction::CloseGroup => actions.push(Action::Close(g)),
            }
        }
        if resp.strip.double_clicked() || resp.tab_double_clicked || (collapsed && resp.tab_clicked) {
            actions.push(Action::ToggleCollapse(g));
        }
        if !locked
            && resp.strip.drag_started()
            && let Some(o) = ui.input(|i| i.pointer.press_origin())
            && let Some(&(vis, _)) = resp.tabs.iter().find(|(_, r)| r.contains(o))
            && let Some(&panel) = panels.get(vis)
        {
            panel_drag = Some(PanelDrag { panel });
        }
        let this_panel_drag = panel_drag.is_some_and(|d| panels.contains(&d.panel));
        if !locked && resp.strip.dragged() && !this_panel_drag {
            dragging = Some(g);
        }
        if !locked
            && resp.strip.drag_stopped()
            && let Some(p) = ui.ctx().pointer_interact_pos()
        {
            match panel_drag.filter(|_| this_panel_drag) {
                Some(d) => panel_dropped = Some((d.panel, p)),
                None => actions.push(Action::Move(g, drop_before(&order, &rects, g, p.y))),
            }
        }
        if dragging == Some(g) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        egui::Popup::menu(&resp.menu).show(|ui| {
            crate::widgets::style_spectrum_popup_menu(ui);
            ui.set_min_width(170.0);
            if panels.get(sel) == Some(&Panel::Layers) {
                crate::layer_row_ui::panel_menu(app, ui);
                ui.separator();
            }
            if panels.get(sel) == Some(&Panel::Swatches) {
                crate::swatches_ui::panel_menu(app, ui);
                ui.separator();
            }
            if ui.button(if collapsed { tl!("Expand Panel Group") } else { tl!("Collapse Panel Group") }).clicked() {
                actions.push(Action::ToggleCollapse(g));
                ui.close();
            }
            let pos = order.iter().position(|x| *x == g).unwrap_or(0);
            if ui.add_enabled(!locked && pos > 0, egui::Button::new(tl!("Move Group Up"))).clicked() {
                actions.push(Action::Move(g, order.get(pos.saturating_sub(1)).copied()));
                ui.close();
            }
            if ui.add_enabled(!locked && pos + 1 < order.len(), egui::Button::new(tl!("Move Group Down"))).clicked() {
                actions.push(Action::Move(g, order.get(pos + 2).copied()));
                ui.close();
            }
            ui.separator();
            if ui.button(tl!("Close Tab Group")).clicked() {
                actions.push(Action::Close(g));
                ui.close();
            }
        });
        // Splitter in the gap below this group: resizes it, pushing the neighbouring groups.
        if i + 1 < rects.len() && !collapsed && rects.iter().skip(i + 1).any(|(n, _)| !app.ui.dock.is_collapsed(*n)) {
            let gap = Rect::from_min_size(pos2(rect.left(), rect.bottom()), vec2(rect.width(), GAP)).expand2(vec2(0.0, 2.0));
            let sresp = ui.interact(gap, ui.id().with(("dock-splitter", g.key())), Sense::drag());
            if sresp.hovered() || sresp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                ui.painter().line_segment([gap.left_center(), gap.right_center()], Stroke::new(2.0, t.accent.gamma_multiply(0.7)));
            }
            if sresp.dragged() {
                resize(&mut app.ui.dock, &heights, i, sresp.drag_delta().y);
            }
        }
    }
    // Drop indicator while a group is dragged by its tab strip.
    if let (Some(g), Some(p)) = (dragging, ui.ctx().pointer_interact_pos()) {
        let before = drop_before(&order, &rects, g, p.y);
        let line_y = match before.and_then(|b| rects.iter().find(|(x, _)| *x == b)) {
            Some((_, r)) => r.top() - GAP / 2.0,
            None => rects.last().map_or(area.top(), |(_, r)| r.bottom() + GAP / 2.0),
        };
        ui.painter().line_segment([pos2(area.left(), line_y), pos2(area.right(), line_y)], Stroke::new(3.0, t.accent));
    }
    // While a panel is dragged: where it would land (a group outlined with the insertion point
    // on its strip, or a line where a new group would go) and its name at the pointer.
    if let (Some(d), Some(p), None) = (panel_drag, ui.ctx().pointer_interact_pos(), panel_dropped) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let fg = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.id().with("dock-panel-drag")));
        match panel_drop_at(&rects, &strips, strip, &app.ui.dock, t.pro, p) {
            Some(PanelDrop::Into(g, at)) => {
                if let Some((_, r)) = rects.iter().find(|(x, _)| *x == g) {
                    fg.rect_stroke(*r, t.radius_sm, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
                    if let Some(s) = strips.iter().find(|s| s.group == g) {
                        let x = s.tabs.iter().find(|(i, _)| *i >= at).map_or_else(|| s.tabs.last().map_or(r.left() + 6.0, |(_, tr)| tr.right()), |(_, tr)| tr.left());
                        fg.line_segment([pos2(x, r.top() + 3.0), pos2(x, r.top() + strip - 3.0)], Stroke::new(3.0, t.accent));
                    }
                }
            }
            Some(PanelDrop::NewGroup(before)) => {
                let y = match before.and_then(|b| rects.iter().find(|(x, _)| *x == b)) {
                    Some((_, r)) => r.top() - GAP / 2.0,
                    None => rects.last().map_or(area.top(), |(_, r)| r.bottom() + GAP / 2.0),
                };
                fg.line_segment([pos2(area.left(), y), pos2(area.right(), y)], Stroke::new(3.0, t.accent));
            }
            None => {}
        }
        let label = egui::WidgetText::from(tl!(d.panel.name())).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
        let chip = Rect::from_min_size(p + vec2(14.0, 10.0), label.size() + vec2(16.0, 8.0));
        fg.rect_filled(chip, t.radius_sm, t.card);
        fg.rect_stroke(chip, t.radius_sm, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        fg.galley(chip.min + vec2(8.0, 4.0), label, t.text);
    }
    let landed = panel_dropped.and_then(|(panel, p)| panel_drop_at(&rects, &strips, strip, &app.ui.dock, t.pro, p).map(|to| (panel, to)));
    let still_dragging = panel_dropped.is_none() && ui.input(|i| i.pointer.any_down());
    ui.ctx().data_mut(|d| {
        d.insert_temp(rects_id(), rects);
        d.insert_temp(strips_id(), strips);
        d.insert_temp::<Option<PanelDrag>>(panel_drag_id(), panel_drag.filter(|_| still_dragging));
    });
    if let Some((panel, to)) = landed {
        drop_panel(app, panel, to);
    }
    ui.advance_cursor_after_rect(area);
    for a in actions {
        match a {
            Action::ToggleCollapse(g) => {
                let on = !app.ui.dock.is_collapsed(g);
                app.ui.dock.set_collapsed(g, on);
            }
            Action::Close(g) => *g.shown_mut(&mut app.ui.panels) = false,
            Action::CloseTab(g, tab) => {
                app.ui.dock.hide_tab(g, tab, t.pro);
                let remaining = app.ui.dock.visible_tabs(g, t.pro);
                if remaining.is_empty() {
                    *g.shown_mut(&mut app.ui.panels) = false;
                } else if *g.tab_mut(&mut app.ui.dock_tabs) == tab {
                    // Prefer the next tab, or the previous one when closing the last.
                    let next = remaining.iter().find(|(i, _)| *i > tab).or_else(|| remaining.last());
                    if let Some(&(next, _)) = next {
                        *g.tab_mut(&mut app.ui.dock_tabs) = next;
                    }
                }
            }
            Action::Move(g, before) => {
                if before != Some(g) {
                    app.ui.dock.move_group(g, before);
                }
            }
        }
    }
}

fn rects_after_layout(heights: &[(Group, f32)], area: Rect) -> Vec<(Group, Rect)> {
    let mut y = area.top();
    heights
        .iter()
        .map(|(g, h)| {
            let r = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h.max(0.0)));
            y = r.bottom() + GAP;
            (*g, r)
        })
        .collect()
}

/// The group the dragged one lands before when released at `y` (`None` = last).
fn drop_before(order: &[Group], rects: &[(Group, Rect)], dragged: Group, y: f32) -> Option<Group> {
    let hit = rects.iter().find(|(_, r)| y < r.center().y).map(|(g, _)| *g);
    match hit {
        // Dropping onto itself, or just below itself, keeps the place.
        Some(g) if g == dragged => Some(dragged),
        Some(g) => {
            let after_self = order.iter().position(|x| *x == dragged).zip(order.iter().position(|x| *x == g)).is_some_and(|(a, b)| b == a + 1);
            if after_self { Some(dragged) } else { Some(g) }
        }
        None => None,
    }
}

/// Splitter `i` (below group `i`) dragged by `dy`. Like Photoshop, the groups on the side the
/// splitter moves into shrink in turn, nearest first, each down to its minimum (#2573), and the
/// group on the other side grows by what they gave. The filler absorbs its share implicitly,
/// so the column height stays the same.
fn resize(layout: &mut DockLayout, heights: &[(Group, f32)], i: usize, dy: f32) {
    if !dy.is_finite() || dy == 0.0 {
        return;
    }
    let expanded = |k: &usize| heights.get(*k).is_some_and(|(g, _)| !layout.is_collapsed(*g));
    let filler = (0..heights.len()).rev().find(expanded);
    let Some(j) = (i + 1..heights.len()).find(expanded) else { return };
    if !expanded(&i) {
        return;
    }
    // Dragging down grows group `i` at the expense of the groups below; dragging up grows the
    // next expanded group at the expense of `i` and the groups above it.
    let (grow, shrink): (usize, Vec<usize>) =
        if dy > 0.0 { (i, (j..heights.len()).filter(expanded).collect()) } else { (j, (0..=i).rev().filter(expanded).collect()) };
    // The first drag pins the other groups at the heights they show, so groups still at their
    // defaults (which give way to the filler) don't shift while this one is resized.
    for (k, (o, oh)) in heights.iter().enumerate() {
        if Some(k) != filler && !layout.is_collapsed(*o) {
            layout.heights.entry(*o).or_insert(*oh);
        }
    }
    let mut left = dy.abs();
    for k in shrink {
        if left <= 0.0 {
            break;
        }
        let Some(&(o, oh)) = heights.get(k) else { continue };
        let give = (oh - o.min_height()).max(0.0).min(left);
        left -= give;
        if Some(k) != filler {
            layout.heights.insert(o, oh - give);
        }
    }
    if let Some(&(o, oh)) = heights.get(grow)
        && Some(grow) != filler
    {
        layout.heights.insert(o, oh + (dy.abs() - left));
    }
}

/// What `prefs.panelLayout` holds: the live layout and open panels.
fn snapshot(app: &PhotocraftApp) -> Value {
    json!({"workspace": app.ui.workspace, "panels": app.ui.panels, "dockTabs": app.ui.dock_tabs, "dock": app.ui.dock, "timelineOpen": app.ui.timeline.open})
}

/// Remember the layout in the preferences once the user lets go of the mouse (Workspace ›
/// Remember Workspace Changes). Cheap: a small JSON compare per frame.
pub fn persist(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if !app.session.prefs().workspace.remember_workspace_changes || ctx.input(|i| i.pointer.any_down()) {
        return;
    }
    let now = snapshot(app);
    if app.session.prefs().panel_layout != now {
        app.session.prefs.edit(|p| p.panel_layout = now);
    }
}

/// Restore the remembered layout at launch. Unreadable parts keep their defaults.
pub fn restore(app: &mut PhotocraftApp) {
    if !app.session.prefs().workspace.remember_workspace_changes {
        return;
    }
    let saved = app.session.prefs().panel_layout.clone();
    apply(app, &saved);
    if let Some(ws) = saved.get("workspace").and_then(Value::as_str) {
        app.ui.workspace = ws.to_string();
    }
}

/// Apply the `panels`, `dockTabs`, `dock` and `timelineOpen` parts of a saved layout
/// (a workspace or `panelLayout`). Missing or invalid dock parts are left alone; old
/// layouts without Timeline visibility restore it closed.
pub fn apply(app: &mut PhotocraftApp, v: &Value) {
    if let Some(p) = v.get("panels").and_then(|p| serde_json::from_value(p.clone()).ok()) {
        app.ui.panels = p;
    }
    if let Some(t) = v.get("dockTabs").and_then(|t| serde_json::from_value(t.clone()).ok()) {
        app.ui.dock_tabs = t;
    }
    if let Some(d) = v.get("dock").and_then(|d| serde_json::from_value(d.clone()).ok()) {
        app.ui.dock = d;
    }
    app.ui.timeline.open = v.get("timelineOpen").and_then(Value::as_bool).unwrap_or(false);
    if !app.ui.timeline.open {
        app.ui.timeline.playing = false;
    }
}

#[cfg(test)]
#[path = "dock_tests.rs"]
mod tests;
