//! Effect → Effect Gallery: browse Photoshop-style raster filters by category, build a stack, and
//! apply it as separate appearance effects in one undo step.

use egui::{Color32, Rect, Sense, Stroke};
use serde_json::{Value, json};
use vectorcraft_effects::{EffectInfo, PIXEL_EFFECTS};

use super::DialogSpec;
use crate::state::Dialog;
use crate::theme::Tokens;
use crate::{VectorcraftApp, widgets};

/// Dialog kind for the Effect Gallery.
pub const KIND: &str = "effectGallery";

/// Raster effect submenus shown in the gallery (same paths as the Effect menu).
const CATEGORIES: [(&str, &[&str]); 10] = [
    ("Artistic", &["Effect", "Artistic"]),
    ("Blur", &["Effect", "Blur"]),
    ("Brush Strokes", &["Effect", "Brush Strokes"]),
    ("Distort", &["Effect", "Distort"]),
    ("Pixelate", &["Effect", "Pixelate"]),
    ("Sharpen", &["Effect", "Sharpen"]),
    ("Sketch", &["Effect", "Sketch"]),
    ("Stylize", &["Effect", "Stylize"]),
    ("Texture", &["Effect", "Texture"]),
    ("Video", &["Effect", "Video"]),
];

pub(super) const SPEC: DialogSpec = DialogSpec {
    heading: |_| tl!("Effect Gallery").into(),
    body,
    confirm,
    min_width: 720.0,
    max_width: Some(820.0),
    ..DialogSpec::FORM
};

#[derive(Clone)]
struct StackEntry {
    effect: String,
    visible: bool,
}

/// Open the gallery on the current selection.
pub fn open(app: &mut VectorcraftApp, _p: &Value) -> Result<Value, String> {
    if !app.session.active().is_some_and(|s| !s.selection.is_empty()) {
        return Err(tl!("Select artwork to apply effects").into());
    }
    let selected = effects_in_category(0).first().map(|e| e.id).unwrap_or("");
    app.ui.dialog = Some(Dialog::new(KIND, json!({ "category": 0, "selected": selected, "stack": [], "stackSelected": 0 })));
    Ok(Value::Null)
}

fn effects_in_category(cat: usize) -> Vec<EffectInfo> {
    let path = CATEGORIES.get(cat).map(|(_, p)| *p).unwrap_or(CATEGORIES[0].1);
    let mut out: Vec<EffectInfo> = vectorcraft_effects::effect_catalog()
        .into_iter()
        .filter(|e| e.menu == path && PIXEL_EFFECTS.contains(&e.id))
        .collect();
    out.sort_by(|a, b| a.label.cmp(b.label));
    out
}

fn stack_of(d: &Dialog) -> Vec<StackEntry> {
    d.fields
        .get("stack")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let effect = row.get("effect").and_then(Value::as_str)?;
                    let visible = row.get("visible").and_then(Value::as_bool).unwrap_or(true);
                    Some(StackEntry { effect: effect.to_string(), visible })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_stack(d: &mut Dialog, stack: &[StackEntry]) {
    let rows: Vec<Value> = stack.iter().map(|e| json!({ "effect": e.effect, "visible": e.visible })).collect();
    d.fields.insert("stack".into(), Value::Array(rows));
}

fn category_ix(d: &Dialog) -> usize {
    d.fields.get("category").and_then(Value::as_u64).unwrap_or(0) as usize
}

fn selected_id(d: &Dialog) -> String {
    d.str("selected")
}

fn set_selected(d: &mut Dialog, id: &str) {
    d.fields.insert("selected".into(), json!(id));
}

/// A small procedural thumbnail from the effect id (no copied artwork).
fn thumb(ui: &mut egui::Ui, id: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let hash = id.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let c1 = Color32::from_rgb((hash & 0xFF) as u8 | 40, ((hash >> 8) & 0xFF) as u8 | 40, ((hash >> 16) & 0xFF) as u8 | 40);
    let c2 = Color32::from_rgb(((hash >> 4) & 0xFF) as u8 | 20, ((hash >> 12) & 0xFF) as u8 | 20, ((hash >> 20) & 0xFF) as u8 | 20);
    let painter = ui.painter();
    let steps = 6;
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let t1 = (i + 1) as f32 / steps as f32;
        let mix = |t: f32| {
            let u = t;
            Color32::from_rgba_unmultiplied(
                (c1.r() as f32 * (1.0 - u) + c2.r() as f32 * u) as u8,
                (c1.g() as f32 * (1.0 - u) + c2.g() as f32 * u) as u8,
                (c1.b() as f32 * (1.0 - u) + c2.b() as f32 * u) as u8,
                255,
            )
        };
        let band = Rect::from_min_max(
            egui::pos2(rect.left() + rect.width() * t0, rect.top()),
            egui::pos2(rect.left() + rect.width() * t1, rect.bottom()),
        );
        painter.rect_filled(band, 0.0, mix((t0 + t1) * 0.5));
    }
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Tokens::get(ui.ctx()).input_border), egui::StrokeKind::Inside);
}

