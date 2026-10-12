//! Brush auto-tagging for the Brush HUD: Tier 1 category, Tier 2 family and a one-line
//! description, from a preset's name (and, as a fallback, its folders and group).
//!
//! [`categorize_brush`] is the routing rule ArtForge specifies, ported verbatim: the first
//! category (in table order) whose keyword the lowercased name contains wins, else `Custom`.
//! Substring matching is intended: "Paper Grain" contains "rain" and lands in Atmosphere.

/// The categories and their keywords, in the order [`categorize_brush`] tries them.
pub const CATEGORIES: [(&str, &[&str]); 5] = [
    ("Sketch", &["pencil", "charcoal", "chalk", "ink", "line", "pen", "sketch", "graphite", "pastel", "conte"]),
    ("Paint", &["paint", "oil", "acrylic", "watercolor", "gouache", "blend", "smudge", "wet", "fluid", "wash", "round", "flat", "bristle"]),
    ("Flora", &["tree", "leaf", "fern", "grass", "bush", "shrub", "branch", "twig", "flora", "plant", "vine", "flower", "petal", "weed", "moss"]),
    ("Atmosphere", &["cloud", "fog", "mist", "smoke", "haze", "dust", "rain", "snow", "storm", "spark", "light", "beam", "particle"]),
    ("Surface", &["rock", "stone", "sand", "dirt", "crack", "grunge", "grit", "fabric", "weave", "bark", "brick", "concrete", "noise", "splatter", "texture"]),
];

/// The fallback bucket for names no keyword matches.
pub const CUSTOM: &str = "Custom";

/// Tier 1 hubs around the inner ring, clockwise from 12 o'clock.
pub const MACRO_ORDER: [&str; 5] = ["Sketch", "Paint", "Surface", "Flora", "Atmosphere"];

/// The category a brush name routes to (`Custom` when nothing matches).
pub fn categorize_brush(brush_name: &str) -> &'static str {
    let name = brush_name.to_lowercase();
    for (category, keywords) in CATEGORIES {
        for keyword in keywords {
            if name.contains(keyword) {
                return category;
            }
        }
    }
    CUSTOM
}

fn keywords(category: &str) -> &'static [&'static str] {
    CATEGORIES.iter().find(|(c, _)| *c == category).map_or(&[], |(_, k)| k)
}

/// The keyword of `category` naming the brush's head noun. Brush names put the noun last
/// ("Boston Fern", "Leafy Branch"), so the match furthest right wins; a longer keyword wins a
/// tie ("pencil" over "pen").
pub fn head_keyword(name: &str, category: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    let mut best: Option<(&'static str, usize)> = None;
    for k in keywords(category) {
        if let Some(at) = lower.rfind(k) {
            let better = match best {
                None => true,
                Some((b, bat)) => at > bat || (at == bat && k.len() > b.len()),
            };
            if better {
                best = Some((k, at));
            }
        }
    }
    best.map(|(k, _)| k)
}

/// Upper-cases the first letter of each word.
pub fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A brush's place in the HUD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub category: &'static str,
    pub family: String,
    pub keyword: Option<&'static str>,
}

/// Category and family of a preset.
///
/// - The category is [`categorize_brush`] of the name. Only when that is `Custom` are the
///   preset's folders, then its group, tried the same way (an imported "Ferns" folder sends
///   "Fiddlehead" to Flora).
/// - The family is the innermost folder when it says more than the category itself; otherwise
///   the head keyword ("Fern"); otherwise, for Custom brushes, the group or the name's first word.
pub fn tag(name: &str, folder: &[String], group: &str) -> Tag {
    let mut category = categorize_brush(name);
    let path = folder.join(" ");
    let mut context = None;
    if category == CUSTOM {
        for candidate in [path.as_str(), group] {
            let c = if candidate.trim().is_empty() { CUSTOM } else { categorize_brush(candidate) };
            if c != CUSTOM {
                category = c;
                context = Some(candidate.to_string());
                break;
            }
        }
    }
    let inner = folder.iter().rev().find(|f| !f.trim().is_empty()).map(|f| f.trim().to_string());
    let inner = inner.filter(|f| !f.eq_ignore_ascii_case(category));
    let mut keyword = if category == CUSTOM { None } else { head_keyword(name, category) };
    if keyword.is_none()
        && let Some(c) = &context
    {
        keyword = head_keyword(c, category);
    }
    let family = if let Some(f) = inner {
        title_case(&f)
    } else if let Some(k) = keyword {
        title_case(k)
    } else if category == CUSTOM && !group.trim().is_empty() {
        title_case(group.trim())
    } else {
        title_case(name.split_whitespace().next().unwrap_or(CUSTOM))
    };
    Tag { category, family, keyword }
}

