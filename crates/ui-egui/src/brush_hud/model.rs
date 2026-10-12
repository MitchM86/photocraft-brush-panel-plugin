//! The Brush HUD's view of the preset library: every preset tagged into a Tier 1 category and a
//! Tier 2 family, ordered and filtered by the user's HUD layout, plus search.
//!
//! Removing a node in Edit HUD only hides it here ([`HudLayout::hidden`]); the preset itself is
//! never touched. Brushes are keyed by preset name (unique in a session), families by
//! `fam:<Category>/<Family>`, hubs by category name.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use photocraft_engine::paint::BrushPreset;
use serde::{Deserialize, Serialize};

use super::categorize::{self, CUSTOM, MACRO_ORDER, Tag};

/// One preset in the HUD.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Index into the session's preset list when the library was built.
    pub index: usize,
    pub name: String,
    pub category: &'static str,
    pub family: String,
    pub family_id: String,
    pub description: String,
}

/// A Tier 2 family and its visible variations (entry indices), in HUD order.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub id: String,
    pub name: String,
    pub category: &'static str,
    pub members: Vec<usize>,
}

/// The user's HUD layout: custom order per scope (`cat`, `fam:<Category>`, `var:<family id>`) and
/// the nodes removed from the HUD. Saved in the preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudLayout {
    pub order: BTreeMap<String, Vec<String>>,
    pub hidden: BTreeSet<String>,
}

/// Most entries kept per order scope or in the hidden set (a corrupt preferences file can't grow
/// them without bound).
pub const MAX_LAYOUT_ITEMS: usize = 20_000;

impl HudLayout {
    /// Drops what a damaged preferences file could hold beyond the caps.
    pub fn sanitized(mut self) -> Self {
        if self.order.len() > MAX_LAYOUT_ITEMS {
            self.order = self.order.into_iter().take(MAX_LAYOUT_ITEMS).collect();
        }
        for v in self.order.values_mut() {
            v.truncate(MAX_LAYOUT_ITEMS);
        }
        if self.hidden.len() > MAX_LAYOUT_ITEMS {
            self.hidden = self.hidden.into_iter().take(MAX_LAYOUT_ITEMS).collect();
        }
        self
    }
}

pub fn family_id(category: &str, family: &str) -> String {
    format!("fam:{category}/{family}")
}

/// The scope key of a family's variations.
pub fn var_scope(family_id: &str) -> String {
    format!("var:{family_id}")
}

/// The scope key of a category's families.
pub fn fam_scope(category: &str) -> String {
    format!("fam:{category}")
}

/// The tagged library, rebuilt only when the preset list changes ([`Library::signature`]).
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub entries: Vec<Entry>,
    pub signature: u64,
}

/// A cheap hash of what tagging reads: names, groups, folders and order.
pub fn signature(presets: &[BrushPreset]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    presets.len().hash(&mut h);
    for p in presets {
        (&p.name, &p.group, &p.folder).hash(&mut h);
    }
    h.finish()
}

impl Library {
    pub fn build(presets: &[BrushPreset]) -> Self {
        let entries = presets
            .iter()
            .enumerate()
            .map(|(index, p)| {
                let t: Tag = categorize::tag(&p.name, &p.folder, &p.group);
                Entry {
                    index,
                    name: p.name.clone(),
                    category: t.category,
                    family_id: family_id(t.category, &t.family),
                    description: categorize::describe(&t),
                    family: t.family,
                }
            })
            .collect();
        Library { entries, signature: signature(presets) }
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    fn visible(&self, e: &Entry, layout: &HudLayout) -> bool {
        !layout.hidden.contains(&e.name) && !layout.hidden.contains(&e.family_id)
    }

    /// Tier 1: the five macro hubs (always), plus Custom when it has visible brushes; each with
    /// its visible brush count, in the user's order.
    pub fn categories(&self, layout: &HudLayout) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for e in self.entries.iter().filter(|e| self.visible(e, layout)) {
            *counts.entry(e.category).or_default() += 1;
        }
        let mut names: Vec<&str> = MACRO_ORDER.to_vec();
        if counts.get(CUSTOM).copied().unwrap_or(0) > 0 {
            names.push(CUSTOM);
        }
        let cats: Vec<(String, usize)> = names.into_iter().map(|n| (n.to_string(), counts.get(n).copied().unwrap_or(0))).collect();
        ordered(layout, "cat", cats, |c| c.0.clone())
    }

