use nih_plug_egui::egui::{self, Color32, Rect, Response, Stroke, StrokeKind};
use os_theme::{design::StockEditorLook, Color, Palette};

fn color(ink: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(ink.r, ink.g, ink.b, ink.a)
}

fn surface(face: Color, look: Option<StockEditorLook>) -> Color {
    let Some(look) = look else { return face };
    let bg = look.background;
    let face = bg.tint.map_or(face, |tint| {
        let (hue, sat, _) = tint.hsl();
        let (_, _, light) = face.hsl();
        Color::from_hsl_pub(hue, sat, light, face.a)
    });
    face.mix(
        Color::rgb(0xffffff),
        f32::from(bg.brightness.min(100)) * 0.0015,
    )
}

pub fn visuals(p: &Palette) -> egui::Visuals {
    let look = p.style.stock_editor;
    let mut viz = egui::Visuals::dark();
    let structure = look.map_or(p.toolbar_edge, |l| l.inks[1]);
    let accent = look.map_or(p.pill, |l| l.inks[0]);
    let highlight = look.map_or(p.glyph, |l| l.inks[2]);
    viz.panel_fill = color(surface(p.toolbar, look));
    viz.window_fill = color(surface(p.sunken, look));
    viz.extreme_bg_color = color(surface(p.sunken_dark, look));
    viz.widgets.noninteractive.fg_stroke.color = color(p.text);
    viz.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, color(structure));
    for (widget, face, edge, text) in [
        (&mut viz.widgets.inactive, p.button, structure, p.text),
        (&mut viz.widgets.hovered, p.button_hi, highlight, p.glyph),
        (&mut viz.widgets.active, p.button_hi, accent, accent),
        (&mut viz.widgets.open, p.button_hi, accent, accent),
    ] {
        widget.bg_fill = color(surface(face, look));
        widget.weak_bg_fill = widget.bg_fill;
        widget.bg_stroke = Stroke::new(1.0_f32, color(edge));
        widget.fg_stroke = Stroke::new(1.5_f32, color(text));
        widget.expansion = 0.0;
    }
    viz.selection.bg_fill = color(surface(p.button_hi, look).mix(accent, 0.25));
    viz.selection.stroke = Stroke::new(1.5_f32, color(accent));
    viz.slider_trailing_fill = true;
    viz
}

fn edge(ui: &egui::Ui, rect: Rect, ink: Color, look: StockEditorLook) {
    let glow = f32::from(look.glow.min(100)) / 100.0;
    let spread = f32::from(look.spread.min(100)) / 100.0;
    for (distance, opacity) in [
        (1.0 + 2.0 * spread, 0.08 * glow * spread),
        (0.65, 0.06 + 0.21 * glow),
    ] {
        ui.painter().rect_stroke(
            rect.expand(distance),
            3.,
            Stroke::new(1.0 + distance, color(ink).gamma_multiply(opacity)),
            StrokeKind::Outside,
        );
    }
    ui.painter()
        .rect_stroke(rect, 2., Stroke::new(1.0_f32, color(ink)), StrokeKind::Inside);
}

pub fn button(ui: &mut egui::Ui, p: &Palette, label: &str, selected: bool) -> Response {
    let response = ui.add(egui::Button::new(label).selected(selected));
    if let Some(look) = p.style.stock_editor {
        let ink = if selected || response.is_pointer_button_down_on() {
            look.inks[0]
        } else if response.hovered() {
            look.inks[2]
        } else {
            look.inks[1]
        };
        edge(ui, response.rect, ink, look);
    }
    response
}

// Keep the glow in the bezel; the movie and its letterbox are color references.
pub fn bezel(ui: &egui::Ui, rect: Rect, p: &Palette) {
    if let Some(look) = p.style.stock_editor {
        edge(ui, rect, look.inks[1], look);
    } else {
        ui.painter().rect_stroke(
            rect,
            2.,
            Stroke::new(1.0_f32, color(p.toolbar_edge)),
            StrokeKind::Inside,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Monitor, Picture, HEIGHT, WIDTH};
    use os_theme::design::{neon_with_inks, NeonBackground};
    use std::sync::Arc;

    #[test]
    fn video_player_themes_draw_stock_controls_and_preserve_picture() {
        let normal = Palette::default();
        let inks = [
            Color::rgb(0x80f53b),
            Color::rgb(0x4baafa),
            Color::rgb(0xf98c44),
        ];
        let mut neon = neon_with_inks(inks);
        neon.style.stock_editor = Some(StockEditorLook {
            inks,
            glow: 100,
            spread: 40,
            background: NeonBackground {
                tint: Some(Color::rgb(0x386065)),
                brightness: 35,
                headers: 0,
            },
        });
        assert_eq!(visuals(&normal).panel_fill, color(normal.toolbar));
        assert_ne!(visuals(&normal).panel_fill, visuals(&neon).panel_fill);
        assert_eq!(
            visuals(&neon).widgets.inactive.bg_stroke.color,
            color(inks[1])
        );
        let ctx = egui::Context::default();
        let mut monitor = Monitor::default();
        let picture = Picture {
            pixels: Some(Arc::from(vec![0x2c619a; WIDTH * HEIGHT])),
            end: 2.,
            ..Default::default()
        };
        for palette in [&normal, &neon, &normal] {
            ctx.set_visuals(visuals(palette));
            let output = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| monitor.draw(ui, &picture, palette));
            });
            let id = monitor.texture.as_ref().unwrap().id();
            let mesh = ctx.tessellate(output.shapes, output.pixels_per_point);
            let picture_mesh = mesh
                .iter()
                .find_map(|shape| match &shape.primitive {
                    egui::epaint::Primitive::Mesh(m) if m.texture_id == id => Some(m),
                    _ => None,
                })
                .expect("movie texture drawn independently from controls");
            assert!(picture_mesh
                .vertices
                .iter()
                .all(|v| v.color == Color32::WHITE));
        }
    }
}
