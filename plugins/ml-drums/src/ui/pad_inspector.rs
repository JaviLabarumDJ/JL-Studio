// SPDX-License-Identifier: GPL-3.0-or-later
use super::{
    choose_external_file, spawn_layer_dialog, DisplaySnapshot, Edit, EditorState, ExternalFile,
    GuiAction,
};
use crate::engine::kit::{Envelope, LayerMode, Performance, MAX_LAYERS};
use crate::ui::editor::{PadDisplay, PadParam, WAVEFORM_POINTS};
use crate::ui::state::WaveformSummary;
use crate::ui::theme;
use nih_plug_egui::egui::{
    self, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
};
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

const PANEL: Color32 = Color32::from_rgb(28, 33, 45);
const INSET: Color32 = Color32::from_rgb(19, 23, 33);
const EDGE: Color32 = Color32::from_rgb(53, 61, 77);
const GRID: Color32 = Color32::from_rgb(41, 47, 61);
const TEXT: Color32 = Color32::from_rgb(222, 229, 239);
const LABEL: Color32 = Color32::from_rgb(162, 174, 193);
const ACCENT: Color32 = Color32::from_rgb(112, 218, 196);
const ROW_HEIGHT: f32 = 23.0;

pub(super) fn draw(
    ui: &mut Ui,
    index: usize,
    state: &mut EditorState,
    snap: &DisplaySnapshot,
    triggers: &[AtomicU8],
    actions: &mut Vec<GuiAction>,
) {
    let pad = &snap.pads[index];
    let mut performance = pad.performance;
    ui.push_id(index, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(7.0, 5.0);
        ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
        ui.spacing_mut().interact_size.y = 21.0;
        ui.visuals_mut().override_text_color = Some(TEXT);
        ui.visuals_mut().widgets.inactive.weak_bg_fill = GRID;
        ui.visuals_mut().widgets.hovered.weak_bg_fill = EDGE;
        ui.visuals_mut().selection.bg_fill = Color32::from_rgb(40, 77, 76);

        card(ui, |ui| {
            header(ui, index, state, pad, &mut performance, actions);
            ui.add_space(2.0);
            ui.separator();
            let side_by_side = ui.available_width() >= 470.0;
            ui.horizontal_top(|ui| {
                for (param, label, value, range, default, precision) in [
                    (PadParam::Volume, "Level", pad.volume, 0.0..=2.0, 1.0, 2),
                    (PadParam::Pan, "Pan", pad.pan, -1.0..=1.0, 0.0, 2),
                    (PadParam::Pitch, "Tune", pad.pitch, -48.0..=48.0, 0.0, 1),
                ] {
                    let mut value = value;
                    if rotary(ui, label, &mut value, range, default, precision) {
                        actions.push(GuiAction::SetPadParam(index, param, value));
                    }
                }
                ui.add_space(7.0);
                if side_by_side {
                    routing(ui, index, state, pad, &mut performance, actions);
                }
            });
            if !side_by_side {
                routing(ui, index, state, pad, &mut performance, actions);
            }
        });

        card(ui, |ui| {
            layers(
                ui,
                index,
                state,
                pad,
                &mut performance,
                &triggers[index],
                actions,
            )
        });
        let selected = state.pad_workspace.layer.min(pad.layers.len() - 1);
        let sample = |ui: &mut Ui, state: &mut EditorState, actions: &mut Vec<GuiAction>| {
            card(ui, |ui| {
                sample_editor(ui, index, selected, state, snap, actions)
            });
        };
        if ui.available_width() >= 420.0 {
            ui.columns(2, |cols| {
                sample(&mut cols[0], state, actions);
                card(&mut cols[1], |ui| {
                    envelope_editor(ui, state, &mut performance)
                });
            });
        } else {
            sample(ui, state, actions);
            card(ui, |ui| envelope_editor(ui, state, &mut performance));
        }
    });
    if performance != pad.performance {
        actions.push(GuiAction::PadEdit(index, Edit::Performance(performance)));
    }
}