    /// Tier 2: the visible families of `category`, each with its visible variations.
    pub fn families(&self, category: &str, layout: &HudLayout) -> Vec<Family> {
        let mut fams: Vec<Family> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.category != category || !self.visible(e, layout) {
                continue;
            }
            match fams.iter_mut().find(|f| f.id == e.family_id) {
                Some(f) => f.members.push(i),
                None => fams.push(Family { id: e.family_id.clone(), name: e.family.clone(), category: e.category, members: vec![i] }),
            }
        }
        let fams: Vec<Family> = fams
            .into_iter()
            .map(|mut f| {
                let scope = var_scope(&f.id);
                f.members = ordered(layout, &scope, f.members, |&i| self.entries.get(i).map(|e| e.name.clone()).unwrap_or_default());
                f
            })
            .collect();
        ordered(layout, &fam_scope(category), fams, |f| f.id.clone())
    }

    pub fn family(&self, id: &str, layout: &HudLayout) -> Option<Family> {
        let category = id.strip_prefix("fam:")?.split('/').next()?;
        self.families(category, layout).into_iter().find(|f| f.id == id)
    }

    /// The item keys of an order scope, in current order.
    pub fn scope_keys(&self, scope: &str, layout: &HudLayout) -> Vec<String> {
        if scope == "cat" {
            return self.categories(layout).into_iter().map(|c| c.0).collect();
        }
        if let Some(fid) = scope.strip_prefix("var:") {
            return self
                .family(fid, layout)
                .map(|f| f.members.iter().filter_map(|&i| self.entries.get(i).map(|e| e.name.clone())).collect())
                .unwrap_or_default();
        }
        if let Some(cat) = scope.strip_prefix("fam:") {
            return self.families(cat, layout).into_iter().map(|f| f.id).collect();
        }
        Vec::new()
    }

    /// Visible matches for `query`, best first: every whitespace-separated token must match the
    /// name, family, category or folder path.
    pub fn search(&self, query: &str, presets: &[BrushPreset], layout: &HudLayout, limit: usize) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let tokens: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<(i64, usize)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if !self.visible(e, layout) {
                continue;
            }
            let name = e.name.to_lowercase();
            let family = e.family.to_lowercase();
            let category = e.category.to_lowercase();
            let folders = presets.get(e.index).map(|p| p.folder.join(" ").to_lowercase()).unwrap_or_default();
            let mut score: i64 = 0;
            let mut all = true;
            for t in &tokens {
                let s = if name == *t {
                    100
                } else if name.starts_with(t) {
                    80
                } else if name.split(|c: char| c.is_whitespace() || c == '-' || c == '_').any(|w| w.starts_with(t)) {
                    64
                } else if name.contains(t) {
                    48
                } else if family.starts_with(t) {
                    36
                } else if family.contains(t) || folders.contains(t) {
                    28
                } else if category.starts_with(t) {
                    18
                } else if t.chars().count() >= 3 && subsequence(t, &name) {
                    8
                } else {
                    0
                };
                if s == 0 {
                    all = false;
                    break;
                }
                score += s;
            }
            if all {
                hits.push((score * 100 - name.chars().count() as i64, i));
            }
        }
        let name_of = |i: usize| self.entries.get(i).map_or("", |e| e.name.as_str());
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| name_of(a.1).cmp(name_of(b.1))));
        hits.into_iter().take(limit).map(|(_, i)| i).collect()
    }
}

fn subsequence(needle: &str, hay: &str) -> bool {
    let mut it = needle.chars().peekable();
    for c in hay.chars() {
        if it.peek() == Some(&c) {
            it.next();
        }
    }
    it.peek().is_none()
}

/// `items` in the scope's saved order: saved keys first, in that order, then the rest as they came.
fn ordered<T>(layout: &HudLayout, scope: &str, items: Vec<T>, key: impl Fn(&T) -> String) -> Vec<T> {
    let Some(order) = layout.order.get(scope).filter(|o| !o.is_empty()) else { return items };
    let rank: BTreeMap<&str, usize> = order.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let mut keyed: Vec<(usize, usize, T)> =
        items.into_iter().enumerate().map(|(i, t)| (rank.get(key(&t).as_str()).copied().unwrap_or(usize::MAX), i, t)).collect();
    keyed.sort_by_key(|(r, i, _)| (*r, *i));
    keyed.into_iter().map(|(_, _, t)| t).collect()
}

/// Moves `id` within `scope` so it sits before `before` (last when `None`). Returns the new
/// layout, or `None` when nothing changes.
pub fn moved(lib: &Library, layout: &HudLayout, scope: &str, id: &str, before: Option<&str>) -> Option<HudLayout> {
    let current = lib.scope_keys(scope, layout);
    let from = current.iter().position(|k| k == id)?;
    if before == Some(id) {
        return None;
    }
    let mut next = current.clone();
    let item = next.remove(from);
    let to = match before {
        Some(b) => next.iter().position(|k| k == b)?,
        None => next.len(),
    };
    next.insert(to, item);
    if next == current {
        return None;
    }
    let mut out = layout.clone();
    out.order.insert(scope.to_string(), next);
    Some(out)
}