fn effect_label(id: &str) -> String {
    vectorcraft_effects::effect_info(id).map(|e| tl!(e.label).trim_end_matches('…').to_string()).unwrap_or_else(|| id.to_string())
}

fn body(_app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) -> bool {
    let t = Tokens::get(ui.ctx());
    let cat = category_ix(d);
    let effects = effects_in_category(cat);
    let mut sel = selected_id(d);
    if !effects.iter().any(|e| e.id == sel) {
        sel = effects.first().map(|e| e.id.to_string()).unwrap_or_default();
        set_selected(d, &sel);
    }

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            widgets::subheader(ui, tl!("Categories"));
            ui.set_width(120.0);
            for (i, (name, _)) in CATEGORIES.iter().enumerate() {
                let count = effects_in_category(i).len();
                if count == 0 {
                    continue;
                }
                let label = tl!(name);
                if ui.add(egui::Button::selectable(cat == i, label)).clicked() {
                    d.fields.insert("category".into(), json!(i));
                    let first = effects_in_category(i).first().map(|e| e.id).unwrap_or("");
                    set_selected(d, first);
                }
            }
        });
        ui.add_space(8.0);
        ui.vertical(|ui| {
            widgets::subheader(ui, tl!("Filters"));
            ui.set_width(220.0);
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for e in &effects {
                    let picked = e.id == sel;
                    ui.horizontal(|ui| {
                        thumb(ui, e.id, 28.0);
                        let name = tl!(e.label).trim_end_matches('…').to_string();
                        let resp = ui.add(egui::Button::selectable(picked, name));
                        if resp.clicked() {
                            set_selected(d, e.id);
                        }
                        if resp.double_clicked() {
                            set_selected(d, e.id);
                            add_selected(d, e.id);
                        }
                    });
                }
            });
            if widgets::flat_button(ui, tl!("Add to Stack"), 120.0).clicked() && !sel.is_empty() {
                add_selected(d, &sel);
            }
        });
        ui.add_space(8.0);
        ui.vertical(|ui| {
            widgets::subheader(ui, tl!("Preview"));
            ui.set_width(200.0);
            if !sel.is_empty() {
                thumb(ui, &sel, 160.0);
                ui.label(egui::RichText::new(effect_label(&sel)).color(t.text_strong));
            } else {
                ui.label(egui::RichText::new(tl!("Choose a filter")).color(t.text_dim));
            }
        });
    });

    ui.add_space(10.0);
    widgets::subheader(ui, tl!("Applied Filters"));
    let mut stack = stack_of(d);
    let mut stack_sel = d.fields.get("stackSelected").and_then(Value::as_u64).unwrap_or(0) as usize;
    if stack_sel >= stack.len() && !stack.is_empty() {
        stack_sel = stack.len() - 1;
    }
    enum StackOp {
        Toggle(usize),
        Up(usize),
        Down(usize),
        Remove(usize),
        Select(usize),
    }
    let mut op = None;
    let stack_len = stack.len();
    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
        if stack.is_empty() {
            ui.label(egui::RichText::new(tl!("Add filters from the list above")).color(t.text_dim));
            return;
        }
        for (i, entry) in stack.iter().enumerate().take(stack_len) {
            ui.horizontal(|ui| {
                let eye = if entry.visible { "eye" } else { "eye-off" };
                if widgets::icon_button(ui, eye, tl!("Click to toggle visibility"), false, 22.0).clicked() {
                    op = Some(StackOp::Toggle(i));
                }
                let name = effect_label(&entry.effect);
                if ui.add(egui::Button::selectable(stack_sel == i, name)).clicked() {
                    op = Some(StackOp::Select(i));
                }
                if widgets::icon_button_enabled(ui, "dc-arrow-up", tl!("Move Up"), false, i > 0, 22.0).clicked() {
                    op = Some(StackOp::Up(i));
                }
                if widgets::icon_button_enabled(ui, "dc-arrow-down", tl!("Move Down"), false, i + 1 < stack_len, 22.0).clicked() {
                    op = Some(StackOp::Down(i));
                }
                if widgets::flat_button(ui, tl!("Remove"), 64.0).clicked() {
                    op = Some(StackOp::Remove(i));
                }
            });
        }
    });
    match op {
        Some(StackOp::Toggle(i)) if stack.get(i).is_some() => stack[i].visible = !stack[i].visible,
        Some(StackOp::Select(i)) => stack_sel = i,
        Some(StackOp::Up(i)) if i > 0 => {
            stack.swap(i, i - 1);
            stack_sel = i - 1;
        }
        Some(StackOp::Down(i)) if i + 1 < stack.len() => {
            stack.swap(i, i + 1);
            stack_sel = i + 1;
        }
        Some(StackOp::Remove(i)) if i < stack.len() => {
            stack.remove(i);
            if stack_sel >= stack.len() && !stack.is_empty() {
                stack_sel = stack.len() - 1;
            }
        }
        _ => {}
    }
    write_stack(d, &stack);
    d.fields.insert("stackSelected".into(), json!(stack_sel));
    false
}

