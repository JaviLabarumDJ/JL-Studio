// SPDX-License-Identifier: GPL-3.0-or-later
//! Flat finish for the separately built clean full-mixer preview.
use crate::app::Ui;
use os_ui::{Color, Rect};
use std::f32::consts::PI;

const LINE: Color = Color::rgb(0x191919);
const FACE: Color = Color::rgb(0x292929);
const INK: Color = Color::rgb(0xe2e2df);
const ACCENT: Color = Color::rgb(0x2096f3);

pub(super) fn panel(ui: &mut Ui, r: Rect, base: Color, selected: bool) {
    if ui.t.style.neon_3036 {
        let t = ui.t;
        ui.c.fill_rect(r, t.mixer_strip.mix(base, 0.08));
        ui.c.stroke_rrect(r.inset(0.5), 1., 1., if selected { t.accent } else { t.button_border });
        return;
    }
    ui.c.fill_rect(r, LINE);
    ui.c.fill_rect(r.inset(1.), FACE.mix(base, 0.18));
    if selected {
        ui.c.stroke_rrect(r.inset(1.), 0., 1., ACCENT);
    }
}
pub(super) fn well(ui: &mut Ui, r: Rect) {
    if ui.t.style.neon_3036 {
        ui.c.fill_rect(r, ui.t.mixer_strip_dark);
        return;
    }
    ui.c.fill_rect(r, FACE);
}
pub(super) fn plate(ui: &mut Ui, r: Rect) {
    if ui.t.style.neon_3036 {
        ui.c.fill_rect(r, ui.t.mixer_panel);
        return;
    }
    ui.c.fill_rect(r, LINE);
    ui.c.fill_rect(r.inset(1.), Color::rgb(0x303030));
}
pub(super) fn separator(ui: &mut Ui, r: Rect) {
    if ui.t.style.neon_3036 {
        ui.c.fill_rect(r, ui.t.grid_bar);
        return;
    }
    ui.c.fill_rect(r, LINE);
}
pub(super) fn lamp(ui: &mut Ui, x: f32, y: f32, on: bool, col: Color, r: f32) {
    if ui.t.style.neon_3036 {
        let ink = if on { ui.t.meter_low.mix(col, 0.20) } else { ui.t.grid_bar };
        if on { ui.c.fill_circle(x,y,r+1.8,ink.with_alpha(ui.c.neon_glow_alpha(46))); }
        ui.c.fill_circle(x,y,r,ink);
        return;
    }
    ui.c.fill_circle(x, y, r, if on { col } else { Color::rgb(0x555555) });
}
pub(super) fn knob(ui: &mut Ui, x: f32, y: f32, r: f32, value: f32, _cap: Color, hover: bool) {
    let value = value.clamp(0., 1.);
    let start = -1.25 * PI;
    let sweep = 1.5 * PI * value;
    let a = start + sweep;
    ui.c.fill_circle(x, y, r, Color::rgb(if hover { 0x555555 } else { 0x424242 }));
    ui.c.stroke_circle(x, y, r, 0.7, Color::rgb(0x5a5a5a));
    ui.c.line(
        x + r * 0.51 * a.cos(),
        y + r * 0.51 * a.sin(),
        x + r * 0.94 * a.cos(),
        y + r * 0.94 * a.sin(),
        1.35,
        INK,
    );
}
pub(super) fn fader(ui: &mut Ui, x: f32, y: f32, _selected: bool, hover: bool) {
    if ui.t.style.neon_3036 {
        os_ui::kit::neon_fader(ui.c, ui.t, Rect::new(x-8.,y-13.,16.,26.), _selected, hover);
        return;
    }
    let cap = Rect::new(x - 8., y - 13., 16., 26.);
    ui.c.fill_rrect(cap, 2., LINE);
    ui.c.fill_rrect(
        cap.inset(1.),
        1.5,
        Color::rgb(if hover { 0xc3c3c3 } else { 0xa3a3a3 }),
    );
    ui.c.line(cap.x + 1., y, cap.right() - 1., y, 1., FACE);
}
pub(super) fn button(ui: &mut Ui, r: Rect, on: bool, hover: bool, col: Color) -> Color {
    if ui.t.style.neon_3036 {
        let rim = if on { ui.t.accent } else if hover { ui.t.note_top } else { ui.t.grid_bar };
        ui.c.fill_rrect(r,2.,ui.t.mixer_strip.mix(rim, if on { 0.28 } else { 0.07 }));
        ui.c.stroke_rrect(r.inset(0.5),2.,1.,rim);
        return if on { ui.t.accent } else { ui.t.text };
    }
    let face = if on {
        col
    } else if hover {
        Color::rgb(0x505050)
    } else {
        Color::rgb(0x373737)
    };
    ui.c.fill_rrect(r, 2., LINE);
    ui.c.fill_rrect(r.inset(1.), 1., face);
    if on {
        LINE
    } else {
        INK
    }
}
pub(super) fn signal_switch(ui: &mut Ui, r: Rect, on: bool, hover: bool, swap: bool) {
    let ink = button(ui, r, on, hover, ACCENT);
    let (x, y) = (r.cx(), r.cy());
    if swap {
        for s in [-1., 1.] {
            ui.c.line(x + s, y - 2.6, x + 4. * s, y, 1., ink);
            ui.c.line(x + 4. * s, y, x + s, y + 2.6, 1., ink);
        }
    } else {
        ui.c.stroke_circle(x, y, 2.7, 1., ink);
        ui.c.line(x - 3.2, y + 3.2, x + 3.2, y - 3.2, 1., ink);
    }
}
pub(super) fn rocker(ui: &mut Ui, r: Rect, on: bool, hover: bool) {
    button(ui, r, false, hover, ACCENT);
    let half = Rect::new(
        r.x + 2.,
        if on { r.y + 2. } else { r.cy() },
        r.w - 4.,
        r.h * 0.5 - 2.,
    );
    ui.c.fill_rect(half, if ui.t.style.neon_3036 { if on { ui.t.accent } else { ui.t.button_border } } else if on { ACCENT } else { Color::rgb(0x888888) });
}
pub(super) fn vu(ui: &mut Ui, r: Rect, levels: [f32; 2], clipped: bool, _selected: bool) {
    if ui.t.style.neon_3036 {
        let t = ui.t;
        ui.c.fill_rect(r,t.mixer_strip_dark);
        ui.c.stroke_rrect(r.inset(0.5),1.,1.,t.button_border);
        let face = r.inset(2.);
        ui.c.push_clip(face);
        let (cx,cy,x,y) = super::analog::meter_scale(ui,face,levels,t.text_dim,t.meter_clip);
        ui.c.line(cx,cy,x,y,3.,t.accent.with_alpha(ui.c.neon_glow_alpha(60)));
        ui.c.line(cx,cy,x,y,1.15,t.accent);
        ui.c.fill_circle(cx,cy,1.6,t.note_top);
        if clipped { ui.c.fill_circle(face.right()-4.,face.bottom()-4.,1.7,t.meter_clip); }
        ui.c.pop_clip();
        return;
    }
    if !ui.c.visible(r.outset(1.)) {
        return;
    }
    ui.c.fill_rect(r, LINE);
    let face = r.inset(1.5);
    ui.c.fill_rect(face, Color::rgb(0xd6d6d1));
    ui.c.push_clip(face);
    let (cx, cy, x, y) = super::analog::meter_scale(ui, face, levels, LINE, Color::rgb(0xb4453a));
    ui.c.line(cx, cy, x, y, 1.1, LINE);
    ui.c.fill_circle(cx, cy, 1.6, LINE);
    if clipped {
        ui.c.fill_circle(
            face.right() - 4.,
            face.bottom() - 4.,
            1.7,
            Color::rgb(0xc94d45),
        );
    }
    ui.c.pop_clip();
}