/// Where a node dropped onto `target` lands: after it when moving forward, before it when moving
/// back (the dropped node takes the target's place).
pub fn drop_before(keys: &[String], id: &str, target: &str) -> Option<Option<String>> {
    let from = keys.iter().position(|k| k == id)?;
    let to = keys.iter().position(|k| k == target)?;
    Some(if from < to { keys.get(to + 1).cloned() } else { Some(target.to_string()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_engine::BrushSettings;

    fn preset(name: &str, group: &str, folder: &[&str]) -> BrushPreset {
        BrushPreset {
            name: name.into(),
            brush: BrushSettings::default(),
            builtin: false,
            group: group.into(),
            folder: folder.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn library() -> (Vec<BrushPreset>, Library) {
        let mut v = Vec::new();
        for n in [
            "Fern Frond",
            "Fiddlehead",
            "Maidenhair",
            "Bracken",
            "Sword Fern",
            "Tree Fern",
            "Boston Fern",
            "Fern Spray",
            "Fern Scatter",
            "Fern Silhouette",
            "Fern Shadow",
        ] {
            v.push(preset(n, "Nature", &["Fern"]));
        }
        for n in ["Cumulus Cloud", "Stratus Cloud", "Storm Cloud"] {
            v.push(preset(n, "Sky", &[]));
        }
        for n in ["Hard Round", "Soft Round", "Soft Pencil", "Airbrush Soft", "Bush", "Shrub"] {
            v.push(preset(n, "General", &[]));
        }
        let lib = Library::build(&v);
        (v, lib)
    }

    #[test]
    fn tiers_group_presets() {
        let (_, lib) = library();
        let l = HudLayout::default();
        let cats: Vec<String> = lib.categories(&l).into_iter().map(|c| c.0).collect();
        assert_eq!(cats, ["Sketch", "Paint", "Surface", "Flora", "Atmosphere", "Custom"]);
        let fern = lib.family("fam:Flora/Fern", &l).expect("fern family");
        assert_eq!(fern.members.len(), 11, "folder routes Fiddlehead, Maidenhair and Bracken in");
        assert_eq!(lib.families("Atmosphere", &l)[0].members.len(), 3);
        assert_eq!(lib.families("Flora", &l).len(), 3);
    }

    #[test]
    fn hide_and_reorder_touch_only_the_layout() {
        let (p, lib) = library();
        let mut l = HudLayout::default();
        let fern = "fam:Flora/Fern";
        let names = |l: &HudLayout| lib.scope_keys(&var_scope(fern), l);
        l = moved(&lib, &l, &var_scope(fern), "Fern Shadow", Some("Fern Frond")).expect("moved");
        assert_eq!(names(&l)[0], "Fern Shadow");
        assert!(moved(&lib, &l, &var_scope(fern), "Fern Shadow", Some("Fern Frond")).is_none(), "no-op move");
        l.hidden.insert("Bracken".into());
        assert!(!names(&l).contains(&"Bracken".to_string()));
        l.hidden.insert("fam:Flora/Bush".into());
        assert!(lib.families("Flora", &l).iter().all(|f| f.id != "fam:Flora/Bush"));
        assert_eq!(p.len(), lib.entries.len(), "presets are untouched");
        let l2 = moved(&lib, &l, "cat", "Flora", Some("Sketch")).expect("hub moved");
        assert_eq!(lib.categories(&l2)[0].0, "Flora");
    }

    #[test]
    fn drop_semantics_take_the_target_place() {
        let k: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(drop_before(&k, "a", "c"), Some(Some("d".into())));
        assert_eq!(drop_before(&k, "a", "d"), Some(None));
        assert_eq!(drop_before(&k, "d", "b"), Some(Some("b".into())));
        assert_eq!(drop_before(&k, "x", "b"), None);
    }

    #[test]
    fn search_ranks_names_and_skips_hidden() {
        let (p, lib) = library();
        let mut l = HudLayout::default();
        let names = |hits: Vec<usize>| hits.into_iter().map(|i| lib.entries[i].name.clone()).collect::<Vec<_>>();
        let top = names(lib.search("fern", &p, &l, 3));
        assert_eq!(top.len(), 3);
        assert!(top.iter().all(|n| n.to_lowercase().contains("fern")), "{top:?}");
        assert_eq!(names(lib.search("cumulus", &p, &l, 5)), ["Cumulus Cloud"]);
        assert_eq!(names(lib.search("fiddle", &p, &l, 5)), ["Fiddlehead"]);
        assert_eq!(lib.entries[lib.search("flora fern", &p, &l, 1)[0]].family, "Fern");
        l.hidden.insert("Cumulus Cloud".into());
        assert!(lib.search("cumulus", &p, &l, 5).is_empty());
        assert!(lib.search("   ", &p, &l, 5).is_empty());
    }

    #[test]
    fn signature_follows_names_and_folders() {
        let (mut p, lib) = library();
        assert_eq!(signature(&p), lib.signature);
        p[0].folder = vec!["Other".into()];
        assert_ne!(signature(&p), lib.signature);
    }

    #[test]
    fn damaged_layouts_are_capped() {
        let mut l = HudLayout::default();
        l.order.insert("cat".into(), (0..MAX_LAYOUT_ITEMS + 10).map(|i| i.to_string()).collect());
        assert_eq!(l.sanitized().order["cat"].len(), MAX_LAYOUT_ITEMS);
    }
}
