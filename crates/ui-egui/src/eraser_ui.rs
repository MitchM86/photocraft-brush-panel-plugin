//! UI for the Eraser group's colour-aware tools: Magic Eraser (click → `paint.magicEraser`) and
//! Background Eraser (stroke → `paint.backgroundEraser`), with their options bars.

use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::state::Tool;
use crate::theme::Tokens;
use crate::{icons, widgets};

fn report(app: &mut PhotocraftApp, r: Result<Value, String>) {
    if let Err(e) = r {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

/// Magic Eraser click at document `(x, y)`. Returns false for other tools.
pub fn click(app: &mut PhotocraftApp, tool: Tool, x: f64, y: f64) -> bool {
    if tool != Tool::MagicEraser {
        return false;
    }
    let o = &app.ui.tool_options;
    let p = json!({
        "x": x.floor(),
        "y": y.floor(),
        "tolerance": o.tolerance,
        "antiAlias": o.anti_alias,
        "contiguous": o.contiguous,
        "sampleAllLayers": o.sample_all_layers,
        "opacity": o.magic_eraser_opacity,
    });
    let r = app.run("paint.magicEraser", p);
    report(app, r);
    true
}

/// Finishes a Background Eraser stroke. Returns false for other tools.
pub fn finish_stroke(app: &mut PhotocraftApp, tool: Tool, points: &[[f64; 3]]) -> bool {
    if tool != Tool::BackgroundEraser {
        return false;
    }
    let o = &app.ui.tool_options;
    let p = json!({
        "points": points,
        "sampling": o.bg_sampling,
        "limits": o.bg_limits,
        "tolerance": o.bg_tolerance,
        "protectForegroundColor": o.bg_protect_fg,
    });
    let r = app.run("paint.backgroundEraser", p);
    report(app, r);
    true
}

/// Whether `tool` is the Eraser in Pencil mode: it erases hard, aliased pixels through
/// `paint.pencil` (`"erase": true`) and shows the Pencil's pixel-grid cursor (#2662).
pub fn pencil_mode(app: &PhotocraftApp, tool: Tool) -> bool {
    tool == Tool::Eraser && app.ui.tool_options.eraser_mode == "pencil"
}

/// The Eraser's Mode dropdown (Brush / Pencil). Returns whether it changed.
pub fn mode_dropdown(ui: &mut egui::Ui, mode: &mut String) -> bool {
    opt(ui, tl!("Mode:"));
    let modes = [("brush".to_string(), tl!("Brush")), ("pencil".to_string(), tl!("Pencil"))];
    widgets::dropdown(ui, "eraser-mode", mode, &modes, 96.0)
}

/// The Eraser's Erase to History checkbox, with Photoshop's ⌥ shortcut in its tooltip.
pub fn history_checkbox(ui: &mut egui::Ui, on: &mut bool) {
    widgets::vline(ui, 22.0);
    let tip = crate::i18n::fmt(
        tl!("Paint back the History Brush's source state instead of erasing ({alt}-drag: one stroke)"),
        &[("alt", &crate::shortcuts::pretty("Alt"))],
    );
    widgets::checkbox(ui, on, tl!("Erase to History")).on_hover_text(tip);
}

/// The tool an Eraser stroke runs as: the History Brush with Erase to History on or ⌥ held (the
/// Eraser's own brush, size and opacity paint it), else the Eraser.
pub fn stroke_tool(app: &PhotocraftApp, tool: Tool, alt: bool) -> Tool {
    if tool == Tool::Eraser && (app.ui.tool_options.erase_to_history || alt) { Tool::HistoryBrush } else { tool }
}

/// `paint.historyBrush` params beyond the brush: a Pencil-mode Eraser erasing to history paints
/// with a hard edge, as it erases.
pub fn history_params(app: &PhotocraftApp) -> Value {
    if pencil_mode(app, app.ui.tool) { json!({"hardness": 100}) } else { json!({}) }
}

fn opt(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!(&text)).color(t.text_dim).size(12.0));
}

/// Sampling (Continuous / Once / Background Swatch, as icon toggles), Limits and Tolerance, as
/// the Background Eraser and Color Replacement tools show them in Photoshop.
fn sampling_and_limits(ui: &mut egui::Ui, id: &str, sampling: &mut String, limits: &mut String, tolerance: &mut f32) {
    ui.spacing_mut().item_spacing.x = 2.0;
    for (k, icon, tip) in [
        ("continuous", "pipette", tl!("Sampling: Continuous")),
        ("once", "circle-dot", tl!("Sampling: Once")),
        ("backgroundSwatch", "square", tl!("Sampling: Background Swatch")),
    ] {
        if icons::button(ui, icon, 24.0, *sampling == k, tip).clicked() {
            *sampling = k.into();
        }
    }
    ui.spacing_mut().item_spacing.x = 8.0;
    widgets::vline(ui, 22.0);
    opt(ui, tl!("Limits:"));
    let choices = [
        ("discontiguous".to_string(), tl!("Discontiguous")),
        ("contiguous".to_string(), tl!("Contiguous")),
        ("findEdges".to_string(), tl!("Find Edges")),
    ];
    widgets::dropdown(ui, id, limits, &choices, 110.0);
    opt(ui, tl!("Tolerance:"));
    widgets::value_field(ui, tolerance, 0.0..=100.0, "%", 62.0);
}

/// Options bar for the Magic Eraser, Background Eraser and Color Replacement tools. Returns false
/// for other tools.
pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui, tool: Tool) -> bool {
    let o = &mut app.ui.tool_options;
    match tool {
        Tool::MagicEraser => {
            opt(ui, tl!("Tolerance:"));
            widgets::value_field(ui, &mut o.tolerance, 0.0..=255.0, "", 58.0);
            widgets::checkbox(ui, &mut o.anti_alias, tl!("Anti-alias"));
            widgets::checkbox(ui, &mut o.contiguous, tl!("Contiguous"));
            widgets::checkbox(ui, &mut o.sample_all_layers, tl!("Sample All Layers"));
            widgets::vline(ui, 22.0);
            opt(ui, tl!("Opacity:"));
            widgets::value_field(ui, &mut o.magic_eraser_opacity, 1.0..=100.0, "%", 62.0);
        }
        Tool::BackgroundEraser => {
            sampling_and_limits(ui, "bg-eraser-limits", &mut o.bg_sampling, &mut o.bg_limits, &mut o.bg_tolerance);
            widgets::checkbox(ui, &mut o.bg_protect_fg, tl!("Protect Foreground Color"));
        }
        // Color Replacement: Mode, then the Background Eraser's Sampling / Limits / Tolerance.
        Tool::ColorReplacement => {
            opt(ui, tl!("Mode:"));
            let modes = [
                ("hue".to_string(), tl!("Hue")),
                ("saturation".to_string(), tl!("Saturation")),
                ("color".to_string(), tl!("Color")),
                ("luminosity".to_string(), tl!("Luminosity")),
            ];
            widgets::dropdown(ui, "color-replace-mode", &mut o.replace_mode, &modes, 100.0);
            widgets::vline(ui, 22.0);
            sampling_and_limits(ui, "color-replace-limits", &mut o.replace_sampling, &mut o.replace_limits, &mut o.replace_tolerance);
            widgets::checkbox(ui, &mut o.anti_alias, tl!("Anti-alias"));
        }
        _ => return false,
    }
    true
}
