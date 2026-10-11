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
    /// Brush Settings | Brushes (Window › Brush Settings, F5): floating unless docked.
    Brush,
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
    ColorWheel,
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
    BrushSettings,
    Brushes,
}

impl Panel {
    pub const ALL: [Panel; 20] = [
        Panel::Color,
        Panel::ColorWheel,
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
        Panel::BrushSettings,
        Panel::Brushes,
    ];

    /// The tab label (English; translated where it is drawn).
    pub fn name(self) -> &'static str {
        match self {
            Panel::Color => "Color",
            Panel::ColorWheel => "Color Wheel",
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
            Panel::BrushSettings => "Brush Settings",
            Panel::Brushes => "Brushes",
        }
    }

    /// The panel with this tab label.
    pub fn from_name(name: &str) -> Option<Panel> {
        Panel::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The group the panel belongs to in the default workspace.
    pub fn home(self) -> Group {
        match self {
            Panel::Color | Panel::ColorWheel | Panel::Swatches | Panel::Gradients | Panel::Patterns => Group::Color,
            Panel::Properties | Panel::Adjustments => Group::Properties,
            Panel::Character | Panel::Paragraph => Group::Character,
            Panel::Navigator | Panel::Histogram | Panel::Info => Group::Navigator,
            Panel::History | Panel::Actions | Panel::LayerComps => Group::History,
            Panel::Layers | Panel::Channels | Panel::Paths => Group::Layers,
            Panel::BrushSettings | Panel::Brushes => Group::Brush,
        }
    }

    /// Panels that lay out their own scrolling list and footer (they fill the group).
    pub fn scrolls_itself(self) -> bool {
        matches!(self, Panel::Layers | Panel::History | Panel::BrushSettings | Panel::Brushes | Panel::ColorWheel)
    }

    /// The icon a panel shows when collapsed into the icon rail.
    pub fn icon(self) -> &'static str {
        match self {
            Panel::Color => "droplet",
            Panel::ColorWheel => "palette",
            Panel::Swatches => "swatch-book",
            Panel::Gradients => "blend",
            Panel::Patterns => "grid-3x3",
            Panel::Properties => "sliders-horizontal",
            Panel::Adjustments => "adjustment-layer",
            Panel::Character => "type",
            Panel::Paragraph => "align-left",
            Panel::Navigator => "navigation",
            Panel::Histogram => "gauge",
            Panel::Info => "info",
            Panel::History => "clock",
            Panel::Actions => "play",
            Panel::LayerComps => "copy",
            Panel::Layers => "layers",
            Panel::Channels => "layer-mask",
            Panel::Paths => "pen-tool",
            Panel::BrushSettings => "brush",
            Panel::Brushes => "brush-cleaning",
        }
    }

    /// The shortest a group holding this panel can be (Brush Settings' section list and strip).
    pub fn min_height(self) -> f32 {
        match self {
            Panel::BrushSettings | Panel::Brushes => 360.0,
            _ => 0.0,
        }
    }

    /// The narrowest a group showing this panel can be (Brush Settings' two columns).
    pub fn min_width(self) -> f32 {
        match self {
            Panel::BrushSettings | Panel::Brushes => 460.0,
            _ => 0.0,
        }
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
            Group::Brush => "brushSettings",
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
            Group::Brush => 640.0,
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
            Group::Brush => 420.0,
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
            Group::Brush => 300.0,
            _ => 80.0,
        }
    }

    pub fn tabs(self, pro: bool) -> &'static [&'static str] {
        match self {
            // Photoshop Essentials: Color | Swatches | Gradients | Patterns.
            Group::Color if pro => &["Color", "Color Wheel", "Swatches", "Gradients", "Patterns"],
            Group::Color => &["Swatches", "Color Wheel", "Color", "Gradients", "Patterns"],
            Group::Properties => &["Properties", "Adjustments"],
            Group::Character => &["Character", "Paragraph"],
            Group::Navigator => &["Navigator", "Histogram", "Info"],
            Group::History => &["History", "Actions", "Layer Comps"],
            Group::Layers => &["Layers", "Channels", "Paths"],
            Group::Brush => &["Brush Settings", "Brushes"],
            Group::Extra(_) => &[],
        }
    }

    /// The group's panels in the default workspace, in tab order ([`Self::tabs`]' panels).
    pub fn home_panels(self, pro: bool) -> &'static [Panel] {
        match self {
            // The Color Wheel between Color and Swatches.
            Group::Color if pro => &[Panel::Color, Panel::ColorWheel, Panel::Swatches, Panel::Gradients, Panel::Patterns],
            Group::Color => &[Panel::Swatches, Panel::ColorWheel, Panel::Color, Panel::Gradients, Panel::Patterns],
            Group::Properties => &[Panel::Properties, Panel::Adjustments],
            Group::Character => &[Panel::Character, Panel::Paragraph],
            Group::Navigator => &[Panel::Navigator, Panel::Histogram, Panel::Info],
            Group::History => &[Panel::History, Panel::Actions, Panel::LayerComps],
            Group::Layers => &[Panel::Layers, Panel::Channels, Panel::Paths],
            Group::Brush => &[Panel::BrushSettings, Panel::Brushes],
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
            Group::Brush => &mut tabs.brush,
            Group::Extra(n) => tabs.extra.entry(n).or_insert(0),
        }
    }

    /// The group for a `panels` / `dockTabs` key ("color", "properties", …, "extra3").
    pub fn from_key(key: &str) -> Option<Group> {
        if let Some(n) = key.strip_prefix("extra").and_then(|n| n.parse::<u8>().ok()).filter(|n| *n < MAX_EXTRA) {
            return Some(Group::Extra(n));
        }
        Group::ALL.into_iter().chain([Group::Brush]).find(|g| g.key() == key)
    }

    pub fn shown(self, panels: &Panels) -> bool {
        match self {
            Group::Color => panels.color,
            Group::Properties => panels.properties,
            Group::Character => panels.character,
            Group::Navigator => panels.navigator,
            Group::History => panels.history,
            Group::Layers => panels.layers,
            Group::Brush => panels.brush_settings,
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
            Group::Brush => &mut panels.brush_settings,
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
    /// Floating groups and their window rectangles (`[x, y, width, height]`, points). Brush
    /// Settings floats unless docked (in `order`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub floating: BTreeMap<Group, [f32; 4]>,
    /// Groups collapsed to icons in the icon rail (Photoshop's iconic column), top to bottom: a
    /// click on an icon opens its panel beside the rail.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rail: Vec<Group>,
}

/// Where a dragged panel goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelDrop {
    /// Into a group's tabs at this index (counting all its tabs, hidden ones included).
    Into(Group, usize),
    /// A new group of its own, just before this one (`None` = last).
    NewGroup(Option<Group>),
    /// A floating window of its own at this point (screen points), torn out of its group.
    Float([f32; 2]),
    /// A group of its own in the icon rail, at this position among the rail's groups.
    Rail(usize),
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
        self.groups().into_iter().find_map(|g| self.panels(g, pro).iter().position(|x| *x == p).map(|i| (g, i))).unwrap_or((p.home(), 0))
    }

    /// A group number free for a new user-made group.
    fn free_extra(&self) -> Option<u8> {
        (0..MAX_EXTRA).find(|n| {
            let g = Group::Extra(*n);
            !self.tabs.get(&g).is_some_and(|l| !l.is_empty()) && !self.order.contains(&g) && !self.floating.contains_key(&g) && !self.rail.contains(&g)
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
            PanelDrop::Float(at) => {
                // Already alone in a window: dragging it is moving that window.
                if src.len() == 1 && self.is_floating(from) {
                    return None;
                }
                let g = Group::Extra(self.free_extra()?);
                self.tabs.insert(g, vec![p]);
                let (w, h) = if p.min_width() > 0.0 { (p.min_width().max(562.0), 662.0) } else { (300.0, 380.0) };
                let (x, y) = (if at[0].is_finite() { at[0] } else { 0.0 }, if at[1].is_finite() { at[1] } else { 0.0 });
                self.floating.insert(g, [x - 24.0, y - 12.0, w, h]);
                (g, 0)
            }
            PanelDrop::Rail(i) => {
                // Already alone there: keep its place.
                if src.len() == 1 && self.rail.iter().position(|x| *x == from).is_some_and(|at| at == i || at + 1 == i) {
                    return None;
                }
                let g = Group::Extra(self.free_extra()?);
                self.tabs.insert(g, vec![p]);
                self.rail.insert(i.min(self.rail.len()), g);
                (g, 0)
            }
        };
        src.retain(|x| *x != p);
        self.tabs.insert(from, src);
        self.prune_extra();
        Some((group, index))
    }

    /// Dock the floating (or iconic) group `g` as a whole: its panels into another group's tabs, or
    /// the group itself into the column or the icon rail. Returns where it is now; `None` for a drop
    /// that leaves it floating.
    pub fn dock_group(&mut self, g: Group, to: PanelDrop, pro: bool) -> Option<(Group, usize)> {
        match to {
            PanelDrop::Into(t, i) if t != g => {
                let moving = self.panels(g, pro);
                let first = moving.first().copied()?;
                let mut dst = self.panels(t, pro);
                let i = i.min(dst.len());
                for (k, p) in moving.iter().enumerate() {
                    dst.insert(i + k, *p);
                }
                self.tabs.insert(t, dst);
                self.tabs.insert(g, Vec::new());
                self.prune_extra();
                self.locate(first, pro).into()
            }
            PanelDrop::NewGroup(before) => {
                self.floating.remove(&g);
                self.rail.retain(|x| *x != g);
                let mut order = self.order();
                order.retain(|x| *x != g);
                let pos = before.and_then(|b| order.iter().position(|x| *x == b)).unwrap_or(order.len());
                order.insert(pos, g);
                self.order = order;
                Some((g, 0))
            }
            PanelDrop::Rail(i) => {
                self.floating.remove(&g);
                self.order.retain(|x| *x != g);
                self.rail.retain(|x| *x != g);
                self.rail.insert(i.min(self.rail.len()), g);
                Some((g, 0))
            }
            _ => None,
        }
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
            self.floating.remove(&g);
            self.rail.retain(|x| *x != g);
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

    /// Every group: the docked ones in display order, then the floating ones.
    pub fn groups(&self) -> Vec<Group> {
        let mut out = self.order();
        for g in self.floating.keys().copied().chain(self.rail.iter().copied()).chain([Group::Brush]) {
            if !out.contains(&g) {
                out.push(g);
            }
        }
        out
    }

    /// Is `g` a floating window rather than part of the dock column?
    pub fn is_floating(&self, g: Group) -> bool {
        !self.rail.contains(&g) && (self.floating.contains_key(&g) || (g == Group::Brush && !self.order.contains(&g)))
    }

    /// Is `g` collapsed to icons in the icon rail?
    pub fn is_iconic(&self, g: Group) -> bool {
        self.rail.contains(&g)
    }

    /// Every docked group once, in display order (floating ones are left out).
    pub fn order(&self) -> Vec<Group> {
        let mut out: Vec<Group> = Vec::with_capacity(Group::ALL.len());
        for g in &self.order {
            if !out.contains(g) && !self.floating.contains_key(g) && !self.rail.contains(g) {
                out.push(*g);
            }
        }
        // Missing groups slot in below their default predecessor, so a group new to an old
        // saved layout (Character) never lands below Layers and takes over as the filler.
        for (i, g) in Group::ALL.iter().enumerate() {
            if out.contains(g) || self.floating.contains_key(g) || self.rail.contains(g) {
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

    /// The shortest `g` can be: its own minimum, or more for the panels it holds now.
    pub fn min_height(&self, g: Group) -> f32 {
        self.panels(g, true).iter().map(|p| p.min_height()).fold(g.min_height(), f32::max)
    }

    /// `g`'s height until resized: its default, at least what its panels need.
    fn default_height(&self, g: Group) -> f32 {
        g.default_height().max(self.min_height(g) + 40.0)
    }

    /// What `g` gives way down to in a short column, never below what its panels need.
    fn compact_height(&self, g: Group) -> f32 {
        g.compact_height().max(self.min_height(g))
    }

    /// The stored (or default) height, sanitised.
    pub fn height(&self, g: Group) -> f32 {
        match self.heights.get(&g) {
            Some(h) if h.is_finite() => h.clamp(self.min_height(g), MAX_HEIGHT),
            _ => self.default_height(g),
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
            let min_fill = shown.get(f).map_or(0.0, |g| self.min_height(*g));
            let pref_fill = shown.get(f).map_or(0.0, |g| g.preferred_fill().max(self.min_height(*g)));
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
                    let give = (*h - self.compact_height(*g)).max(0.0).min(deficit);
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
                    let give = (*h - self.min_height(*g)).max(0.0).min(deficit);
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

/// The panel a `window.toggle.<key>` item stands for (the dock and bar toggles have none).
pub fn toggle_panel(key: &str) -> Option<Panel> {
    Some(match key {
        "layers" => Panel::Layers,
        "history" => Panel::History,
        "properties" => Panel::Properties,
        "color" => Panel::Color,
        "navigator" => Panel::Navigator,
        "brushSettings" => Panel::BrushSettings,
        _ => return None,
    })
}

/// Is the group holding `p` open (docked or floating)?
pub fn group_shown(app: &PhotocraftApp, p: Panel) -> bool {
    locate(app, p).0.shown(&app.ui.panels)
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
    let live: Vec<Group> = app.ui.dock.groups();
    app.ui.dock_tabs.extra.retain(|n, _| live.contains(&Group::Extra(*n)));
    app.ui.panels.extra_groups.retain(|n, _| live.contains(&Group::Extra(*n)));
    true
}

/// A floating window being dragged by its body (egui memory): over the dock or the rail it
/// docks as a whole.
fn group_drag_id() -> egui::Id {
    egui::Id::new("dock-group-drag")
}

/// Where a floating window dragged to `p` would dock: the column or the rail (`None`: it just moves).
fn group_drop_target(ctx: &egui::Context, layout: &DockLayout, pro: bool, p: egui::Pos2) -> Option<PanelDrop> {
    let frame = ctx.cumulative_frame_nr();
    if let Some(r) = ctx.data(|d| d.get_temp::<RailFrame>(rail_frame_id())).filter(|r| r.frame == frame && r.rect.contains(p)) {
        let mut groups: Vec<Group> = Vec::new();
        for (g, ir) in &r.icons {
            if ir.center().y < p.y && !groups.contains(g) {
                groups.push(*g);
            }
        }
        return Some(PanelDrop::Rail(groups.len()));
    }
    let c = ctx.data(|d| d.get_temp::<DockColumn>(column_id())).filter(|c| c.frame == frame)?;
    panel_drop_at(&c.rects, &c.strips, c.strip_h, layout, pro, p)
}

/// Finish or preview a floating window dragged by its body (see [`group_drop_target`]).
fn end_group_drag(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let Some(g) = ctx.data(|x| x.get_temp::<Option<Group>>(group_drag_id())).flatten() else { return };
    let t = Tokens::get(ctx);
    let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    let to = group_drop_target(ctx, &app.ui.dock, t.pro, p);
    if !ctx.input(|i| i.pointer.any_down()) {
        ctx.data_mut(|x| x.insert_temp::<Option<Group>>(group_drag_id(), None));
        if let Some(to) = to.filter(|_| !app.session.prefs().workspace_locked)
            && let Some((group, index)) = app.ui.dock.dock_group(g, to, t.pro)
        {
            *group.tab_mut(&mut app.ui.dock_tabs) = index;
            *group.shown_mut(&mut app.ui.panels) = true;
            app.ui.dock.set_collapsed(group, false);
        }
        return;
    }
    if let Some(to) = to {
        preview_drop(ctx, &t, to, None);
    }
}

/// A panel being dragged by its tab (egui memory): it and the group it came from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PanelDrag {
    panel: Panel,
}

fn panel_drag_id() -> egui::Id {
    egui::Id::new("dock-panel-drag")
}

/// The dock column as drawn this frame (for dropping panels after the floating windows drew).
#[derive(Clone, Debug)]
struct DockColumn {
    frame: u64,
    rects: Vec<(Group, Rect)>,
    strips: Vec<StripRects>,
    strip_h: f32,
    area: Rect,
}

fn column_id() -> egui::Id {
    egui::Id::new("dock-column-frame")
}

/// A floating window as drawn this frame: its group, its rectangle and its tabs.
#[derive(Clone, Debug)]
struct FloatingFrame {
    frame: u64,
    group: Group,
    rect: Rect,
    header: Rect,
    /// `(tab index, rect)`.
    tabs: Vec<(usize, Rect)>,
}

fn floating_frames_id() -> egui::Id {
    egui::Id::new("dock-floating-frames")
}

/// The icon rail as drawn this frame: its rectangle and the icons of the groups collapsed into it.
#[derive(Clone, Debug)]
struct RailFrame {
    frame: u64,
    rect: Rect,
    icons: Vec<(Group, Rect)>,
}

fn rail_frame_id() -> egui::Id {
    egui::Id::new("dock-rail-frame")
}

/// The rail group whose panel is popped out beside the rail (egui memory; one at a time).
fn rail_popout_id() -> egui::Id {
    egui::Id::new("dock-rail-popout")
}

/// Draw the panels collapsed to icons in the icon rail (below its fixed buttons), inside the rail
/// `rail` (its whole rectangle). A click opens or closes the panel beside the rail; dragging an
/// icon moves the panel like dragging its tab.
pub fn rail_icons(app: &mut PhotocraftApp, ui: &mut egui::Ui, rail: Rect, box_size: f32) {
    let t = Tokens::get(ui.ctx());
    let locked = app.session.prefs().workspace_locked;
    let open: Option<Group> = ui.ctx().data(|d| d.get_temp::<Option<Group>>(rail_popout_id())).flatten();
    let mut icons: Vec<(Group, Rect)> = Vec::new();
    let groups = app.ui.dock.rail.clone();
    if !groups.is_empty() {
        ui.add_space(4.0);
        let r = ui.cursor();
        ui.painter().line_segment([pos2(r.left() + 4.0, r.top()), pos2(r.left() + box_size - 4.0, r.top())], Stroke::new(1.0, t.separator));
        ui.add_space(5.0);
    }
    let mut toggle: Option<(Group, usize)> = None;
    for g in groups {
        let selected = *g.tab_mut(&mut app.ui.dock_tabs);
        for (i, p) in app.ui.dock.visible_panels(g, t.pro) {
            let on = open == Some(g) && selected == i;
            let resp = crate::icons::rail_button_sense(ui, p.icon(), box_size, on, tl!(p.name()), Sense::click_and_drag());
            if resp.clicked() {
                toggle = Some((g, i));
            }
            if !locked && resp.drag_started() {
                ui.ctx().data_mut(|d| d.insert_temp::<Option<PanelDrag>>(panel_drag_id(), Some(PanelDrag { panel: p })));
            }
            icons.push((g, resp.rect));
        }
    }
    if let Some((g, i)) = toggle {
        let close = open == Some(g) && *g.tab_mut(&mut app.ui.dock_tabs) == i;
        *g.tab_mut(&mut app.ui.dock_tabs) = i;
        *g.shown_mut(&mut app.ui.panels) = true;
        ui.ctx().data_mut(|d| d.insert_temp::<Option<Group>>(rail_popout_id(), (!close).then_some(g)));
    }
    let frame = ui.ctx().cumulative_frame_nr();
    ui.ctx().data_mut(|d| d.insert_temp(rail_frame_id(), RailFrame { frame, rect: rail, icons }));
}

/// The rail group popped out beside the rail: its panel in a card next to the rail (left of it, or
/// of the dock when the dock is left of the rail), its tabs on top. It closes on a click elsewhere,
/// on its icon, or its close button.
fn rail_popout(app: &mut PhotocraftApp, ctx: &egui::Context, frames: &mut Vec<FloatingFrame>) {
    let Some(g) = ctx.data(|d| d.get_temp::<Option<Group>>(rail_popout_id())).flatten() else { return };
    let frame_nr = ctx.cumulative_frame_nr();
    let rail = ctx.data(|d| d.get_temp::<RailFrame>(rail_frame_id())).filter(|r| r.frame == frame_nr);
    let t = Tokens::get(ctx);
    let visible = app.ui.dock.visible_panels(g, t.pro);
    let (Some(rail), false) = (rail, visible.is_empty() || !app.ui.dock.is_iconic(g)) else {
        ctx.data_mut(|d| d.insert_temp::<Option<Group>>(rail_popout_id(), None));
        return;
    };
    let w = visible.iter().map(|(_, p)| p.min_width()).fold(300.0, f32::max).max(if visible.iter().any(|(_, p)| p.min_width() > 0.0) { 562.0 } else { 300.0 });
    let h: f32 = if visible.iter().any(|(_, p)| p.min_width() > 0.0) { 662.0 } else { 420.0 };
    let column_left = ctx.data(|d| d.get_temp::<DockColumn>(column_id())).filter(|c| c.frame == frame_nr).map(|c| c.area.left());
    let right = column_left.filter(|l| *l < rail.rect.left()).unwrap_or(rail.rect.left()) - 6.0;
    let top = rail.icons.iter().find(|(x, _)| *x == g).map_or(rail.rect.top(), |(_, r)| r.top());
    let screen = ctx.content_rect();
    let pos = pos2((right - w).max(screen.left() + 4.0), top.min(screen.bottom() - h - 4.0).max(screen.top() + 4.0));
    let locked = app.session.prefs().workspace_locked;
    let selected = *g.tab_mut(&mut app.ui.dock_tabs);
    let mut sel = visible.iter().position(|(i, _)| *i == selected).unwrap_or(0);
    let mut close = false;
    let id = egui::Id::new(("dock-rail-popout", g.key()));
    let out = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.card_border))
            .corner_radius(egui::CornerRadius::same(t.radius_lg as u8))
            .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: t.shadow })
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(w - 20.0);
                ui.set_height(h - 20.0);
                let mut tabs: Vec<(usize, Rect)> = Vec::new();
                let header = ui
                    .horizontal(|ui| {
                        for (k, (i, p)) in visible.iter().enumerate() {
                            let resp = widgets::pill_tab_sense(ui, p.name(), k == sel, Sense::click_and_drag());
                            if resp.clicked() {
                                sel = k;
                            }
                            if !locked && resp.drag_started() {
                                ui.ctx().data_mut(|d| d.insert_temp::<Option<PanelDrag>>(panel_drag_id(), Some(PanelDrag { panel: *p })));
                            }
                            tabs.push((*i, resp.rect));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::icons::button(ui, "x", 20.0, false, tl!("Close")).clicked() {
                                close = true;
                            }
                        });
                    })
                    .response
                    .rect;
                ui.add_space(6.0);
                widgets::hairline(ui);
                ui.add_space(6.0);
                if let Some(&(_, p)) = visible.get(sel) {
                    if p.scrolls_itself() {
                        ui.set_min_height(ui.available_height());
                        crate::panels::dock_body(app, ui, p);
                    } else {
                        egui::ScrollArea::vertical().id_salt(("rail-scroll", g.key(), p.name())).auto_shrink([false, false]).show(ui, |ui| crate::panels::dock_body(app, ui, p));
                    }
                }
                (header, tabs)
            })
            .inner
    });
    if let Some(&(i, _)) = visible.get(sel) {
        *g.tab_mut(&mut app.ui.dock_tabs) = i;
    }
    let rect = out.response.rect;
    let (header, tabs) = out.inner;
    // A press outside the card and the rail closes it (not while a panel is being dragged).
    let dragging = ctx.data(|d| d.get_temp::<Option<PanelDrag>>(panel_drag_id())).flatten().is_some();
    let pressed_outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p) && !rail.rect.contains(p)));
    if close || (pressed_outside && !dragging) {
        ctx.data_mut(|d| d.insert_temp::<Option<Group>>(rail_popout_id(), None));
    }
    frames.push(FloatingFrame { frame: frame_nr, group: g, rect, header, tabs });
}