fn card(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    egui::Frame::NONE
        .fill(PANEL)
        .stroke(Stroke::new(1.0, EDGE))
        .corner_radius(6)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width((width - 18.0).max(0.0));
            content(ui);
        });
}

fn header(
    ui: &mut Ui,
    index: usize,
    state: &mut EditorState,
    pad: &PadDisplay,
    performance: &mut Performance,
    actions: &mut Vec<GuiAction>,
) {
    let accent = performance.color.map_or_else(
        || theme::category_color32(pad.category),
        |[r, g, b]| Color32::from_rgb(r, g, b),
    );
    ui.horizontal_wrapped(|ui| {
        let (swatch, _) = ui.allocate_exact_size(egui::vec2(3.0, 17.0), Sense::hover());
        ui.painter().rect_filled(swatch, 1, accent);
        ui.label(RichText::new(format!("Pad {:02}", index + 1)).color(LABEL));
        let width = (ui.available_width() - 184.0).clamp(74.0, 300.0);
        let rename = &mut state.pad_workspace.rename;
        if let Some((_, text)) = rename.as_mut().filter(|(pad, _)| *pad == index) {
            let field = ui.add(egui::TextEdit::singleline(text).desired_width(width));
            let (cancel, submit) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::Escape),
                    input.key_pressed(egui::Key::Enter),
                )
            });
            if cancel || submit || field.lost_focus() {
                let name = text.trim().chars().take(64).collect::<String>();
                if !cancel && !name.is_empty() && name != pad.name {
                    actions.push(GuiAction::PadEdit(index, Edit::Name(name)));
                }
                *rename = None;
            } else if !field.has_focus() {
                field.request_focus();
            }
        } else if ui
            .add_sized(
                [width, 23.0],
                egui::Button::new(RichText::new(&pad.name).strong())
                    .frame(false)
                    .truncate(),
            )
            .on_hover_text("Rename pad")
            .clicked()
        {
            *rename = Some((index, pad.name.clone()));
        }
        let [r, g, b, _] = accent.to_array();
        let mut rgb = [r, g, b];
        if ui
            .color_edit_button_srgb(&mut rgb)
            .on_hover_text("Pad color")
            .changed()
        {
            performance.color = Some(rgb);
        }
        ui.toggle_value(&mut performance.muted, "Mute");
        ui.toggle_value(&mut performance.solo, "Solo");
        if performance.color.is_some() {
            ui.menu_button("Color", |ui| {
                if ui.button("Reset pad color").clicked() {
                    performance.color = None;
                    ui.close_menu();
                }
            });
        }
    });
}