/// One line for the preview panel: what a brush of this kind is for.
pub fn describe(tag: &Tag) -> String {
    let line = tag.keyword.and_then(|k| DESCRIPTIONS.iter().find(|(w, _)| *w == k)).map(|(_, d)| *d);
    match line {
        Some(d) => d.to_string(),
        None if tag.category == CUSTOM => "Custom brush, not matched to a category by name.".to_string(),
        None => format!("{} brush in the {} family.", tag.category, tag.family),
    }
}

const DESCRIPTIONS: &[(&str, &str)] = &[
    ("pencil", "Fine, textured pencil with natural grain and pressure sensitivity."),
    ("charcoal", "Soft, crumbly charcoal that breaks up on the paper tooth."),
    ("chalk", "Dry chalk with a broken edge for quick value blocking."),
    ("ink", "Crisp ink line that tapers with pressure."),
    ("line", "Clean line work with a smooth pressure taper."),
    ("pen", "Technical pen with a steady, hard edge."),
    ("sketch", "Loose sketching brush for gesture and construction lines."),
    ("graphite", "Graphite with a cool sheen and fine grain."),
    ("pastel", "Waxy pastel stick with a textured drag."),
    ("conte", "Conté crayon: dense, square-edged and grainy."),
    ("paint", "General painting brush with smooth coverage."),
    ("oil", "Loaded oil brush with visible bristle streaks."),
    ("acrylic", "Opaque acrylic with a fast, flat finish."),
    ("watercolor", "Transparent watercolor with soft wet edges."),
    ("gouache", "Matte, opaque gouache for flat shapes."),
    ("blend", "Soft blender that pulls colors together."),
    ("smudge", "Smudging tip that drags and softens pigment."),
    ("wet", "Wet brush with pooling edges."),
    ("fluid", "Fluid paint with long, even strokes."),
    ("wash", "Broad wash for laying in large areas."),
    ("round", "Round brush with pressure-driven size."),
    ("flat", "Flat brush for planes and hard edges."),
    ("bristle", "Bristle brush that leaves individual hair marks."),
    ("tree", "Stamps tree forms and canopy clusters."),
    ("leaf", "Scattered leaves with size and angle jitter."),
    ("fern", "Fern fronds and leaflets for natural foliage."),
    ("grass", "Grass blades that follow the stroke direction."),
    ("bush", "Dense bush clusters for mid-ground foliage."),
    ("shrub", "Shrub masses with broken silhouettes."),
    ("branch", "Branch structures with forking twigs."),
    ("twig", "Thin twigs and fine woody detail."),
    ("flora", "General flora scatter brush."),
    ("plant", "Plant forms with organic variation."),
    ("vine", "Climbing vine with curling tendrils."),
    ("flower", "Flower heads scattered along the stroke."),
    ("petal", "Loose petals with color variation."),
    ("weed", "Weeds and wild growth for ground cover."),
    ("moss", "Moss and lichen texture for stone and bark."),
    ("cloud", "Billowing cloud puffs with soft edges."),
    ("fog", "Low, soft fog for depth."),
    ("mist", "Thin mist for atmospheric perspective."),
    ("smoke", "Wispy smoke trails."),
    ("haze", "Even haze to push back distant planes."),
    ("dust", "Fine dust particles."),
    ("rain", "Streaking rain lines."),
    ("snow", "Snowflakes with scatter and size jitter."),
    ("storm", "Heavy storm cloud masses."),
    ("spark", "Bright sparks and embers."),
    ("light", "Glowing light for highlights and bloom."),
    ("beam", "Directional light beams."),
    ("particle", "Scattered particles with count jitter."),
    ("rock", "Rock faces with hard, faceted edges."),
    ("stone", "Stone texture with chips and pits."),
    ("sand", "Fine sand grain."),
    ("dirt", "Dirt and grime for weathering."),
    ("crack", "Cracks and fissures."),
    ("grunge", "Grunge texture for wear and tear."),
    ("grit", "Gritty surface noise."),
    ("fabric", "Fabric weave texture."),
    ("weave", "Woven canvas pattern."),
    ("bark", "Tree bark ridges."),
    ("brick", "Brick and mortar texture."),
    ("concrete", "Concrete with pores and stains."),
    ("noise", "Fine noise for breaking up flat areas."),
    ("splatter", "Paint splatter and droplets."),
    ("texture", "General surface texture brush."),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn routes_by_the_first_matching_keyword_in_category_order() {
        assert_eq!(categorize_brush("Pencil"), "Sketch");
        assert_eq!(categorize_brush("HARD ROUND"), "Paint");
        assert_eq!(categorize_brush("Boston Fern"), "Flora");
        assert_eq!(categorize_brush("Cumulus Cloud"), "Atmosphere");
        assert_eq!(categorize_brush("Rock Face"), "Surface");
        assert_eq!(categorize_brush("Signature Stamp"), "Custom");
        // Sketch is tried before Paint, and matching is by substring, as specified.
        assert_eq!(categorize_brush("Oil Pastel"), "Sketch");
        assert_eq!(categorize_brush("Paper Grain"), "Atmosphere");
        assert_eq!(categorize_brush(""), "Custom");
    }

    #[test]
    fn every_keyword_routes_to_its_own_category_unless_an_earlier_one_contains_it() {
        for (i, (cat, words)) in CATEGORIES.iter().enumerate() {
            for w in *words {
                let earlier = CATEGORIES[..i].iter().any(|(_, ws)| ws.iter().any(|e| w.contains(e)));
                if !earlier {
                    assert_eq!(categorize_brush(w), *cat, "{w}");
                }
            }
        }
        let mut macro_sorted = MACRO_ORDER.to_vec();
        macro_sorted.sort_unstable();
        let mut cats: Vec<&str> = CATEGORIES.iter().map(|(c, _)| *c).collect();
        cats.sort_unstable();
        assert_eq!(macro_sorted, cats);
    }

    #[test]
    fn families_follow_the_head_noun_then_folders() {
        assert_eq!(tag("Boston Fern", &[], ""), Tag { category: "Flora", family: "Fern".into(), keyword: Some("fern") });
        assert_eq!(tag("Tree Fern", &[], "").family, "Fern");
        assert_eq!(tag("Leafy Branch", &[], "").family, "Branch");
        assert_eq!(tag("Mechanical Pencil", &[], "").family, "Pencil");
        assert_eq!(tag("Hard Round Pressure Size", &[], "General").family, "Round");
        // Folder fallback for the category, and folders naming the family.
        let t = tag("Fiddlehead", &s(&["Ferns"]), "Kyle Nature");
        assert_eq!((t.category, t.family.as_str()), ("Flora", "Ferns"));
        assert_eq!(tag("Grass Clump Dry", &s(&["Grass Clump"]), "").family, "Grass Clump");
        // A folder that only repeats the category doesn't become a family.
        assert_eq!(tag("Fern Frond", &s(&["Flora"]), "").family, "Fern");
        // Group fallback, then Custom families.
        assert_eq!(tag("Fiddlehead", &[], "Ferns").category, "Flora");
        assert_eq!(tag("Airbrush Soft", &[], "General"), Tag { category: "Custom", family: "General".into(), keyword: None });
        assert_eq!(tag("Halftone Dots", &[], "").family, "Halftone");
    }

    #[test]
    fn descriptions_never_come_back_empty() {
        assert!(describe(&tag("Soft Pencil", &[], "")).starts_with("Fine, textured pencil"));
        assert!(describe(&tag("Odd", &[], "")).contains("Custom"));
        for (_, words) in CATEGORIES {
            for w in words {
                assert!(DESCRIPTIONS.iter().any(|(k, _)| k == w), "{w} has no description");
            }
        }
    }
}