fn add_selected(d: &mut Dialog, id: &str) {
    let mut stack = stack_of(d);
    stack.push(StackEntry { effect: id.to_string(), visible: true });
    write_stack(d, &stack);
    d.fields.insert("stackSelected".into(), json!(stack.len().saturating_sub(1)));
}

fn confirm(app: &mut VectorcraftApp, d: &Dialog) -> Result<Value, String> {
    let stack = stack_of(d);
    if stack.is_empty() {
        app.ui.dialog = None;
        return Ok(Value::Null);
    }
    let mut commands = Vec::with_capacity(stack.len() * 2);
    for (i, entry) in stack.iter().enumerate() {
        let params = vectorcraft_effects::effect_info(&entry.effect).map(|e| e.defaults.clone()).unwrap_or(json!({}));
        commands.push(json!({ "command": "effect.apply", "params": { "effect": entry.effect, "params": params } }));
        if !entry.visible {
            commands.push(json!({ "command": "effect.setParams", "params": { "index": i, "visible": false } }));
        }
    }
    let r = app.run("command.batch", json!({ "label": "Effect Gallery", "commands": commands }));
    if r.is_ok() {
        app.ui.dialog = None;
    }
    r
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vectorcraft_engine::Session;

    use crate::VectorcraftApp;

    fn frame(app: &mut VectorcraftApp) {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::apply(&ctx, Default::default());
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| crate::dialogs::show(app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn stack_ok_is_one_undo_step() {
        let mut app = VectorcraftApp::new(Session::new(), Default::default());
        app.run("file.new", json!({"width": 200, "height": 200})).unwrap();
        let id = app.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 40, "height": 40})).unwrap()["id"].as_u64().unwrap();
        app.run("ui.effectGalleryDialog", json!({})).unwrap();
        assert_eq!(app.ui.dialog.as_ref().map(|d| d.kind.as_str()), Some(super::KIND));
        let d = app.ui.dialog.as_mut().unwrap();
        d.fields.insert(
            "stack".into(),
            json!([{ "effect": "blur.gaussian", "visible": true }, { "effect": "stylize.glowingEdges", "visible": false }]),
        );
        frame(&mut app);
        let steps = app.session.active().unwrap().history.undo.len();
        crate::dialogs::confirm(&mut app).unwrap();
        let st = app.session.active().unwrap();
        let n = st.doc.node(vectorcraft_doc::NodeId(id)).unwrap();
        assert_eq!(n.appearance.effects.len(), 2);
        assert_eq!(n.appearance.effects[0].id, "blur.gaussian");
        assert_eq!(n.appearance.effects[1].id, "stylize.glowingEdges");
        assert!(!n.appearance.effects[1].visible);
        assert_eq!(st.history.undo.len(), steps + 1);
    }
}