fn routing(
    ui: &mut Ui,
    index: usize,
    state: &mut EditorState,
    pad: &PadDisplay,
    performance: &mut Performance,
    actions: &mut Vec<GuiAction>,
) {
    ui.vertical(|ui| {
        ui.horizontal_wrapped(|ui| {
            let mut note = pad.midi_note;
            let name = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
            if ui.add_sized([76.0, 21.0], egui::DragValue::new(&mut note).range(0..=127).prefix("Note "))
                .on_hover_text(format!("{}{} · MIDI {}", name[usize::from(pad.midi_note % 12)], i16::from(pad.midi_note / 12) - 1, pad.midi_note))
                .changed()
            {
                actions.push(GuiAction::PadEdit(index, Edit::Note(note)));
            }
            ui.menu_button("MIDI learn", |ui| {
                if ui.button("Learn next note").clicked() {
                    state.pad_workspace.learn = Some((index, index + 1));
                    ui.close_menu();
                }
                if ui.add_enabled(state.pad_workspace.last_hit != 0, egui::Button::new("Use last note")).clicked() {
                    actions.push(GuiAction::PadEdit(index, Edit::Note(state.pad_workspace.last_hit as u8)));
                    ui.close_menu();
                }
            });
            ui.checkbox(&mut performance.velocity_gain, "Velocity gain");
            let mut kit_fx = !performance.source_level;
            if ui.checkbox(&mut kit_fx, "Kit FX")
                .on_hover_text("Send the dry hit through the kit compressor, drive and limiter")
                .changed()
            {
                performance.source_level = !kit_fx;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.add_sized([59.0, 21.0], egui::DragValue::new(&mut performance.cut).range(0..=32).prefix("Cut "))
                .on_hover_text("Choke pads in this group");
            ui.add_sized([70.0, 21.0], egui::DragValue::new(&mut performance.cut_by).range(0..=32).prefix("Cut by "))
                .on_hover_text("This group can choke the pad");
            let label = |offset| if offset == 0 { "Main".to_owned() } else { format!("+{offset}") };
            egui::ComboBox::from_id_salt("mixer-output").width(79.0)
                .selected_text(format!("Out {}", label(performance.output))).show_ui(ui, |ui| {
                    for output in 0..=crate::engine::kit::AUX_OUTPUTS as u8 {
                        ui.selectable_value(&mut performance.output, output, label(output));
                    }
                }).response.on_hover_text("Mixer output offset. Enable Split plugin outputs to consecutive inserts in the Channel Rack.");
        });
    });
}

fn rotary(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    default: f32,
    precision: usize,
) -> bool {
    rotary_sized(ui, label, value, range, default, precision, 56.0, "")
}

fn rotary_sized(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    default: f32,
    precision: usize,
    width: f32,
    suffix: &str,
) -> bool {
    let mut changed = false;
    ui.allocate_ui_with_layout(
        egui::vec2(width, 65.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(label).size(11.0).color(LABEL));
            let (rect, knob) =
                ui.allocate_exact_size(egui::vec2(34.0, 34.0), Sense::click_and_drag());
            let knob = knob
                .on_hover_cursor(egui::CursorIcon::ResizeVertical)
                .on_hover_text(
                    "Drag to adjust · Shift for fine adjustment · Double-click to reset",
                );
            let delta = knob.drag_delta();
            let span = (range.end() - range.start()).max(f32::EPSILON);
            let next = if knob.double_clicked() {
                default.clamp(*range.start(), *range.end())
            } else if knob.dragged() {
                let divisor = if ui.input(|input| input.modifiers.shift) {
                    900.0
                } else {
                    190.0
                };
                (*value + (delta.x - delta.y) * span / divisor).clamp(*range.start(), *range.end())
            } else {
                *value
            };
            changed = *value != next;
            *value = next;

            let center = rect.center();
            let amount = ((*value - range.start()) / span).clamp(0.0, 1.0);
            let angle = |t| std::f32::consts::TAU * (0.375 + 0.75 * t);
            let at = |t, r| center + egui::Vec2::angled(angle(t)) * r;
            let arc = |start, end| {
                (0..=28)
                    .map(|i| at(start + (end - start) * i as f32 / 28.0, 15.0))
                    .collect()
            };
            let painter = ui.painter();
            painter.circle(
                center,
                11.5,
                if knob.hovered() || knob.dragged() {
                    EDGE
                } else {
                    GRID
                },
                Stroke::new(1.0, EDGE),
            );
            painter.add(egui::Shape::line(arc(0.0, 1.0), Stroke::new(2.5, EDGE)));
            let anchor = if *range.start() < 0.0 {
                (-range.start() / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            painter.add(egui::Shape::line(
                arc(anchor, amount),
                Stroke::new(2.5, ACCENT),
            ));
            painter.line_segment([at(amount, 4.0), at(amount, 9.0)], Stroke::new(1.8, TEXT));
            changed |= ui
                .add_sized(
                    [(width - 4.0).max(40.0), 16.0],
                    egui::DragValue::new(value)
                        .range(range)
                        .fixed_decimals(precision)
                        .suffix(suffix)
                        .speed(f64::from(span / 220.0)),
                )
                .changed();
        },
    );
    changed
}

fn layers(
    ui: &mut Ui,
    index: usize,
    state: &mut EditorState,
    pad: &PadDisplay,
    performance: &mut Performance,
    trigger: &AtomicU8,
    actions: &mut Vec<GuiAction>,
) {
    let mut selected = state.pad_workspace.layer.min(pad.layers.len() - 1);
    ui.horizontal_wrapped(|ui| {
        ui.strong("Layers");
        egui::ComboBox::from_id_salt("layer-mode")
            .width(92.0)
            .selected_text(performance.layer_mode.label())
            .show_ui(ui, |ui| {
                for mode in LayerMode::ALL {
                    ui.selectable_value(&mut performance.layer_mode, mode, mode.label());
                }
            })
            .response
            .on_hover_text("Playback mode for layers in the same velocity range");
        ui.add_space(4.0);
        if ui
            .add_enabled(
                pad.layers.len() < MAX_LAYERS,
                egui::Button::new("Add samples…"),
            )
            .clicked()
        {
            spawn_layer_dialog(state, index, None);
        }
        ui.menu_button("Layer actions", |ui| {
            let mut edit = |change| actions.push(GuiAction::PadEdit(index, change));
            if ui
                .add_enabled(
                    pad.layers.len() < MAX_LAYERS,
                    egui::Button::new("Add blank layer"),
                )
                .clicked()
            {
                edit(Edit::Add);
                state.pad_workspace.layer = pad.layers.len();
                ui.close_menu();
            }
            if ui.button("Spread velocity evenly").clicked() {
                edit(Edit::Spread);
                ui.close_menu();
            }
            if ui
                .add_enabled(selected > 0, egui::Button::new("Move up"))
                .clicked()
            {
                edit(Edit::Swap(selected, selected - 1));
                state.pad_workspace.layer = selected - 1;
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    selected + 1 < pad.layers.len(),
                    egui::Button::new("Move down"),
                )
                .clicked()
            {
                edit(Edit::Swap(selected, selected + 1));
                state.pad_workspace.layer = selected + 1;
                ui.close_menu();
            }
            if ui.button("Remove selected layer").clicked() {
                edit(Edit::Remove(selected));
                state.pad_workspace.layer = selected.saturating_sub(1);
                ui.close_menu();
            }
        });
    });

    let range_width = (ui.available_width() * 0.27).clamp(82.0, 160.0);
    egui::ScrollArea::vertical()
        .id_salt("layer-rows")
        .max_height(2.0 * (ROW_HEIGHT + 3.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            for (i, (layer, path, sample)) in pad.layers.iter().enumerate() {
                let name = path
                    .as_deref()
                    .and_then(|path| Path::new(path).file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or("Empty layer");
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - range_width - 44.0).max(80.0);
                    if ui
                        .add_sized(
                            [width, ROW_HEIGHT],
                            egui::Button::new(format!("{:02}   {name}", i + 1))
                                .selected(i == selected)
                                .truncate(),
                        )
                        .on_hover_text(name)
                        .clicked()
                    {
                        selected = i;
                        state.pad_workspace.layer = i;
                    }
                    velocity_range(
                        ui,
                        range_width,
                        layer.low,
                        layer.high,
                        i == selected,
                        trigger,
                    );
                    if ui
                        .add_enabled(sample.is_some(), egui::Button::new("▶"))
                        .on_hover_text("Audition layer")
                        .clicked()
                    {
                        actions.push(GuiAction::PreviewLayer(index, i));
                    }
                });
            }
        });
    let mut layer = pad.layers[selected].0;
    ui.horizontal_wrapped(|ui| {
        let mut low = f32::from(layer.low);
        let mut high = f32::from(layer.high);
        if rotary(ui, "Min velocity", &mut low, 1.0..=high, 1.0, 0) {
            layer.low = low.round() as u8;
        }
        if rotary(
            ui,
            "Max velocity",
            &mut high,
            f32::from(layer.low)..=127.0,
            127.0,
            0,
        ) {
            layer.high = high.round() as u8;
        }
        rotary(ui, "Layer gain", &mut layer.gain, 0.0..=2.0, 1.0, 2);
        rotary(ui, "Layer pan", &mut layer.pan, -1.0..=1.0, 0.0, 2);
        rotary(ui, "Tune (st)", &mut layer.pitch, -24.0..=24.0, 0.0, 1);
        ui.vertical(|ui| {
            ui.label(RichText::new("Direction").size(10.0).color(LABEL));
            ui.toggle_value(&mut layer.reverse, "Reverse");
        });
    });
    if layer != pad.layers[selected].0 {
        actions.push(GuiAction::PadEdit(index, Edit::Layer(selected, layer)));
    }
}