/// The dock layout was replaced as a whole: floating windows take their place and size from it.
pub fn layout_replaced(app: &mut PhotocraftApp) {
    app.dock_generation = app.dock_generation.wrapping_add(1);
}

/// A floating window's egui id. Brush Settings keeps its old id until a layout is applied, so its
/// remembered place and size carry over; a new generation forgets egui's memory of all of them.
fn float_id(g: Group, generation: u64) -> egui::Id {
    match (g, generation) {
        (Group::Brush, 0) => egui::Id::new("brush-settings"),
        _ => egui::Id::new(("dock-float", g.key(), generation)),
    }
}

/// Where Brush Settings opens before it is moved: right of the canvas, below its top.
fn default_float_rect(g: Group, canvas: Rect) -> [f32; 4] {
    let (w, h) = if g == Group::Brush { (crate::brush_panel::WIDTH + 22.0, crate::brush_panel::DEFAULT_H) } else { (300.0, 380.0) };
    [(canvas.right() - w - 8.0).max(canvas.left() + 8.0), canvas.top() + 24.0, w, h]
}

/// Draw the floating panel groups (Brush Settings, and panels torn out of the dock): resizable
/// windows with the group's tabs on top (drag a tab to move that panel) and a close button. Their
/// rectangles are kept in the layout, so workspaces and the next launch restore them.
pub fn floating_windows(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let t = Tokens::get(ctx);
    let locked = app.session.prefs().workspace_locked;
    let canvas = app.last_canvas_rect;
    let frame_nr = ctx.cumulative_frame_nr();
    let mut frames: Vec<FloatingFrame> = Vec::new();
    let groups: Vec<Group> = app.ui.dock.groups().into_iter().filter(|g| app.ui.dock.is_floating(*g) && g.shown(&app.ui.panels)).collect();
    for g in groups {
        let visible = app.ui.dock.visible_panels(g, t.pro);
        if visible.is_empty() {
            continue;
        }
        let min_w = visible.iter().map(|(_, p)| p.min_width()).fold(220.0, f32::max);
        let min_h = if visible.iter().any(|(_, p)| p.min_width() > 0.0) { crate::brush_panel::MIN_SIZE.1 } else { 160.0 };
        let stored = app.ui.dock.floating.get(&g).copied();
        let r = stored.filter(|r| r.iter().all(|v| v.is_finite())).unwrap_or_else(|| default_float_rect(g, canvas));
        let id = float_id(g, app.dock_generation);
        let frame = egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.card_border))
            .corner_radius(egui::CornerRadius::same(t.radius_lg as u8))
            .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: t.shadow })
            .inner_margin(egui::Margin::same(10));
        let selected = *g.tab_mut(&mut app.ui.dock_tabs);
        let mut sel = visible.iter().position(|(i, _)| *i == selected).unwrap_or(0);
        let mut open = true;
        let title = visible.first().map_or("", |(_, p)| p.name());
        let shown = egui::Window::new(tl!(title))
            .id(id)
            .title_bar(false)
            // Drag an edge or the corner to resize (Photoshop's panels).
            .resizable(true)
            .default_pos(pos2(r[0], r[1]))
            .default_size(vec2(r[2].max(min_w), r[3].max(min_h)))
            .min_size(vec2(min_w, min_h))
            .frame(frame)
            .show(ctx, |ui| {
                let mut tabs: Vec<(usize, Rect)> = Vec::new();
                let header = ui
                    .horizontal(|ui| {
                        for (k, (i, p)) in visible.iter().enumerate() {
                            // A click picks the tab; a drag takes the panel.
                            let resp = widgets::pill_tab_sense(ui, p.name(), k == sel, Sense::click_and_drag());
                            if resp.clicked() {
                                sel = k;
                            }
                            if !locked && resp.drag_started() {
                                ui.ctx().data_mut(|d| d.insert_temp::<Option<PanelDrag>>(panel_drag_id(), Some(PanelDrag { panel: *p })));
                            }
                            tabs.push((*i, resp.rect));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::icons::button(ui, "x", 20.0, false, tl!("Close")).clicked() {
                                open = false;
                            }
                        });
                    })
                    .response
                    .rect;
                ui.add_space(6.0);
                widgets::hairline(ui);
                ui.add_space(6.0);
                if let Some(&(_, p)) = visible.get(sel) {
                    if p.scrolls_itself() {
                        crate::panels::dock_body(app, ui, p);
                    } else {
                        egui::ScrollArea::vertical().id_salt(("float-scroll", g.key(), p.name())).auto_shrink([false, false]).show(ui, |ui| crate::panels::dock_body(app, ui, p));
                    }
                }
                (header, tabs)
            });
        if let Some(&(i, _)) = visible.get(sel) {
            *g.tab_mut(&mut app.ui.dock_tabs) = i;
        }
        if !open {
            *g.shown_mut(&mut app.ui.panels) = false;
        }
        if let Some(w) = shown {
            // Dragged by its body: over the dock or the rail it docks as a whole on release.
            if !locked && w.response.dragged() && ctx.data(|d| d.get_temp::<Option<PanelDrag>>(panel_drag_id())).flatten().is_none() {
                ctx.data_mut(|d| d.insert_temp::<Option<Group>>(group_drag_id(), Some(g)));
            }
            let rect = w.response.rect;
            let (header, tabs) = w.inner.unwrap_or((Rect::NOTHING, Vec::new()));
            frames.push(FloatingFrame { frame: frame_nr, group: g, rect, header, tabs });
            // Keep where the user put it (Brush Settings only once it was moved or resized).
            let now = [rect.left(), rect.top(), rect.width(), rect.height()];
            let moved = stored.is_some() || g != Group::Brush || ctx.input(|i| i.pointer.any_down()) && w.response.rect != Rect::from_min_size(pos2(r[0], r[1]), vec2(r[2], r[3]));
            if moved && stored.is_none_or(|s| s.iter().zip(now).any(|(a, b)| (a - b).abs() > 0.5)) && !ctx.input(|i| i.pointer.any_down()) {
                app.ui.dock.floating.insert(g, now);
            }
        }
    }
    rail_popout(app, ctx, &mut frames);
    ctx.data_mut(|d| d.insert_temp(floating_frames_id(), frames));
}