fn velocity_range(ui: &mut Ui, width: f32, low: u8, high: u8, selected: bool, trigger: &AtomicU8) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect);
    let at = |v| egui::lerp(rect.x_range(), v);
    painter.rect_filled(rect, 3, INSET);
    for i in 1..8 {
        let x = at(i as f32 / 8.0);
        painter.vline(x, rect.y_range(), Stroke::new(1.0, GRID));
    }
    let from = at(f32::from(low.saturating_sub(1)) / 127.0);
    let to = at(f32::from(high) / 127.0);
    let bar = Rect::from_min_max(
        egui::pos2(from, rect.center().y - 4.0),
        egui::pos2(to, rect.center().y + 4.0),
    );
    painter.rect_filled(bar, 2, if selected { ACCENT } else { LABEL });
    if response
        .on_hover_text(format!("Velocity {low}–{high} · Click to audition the pad"))
        .clicked()
    {
        let velocity = ui.ctx().pointer_interact_pos().map_or(102, |p| {
            (((p.x - rect.left()) / rect.width()) * 126.0 + 1.0).clamp(1.0, 127.0) as u8
        });
        trigger.store(velocity, Ordering::Relaxed);
    }
}

fn sample_editor(
    ui: &mut Ui,
    index: usize,
    selected: usize,
    state: &mut EditorState,
    snap: &DisplaySnapshot,
    actions: &mut Vec<GuiAction>,
) {
    let pad = &snap.pads[index];
    ui.horizontal_wrapped(|ui| {
        ui.strong("Sample");
        if ui
            .small_button("Load…")
            .on_hover_text("Replace selected layer sample")
            .clicked()
        {
            spawn_layer_dialog(state, index, Some(selected));
        }
    });
    let wave = if selected == 0 {
        snap.waveforms[index].as_ref()
    } else if let Some(samples) = pad.layers[selected].2.as_ref() {
        let cache = &mut state.pad_workspace.waveform;
        if cache
            .as_ref()
            .is_none_or(|(cached, _)| !Arc::ptr_eq(samples, cached))
        {
            *cache = Some((
                Arc::clone(samples),
                WaveformSummary::from_samples(samples, WAVEFORM_POINTS),
            ));
        }
        cache.as_ref().map(|(_, wave)| wave)
    } else {
        None
    };
    let (rect, response) = graph(ui, Sense::click());
    let painter = ui.painter_at(rect);
    if let Some(wave) = wave.filter(|wave| !wave.points.is_empty()) {
        let from = egui::lerp(rect.x_range(), pad.start);
        let to = egui::lerp(rect.x_range(), pad.end);
        let height = rect.height() * 0.44;
        for (i, &[low, high]) in wave.points.iter().enumerate() {
            let x = egui::lerp(rect.x_range(), (i as f32 + 0.5) / wave.points.len() as f32);
            let center = rect.center().y;
            let y = (center - high.clamp(-1.0, 1.0) * height)
                ..=(center - low.clamp(-1.0, 1.0) * height);
            painter.vline(
                x,
                egui::Rangef::new(*y.start(), *y.end()),
                Stroke::new(
                    1.4,
                    if x >= from && x <= to {
                        ACCENT
                    } else {
                        LABEL.gamma_multiply(0.45)
                    },
                ),
            );
        }
        for x in [from, to] {
            painter.vline(
                x.clamp(rect.left() + 1.0, rect.right() - 1.0),
                rect.y_range(),
                Stroke::new(1.0, LABEL),
            );
        }
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Drop a sample here",
            FontId::proportional(11.0),
            LABEL,
        );
    }
    if rect.intersects(ui.clip_rect()) {
        state.pad_workspace.drop_target = Some((rect.intersect(ui.clip_rect()), index, selected));
    }
    if response.clicked() && wave.is_some() {
        actions.push(GuiAction::PreviewLayer(index, selected));
    }
    let automatic = crate::engine::trim::EdgeFades::automatic(
        pad.start,
        pad.end,
        pad.decay,
        pad.layers[selected].0.reverse,
    );
    let (start_fade, end_fade) = pad.performance.edge_fades.effective(automatic);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (param, label, mut value, range, default, precision, suffix) in [
            (
                PadParam::Start,
                "Start",
                pad.start,
                0.0..=pad.end,
                0.0,
                3,
                "",
            ),
            (PadParam::End, "End", pad.end, pad.start..=1.0, 1.0, 3, ""),
            (PadParam::Decay, "Length", pad.decay, 0.01..=1.0, 1.0, 3, ""),
            (
                PadParam::StartFade,
                "Start Fade",
                start_fade,
                0.0..=crate::engine::trim::MAX_FADE_MS,
                automatic.0,
                1,
                " ms",
            ),
            (
                PadParam::EndFade,
                "End Fade",
                end_fade,
                0.0..=crate::engine::trim::MAX_FADE_MS,
                automatic.1,
                1,
                " ms",
            ),
        ] {
            if rotary_sized(
                ui, label, &mut value, range, default, precision, 49.0, suffix,
            ) {
                actions.push(GuiAction::SetPadParam(index, param, value));
            }
        }
    });
}