/// Where a panel released at `p` goes: onto a floating window (its tab strip, or its last tab),
/// into the dock column, or anywhere else into a floating window of its own.
fn drop_target(ctx: &egui::Context, layout: &DockLayout, pro: bool, p: egui::Pos2) -> PanelDrop {
    let frame = ctx.cumulative_frame_nr();
    let floats = ctx.data(|d| d.get_temp::<Vec<FloatingFrame>>(floating_frames_id())).unwrap_or_default();
    // The topmost window under the pointer (drawn last).
    if let Some(f) = floats.iter().rev().find(|f| f.frame == frame && f.rect.contains(p)) {
        let all = layout.panels(f.group, pro).len();
        let at = if f.header.contains(p) { f.tabs.iter().find(|(_, tr)| p.x < tr.center().x).map_or(all, |(i, _)| *i) } else { all };
        return PanelDrop::Into(f.group, at);
    }
    // The icon rail: a group of its own there, before the first icon below the pointer.
    if let Some(r) = ctx.data(|d| d.get_temp::<RailFrame>(rail_frame_id())).filter(|r| r.frame == frame && r.rect.contains(p)) {
        let mut groups: Vec<Group> = Vec::new();
        for (g, ir) in &r.icons {
            if ir.center().y < p.y && !groups.contains(g) {
                groups.push(*g);
            }
        }
        return PanelDrop::Rail(groups.len());
    }
    if let Some(c) = ctx.data(|d| d.get_temp::<DockColumn>(column_id())).filter(|c| c.frame == frame)
        && let Some(to) = panel_drop_at(&c.rects, &c.strips, c.strip_h, layout, pro, p)
    {
        return to;
    }
    PanelDrop::Float([p.x, p.y])
}

/// After the dock and the floating windows drew: while a panel is dragged by its tab, show where it
/// would land (a group or window outlined with the insertion point, a line for a new group, the
/// panel's name at the pointer), and drop it where the button is released.
pub fn end_drag(app: &mut PhotocraftApp, ctx: &egui::Context) {
    end_group_drag(app, ctx);
    let Some(d) = ctx.data(|x| x.get_temp::<Option<PanelDrag>>(panel_drag_id())).flatten() else { return };
    let t = Tokens::get(ctx);
    let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    let to = drop_target(ctx, &app.ui.dock, t.pro, p);
    if !ctx.input(|i| i.pointer.any_down()) {
        ctx.data_mut(|x| x.insert_temp::<Option<PanelDrag>>(panel_drag_id(), None));
        if !app.session.prefs().workspace_locked {
            drop_panel(app, d.panel, to);
        }
        return;
    }
    preview_drop(ctx, &t, to, Some(d.panel));
}

/// Show where a drag would land: a group or window outlined with the insertion point, a line for a
/// new group or rail icon, the window a torn-out panel makes, and the dragged panel's name.
fn preview_drop(ctx: &egui::Context, t: &Tokens, to: PanelDrop, panel: Option<Panel>) {
    let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("dock-panel-drag-layer")));
    let frame = ctx.cumulative_frame_nr();
    let column = ctx.data(|x| x.get_temp::<DockColumn>(column_id())).filter(|c| c.frame == frame);
    let floats = ctx.data(|x| x.get_temp::<Vec<FloatingFrame>>(floating_frames_id())).unwrap_or_default();
    let outline = |r: Rect, tabs: &[(usize, Rect)], strip: Rect, at: usize| {
        fg.rect_stroke(r, t.radius_sm, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        let x = tabs.iter().find(|(i, _)| *i >= at).map_or_else(|| tabs.last().map_or(r.left() + 6.0, |(_, tr)| tr.right()), |(_, tr)| tr.left());
        fg.line_segment([pos2(x, strip.top() + 3.0), pos2(x, strip.bottom() - 3.0)], Stroke::new(3.0, t.accent));
    };
    match to {
        PanelDrop::Into(g, at) => {
            if let Some(f) = floats.iter().rev().find(|f| f.frame == frame && f.group == g) {
                outline(f.rect, &f.tabs, f.header, at);
            } else if let Some(c) = &column
                && let Some((_, r)) = c.rects.iter().find(|(x, _)| *x == g)
            {
                let tabs = c.strips.iter().find(|s| s.group == g).map(|s| s.tabs.clone()).unwrap_or_default();
                outline(*r, &tabs, Rect::from_min_size(r.min, vec2(r.width(), c.strip_h)), at);
            }
        }
        PanelDrop::NewGroup(before) => {
            if let Some(c) = &column {
                let y = match before.and_then(|b| c.rects.iter().find(|(x, _)| *x == b)) {
                    Some((_, r)) => r.top() - GAP / 2.0,
                    None => c.rects.last().map_or(c.area.top(), |(_, r)| r.bottom() + GAP / 2.0),
                };
                fg.line_segment([pos2(c.area.left(), y), pos2(c.area.right(), y)], Stroke::new(3.0, t.accent));
            }
        }
        // Into the icon rail: a line where its icon will go.
        PanelDrop::Rail(i) => {
            if let Some(r) = ctx.data(|x| x.get_temp::<RailFrame>(rail_frame_id())).filter(|r| r.frame == frame) {
                fg.rect_stroke(r.rect, t.radius_sm, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
                let mut seen: Vec<Group> = Vec::new();
                let mut y = None;
                for (g, ir) in &r.icons {
                    if !seen.contains(g) {
                        if seen.len() == i {
                            y = Some(ir.top() - 2.0);
                        }
                        seen.push(*g);
                    }
                }
                let y = y.unwrap_or_else(|| r.icons.last().map_or(r.rect.top() + 4.0, |(_, ir)| ir.bottom() + 2.0));
                fg.line_segment([pos2(r.rect.left() + 3.0, y), pos2(r.rect.right() - 3.0, y)], Stroke::new(3.0, t.accent));
            }
        }
        // Torn out: the window it will make.
        PanelDrop::Float(at) => {
            let wide = panel.is_some_and(|p| p.min_width() > 0.0);
            let (w, h) = if wide { (562.0, 662.0) } else { (300.0, 380.0) };
            let r = Rect::from_min_size(pos2(at[0] - 24.0, at[1] - 12.0), vec2(w, h));
            fg.rect_filled(r, t.radius_lg, t.accent.gamma_multiply(0.08));
            fg.rect_stroke(r, t.radius_lg, Stroke::new(1.5, t.accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
        }
    }
    let Some(panel) = panel else { return };
    let label = ctx.fonts_mut(|f| f.layout_no_wrap(tl!(panel.name()).to_owned(), crate::theme::medium(12.5), t.text));
    let chip = Rect::from_min_size(p + vec2(14.0, 10.0), label.size() + vec2(16.0, 8.0));
    fg.rect_filled(chip, t.radius_sm, t.card);
    fg.rect_stroke(chip, t.radius_sm, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    fg.galley(chip.min + vec2(8.0, 4.0), label, t.text);
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
        // A panel dragged by its tab is dropped in `end_drag`, wherever it is released.
        if !locked
            && !this_panel_drag
            && resp.strip.drag_stopped()
            && let Some(p) = ui.ctx().pointer_interact_pos()
        {
            actions.push(Action::Move(g, drop_before(&order, &rects, g, p.y)));
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
    let frame = ui.ctx().cumulative_frame_nr();
    ui.ctx().data_mut(|d| {
        d.insert_temp(column_id(), DockColumn { frame, rects: rects.clone(), strips: strips.clone(), strip_h: strip, area });
        d.insert_temp(rects_id(), rects);
        d.insert_temp(strips_id(), strips);
        d.insert_temp::<Option<PanelDrag>>(panel_drag_id(), panel_drag);
    });
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
        let give = (oh - layout.min_height(o)).max(0.0).min(left);
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
        layout_replaced(app);
    }
    app.ui.timeline.open = v.get("timelineOpen").and_then(Value::as_bool).unwrap_or(false);
    if !app.ui.timeline.open {
        app.ui.timeline.playing = false;
    }
}

#[cfg(test)]
#[path = "dock_tests.rs"]
mod tests;