fn envelope_editor(ui: &mut Ui, state: &mut EditorState, performance: &mut Performance) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut state.pad_workspace.pan_envelope, false, "Volume env");
        ui.selectable_value(&mut state.pad_workspace.pan_envelope, true, "Pan env");
        let env = if state.pad_workspace.pan_envelope {
            &mut performance.pan_envelope
        } else {
            &mut performance.volume_envelope
        };
        ui.toggle_value(&mut env.enabled, "On")
            .on_hover_text("Enable this envelope");
    });
    let envelope = if state.pad_workspace.pan_envelope {
        &mut performance.pan_envelope
    } else {
        &mut performance.volume_envelope
    };
    let (rect, _) = graph(ui, Sense::hover());
    let range = rect.shrink(5.0);
    let duration = (envelope.attack + envelope.decay + envelope.release + 0.2).max(0.5);
    let display = Envelope {
        enabled: true,
        ..*envelope
    };
    let points = (0..=96)
        .map(|i| {
            let x = i as f32 / 96.0;
            egui::pos2(
                egui::lerp(range.x_range(), x),
                egui::lerp(range.y_range(), 1.0 - display.value(x * duration, duration)),
            )
        })
        .collect();
    ui.painter_at(rect).add(egui::Shape::line(
        points,
        Stroke::new(1.7, if envelope.enabled { ACCENT } else { LABEL }),
    ));
    ui.horizontal_wrapped(|ui| {
        for (label, value, range, default, precision) in [
            ("Attack (s)", &mut envelope.attack, 0.0..=2.0, 0.0, 3),
            ("Decay (s)", &mut envelope.decay, 0.0..=4.0, 0.1, 3),
            ("Sustain", &mut envelope.sustain, 0.0..=1.0, 1.0, 2),
            ("Release (s)", &mut envelope.release, 0.0..=4.0, 0.02, 3),
        ] {
            rotary_sized(ui, label, value, range, default, precision, 54.0, "");
        }
    });
}

fn graph(ui: &mut Ui, sense: Sense) -> (Rect, egui::Response) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 60.0), sense);
    let painter = ui.painter_at(rect);
    painter.rect(rect, 4, INSET, Stroke::new(1.0, EDGE), StrokeKind::Inside);
    for i in 1..8 {
        let frac = i as f32 / 8.0;
        painter.vline(
            egui::lerp(rect.x_range(), frac),
            rect.y_range(),
            Stroke::new(1.0, GRID),
        );
    }
    painter.hline(rect.x_range(), rect.center().y, Stroke::new(1.0, GRID));
    (rect, response)
}
