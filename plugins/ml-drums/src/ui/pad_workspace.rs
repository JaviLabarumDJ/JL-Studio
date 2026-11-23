// SPDX-License-Identifier: GPL-3.0-or-later
use super::{
    choose_external_file, spawn_layer_dialog, DisplaySnapshot, EditorState, ExternalFile, GuiAction,
};
use crate::engine::kit::{DrumKit, LayerControls, Performance, SampleLayer, BANK_PADS, MAX_LAYERS};
use crate::ui::state::WaveformSummary;
use crate::ui::theme;
#[path = "pad_inspector.rs"]
mod inspector;
use nih_plug_egui::egui::{
    self, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    pub bank: usize,
    pub layer: usize,
    pub drop_target: Option<(Rect, usize, usize)>,
    waveform: Option<(Arc<Vec<f32>>, WaveformSummary)>,
    pan_envelope: bool,
    learn: Option<(usize, usize)>,
    last_hit: u32,
    rename: Option<(usize, String)>,
    show_inspector: Option<bool>,
}

impl State {
    pub fn read_midi(&mut self, hit: u32, actions: &mut Vec<GuiAction>) {
        if hit == self.last_hit {
            return;
        }
        self.last_hit = hit;
        if let Some((index, end)) = self.learn {
            actions.push(GuiAction::PadEdit(index, Edit::Note(hit as u8)));
            self.learn = (index + 1 < end).then_some((index + 1, end));
        }
    }
}

pub(super) enum Edit {
    Name(String),
    Note(u8),
    Performance(Performance),
    Layer(usize, LayerControls),
    Add,
    Remove(usize),
    Swap(usize, usize),
    Spread,
}

pub(super) fn apply(kit: &mut DrumKit, index: usize, edit: Edit) {
    if let Edit::Note(note) = edit {
        if let Some(other) = kit.pads.iter().position(|pad| pad.midi_note == note) {
            kit.pads[other].midi_note = kit.pads[index].midi_note;
        }
        kit.pads[index].midi_note = note;
        return;
    }
    let pad = &mut kit.pads[index];
    match edit {
        Edit::Name(name) => pad.name = name,
        Edit::Performance(performance) => pad.performance = performance,
        Edit::Layer(0, controls) => pad.primary = controls,
        Edit::Layer(i, controls) => {
            if let Some(layer) = pad.layers.get_mut(i - 1) {
                layer.controls = controls;
            }
        }
        Edit::Add if pad.layers.len() + 1 < MAX_LAYERS => pad.layers.push(SampleLayer::default()),
        Edit::Remove(0) => {
            let layer = if pad.layers.is_empty() {
                SampleLayer::default()
            } else {
                pad.layers.remove(0)
            };
            pad.sample = layer.sample;
            pad.right = layer.right;
            pad.sample_path = layer.path;
            pad.primary = layer.controls;
        }
        Edit::Remove(i) if i <= pad.layers.len() => {
            pad.layers.remove(i - 1);
        }
        Edit::Swap(a, b) => pad.swap_layers(a, b),
        Edit::Spread => pad.spread_layers(),
        _ => {}
    }
}

pub(super) fn draw(
    ui: &mut Ui,
    state: &mut EditorState,
    snap: &DisplaySnapshot,
    triggers: &[AtomicU8],
    actions: &mut Vec<GuiAction>,
) {
    let inspector = *state.pad_workspace.show_inspector.get_or_insert_with(|| {
        os_paths::user_dir()
            .and_then(|path| std::fs::read_to_string(path.join("settings.txt")).ok())
            .is_none_or(|text| {
                !text.lines().any(|line| {
                    line.split_once('=').is_some_and(|(key, value)| {
                        key.trim() == crate::PAD_EDITOR_VISIBILITY
                            && matches!(value.trim(), "false" | "0")
                    })
                })
            })
    });
    if state
        .selected_pad
        .is_none_or(|pad| pad / BANK_PADS != state.pad_workspace.bank)
    {
        state.selected_pad = Some(state.pad_workspace.bank * BANK_PADS);
        state.pad_workspace.layer = 0;
    }
    let available = ui.available_size();
    let matrix_width = if inspector {
        (available.x * 0.39).clamp(244.0, 416.0)
    } else {
        available.x
    };
    state.pad_workspace.drop_target = None;
    let (area, _) = ui.allocate_exact_size(available, Sense::hover());
    let matrix = Rect::from_min_size(area.min, egui::vec2(matrix_width, area.height()));
    let mut pad_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("drums-pads")
            .max_rect(matrix)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    pad_ui.set_clip_rect(ui.clip_rect().intersect(matrix));
    {
        let ui = &mut pad_ui;
        ui.horizontal(|ui| {
            for (bank, label) in ["Bank A", "Bank B"].into_iter().enumerate() {
                if ui
                    .selectable_label(state.pad_workspace.bank == bank, label)
                    .clicked()
                {
                    state.pad_workspace.bank = bank;
                    state.selected_pad = Some(bank * BANK_PADS);
                    state.pad_workspace.layer = 0;
                    state.pad_workspace.learn = None;
                }
            }
            ui.menu_button("Kit", |ui| {
                for (label, kind) in [
                    ("Open kit file…", ExternalFile::Kit),
                    ("Import MIDI groove…", ExternalFile::Groove),
                    ("Load note layout…", ExternalFile::LoadMap),
                    ("Save note layout…", ExternalFile::SaveMap),
                    ("Export SFZ kit…", ExternalFile::Sfz),
                ] {
                    if ui.button(label).clicked() {
                        choose_external_file(state, kind);
                        ui.close_menu();
                    }
                }
                if ui.button("Clear kit").clicked() {
                    actions.push(GuiAction::ClearKit);
                    ui.close_menu();
                }
                if ui.button("Learn bank notes").clicked() {
                    let start = state.pad_workspace.bank * BANK_PADS;
                    state.pad_workspace.learn = Some((start, start + BANK_PADS));
                    ui.close_menu();
                }
            });
        });
        if let Some((index, _)) = state.pad_workspace.learn {
            ui.horizontal(|ui| {
                ui.label(format!("Play a MIDI note for pad {}", index + 1));
                if ui.small_button("Cancel").clicked() {
                    state.pad_workspace.learn = None;
                }
            });
        }
        let side = ((matrix_width - 18.) / 4.)
            .min((ui.available_height() - 24.) / 4.)
            .max(40.0);
        let grid = egui::Grid::new("ml-drums-bank")
            .num_columns(4)
            .spacing([6., 6.]);
        grid.show(ui, |ui| {
            for row in (0..4).rev() {
                for column in 0..4 {
                    let index = state.pad_workspace.bank * BANK_PADS + row * 4 + column;
                    let pad = &snap.pads[index];
                    let tint = pad.performance.color.map_or_else(
                        || theme::category_color32(pad.category),
                        |[r, g, b]| Color32::from_rgb(r, g, b),
                    );
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(side, side), Sense::click());
                    state.pad_rects[index] = Some(rect);
                    let selected = state.selected_pad == Some(index);
                    let painter = ui.painter_at(rect);
                    let level = state.brightness[index];
                    let fill = if level > 0.02 {
                        tint.gamma_multiply(0.33 + 0.57 * level)
                    } else if response.hovered() {
                        Color32::from_rgb(46, 52, 68)
                    } else {
                        Color32::from_rgb(31, 37, 51)
                    };
                    let edge = if selected {
                        theme::ACCENT
                    } else {
                        tint.gamma_multiply(0.56)
                    };
                    painter.rect(
                        rect,
                        9.0,
                        fill,
                        Stroke::new(if selected { 2. } else { 1. }, edge),
                        StrokeKind::Inside,
                    );
                    painter.rect_filled(
                        Rect::from_min_size(
                            rect.left_top() + egui::vec2(8.0, 10.0),
                            egui::vec2(3.0, 15.0),
                        ),
                        1.0,
                        tint,
                    );
                    painter.text(
                        rect.left_top() + egui::vec2(16.0, 8.0),
                        Align2::LEFT_TOP,
                        format!("{:02}", index + 1),
                        FontId::monospace(12.),
                        Color32::WHITE,
                    );
                    let name = pad.name.trim_end_matches(".wav");
                    if rect.height() > 57.0 {
                        painter.text(
                            rect.left_bottom() + egui::vec2(8., -8.),
                            Align2::LEFT_BOTTOM,
                            name,
                            FontId::proportional(11.),
                            theme::TEXT_PRIMARY,
                        );
                    }
                    if pad.performance.muted || pad.performance.solo {
                        painter.circle_filled(
                            rect.right_top() + egui::vec2(-10., 11.),
                            3.,
                            if pad.performance.muted {
                                theme::MUTE_RED
                            } else {
                                theme::SOLO_YELLOW
                            },
                        );
                    }
                    if response.is_pointer_button_down_on()
                        && ui.input(|input| input.pointer.primary_pressed())
                    {
                        let v = response.interact_pointer_pos().map_or(102, |p| {
                            (((p.y - rect.top()) / rect.height()) * 126.0 + 1.0).clamp(1.0, 127.0)
                                as u8
                        });
                        triggers[index].store(v, Ordering::Relaxed);
                        if state.selected_pad != Some(index) {
                            state.pad_workspace.layer = 0;
                        }
                        state.selected_pad = Some(index);
                        state.pad_workspace.learn = None;
                    }
                    response.context_menu(|ui| {
                        if ui.button("Audition").clicked() {
                            triggers[index].store(102, Ordering::Relaxed);
                            ui.close_menu();
                        }
                        if ui.button("Load sample…").clicked() {
                            spawn_layer_dialog(state, index, Some(0));
                            ui.close_menu();
                        }
                        if ui.button("Add sample layers…").clicked() {
                            spawn_layer_dialog(state, index, None);
                            ui.close_menu();
                        }
                        if snap.has_library && ui.button("Dice pad").clicked() {
                            actions.push(GuiAction::DicePad(index));
                            ui.close_menu();
                        }
                        if snap.has_library && ui.button("Dice this category").clicked() {
                            actions.push(GuiAction::DiceCategory(index, pad.category));
                            ui.close_menu();
                        }
                        if inspector && ui.button("Rename").clicked() {
                            state.selected_pad = Some(index);
                            state.pad_workspace.rename = Some((index, pad.name.clone()));
                            ui.close_menu();
                        }
                        if ui
                            .button(if pad.locked { "Unlock" } else { "Lock" })
                            .clicked()
                        {
                            actions.push(GuiAction::ToggleLock(index));
                            ui.close_menu();
                        }
                    });
                }
                ui.end_row();
            }
        });
    }
    if inspector {
        let index = state
            .selected_pad
            .unwrap_or(state.pad_workspace.bank * BANK_PADS);
        let panel = Rect::from_min_max(egui::pos2(matrix.right() + 10.0, area.top()), area.max);
        let mut panel_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("drums-inspector")
                .max_rect(panel)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        panel_ui.set_clip_rect(ui.clip_rect().intersect(panel));
        egui::ScrollArea::vertical()
            .id_salt("drums-pad-inspector")
            .max_height(panel.height())
            .show(&mut panel_ui, |ui| {
                inspector::draw(ui, index, state, snap, triggers, actions)
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::kit::NUM_PADS;
    use crate::ui::state::SharedState;

    #[test]
    fn inspector_keeps_the_main_controls_visible_with_a_full_layer_list() {
        let context = egui::Context::default();
        let bounds = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1060.0, 540.0));
        let mut shared = SharedState::new();
        let sample = Arc::new(vec![0.4, -0.5, 0.3, -0.2]);
        let pad = &mut shared.kit.pads[0];
        let long_name = format!("{}.wav", "Long sampled drum name ".repeat(6));
        pad.name = long_name.clone();
        pad.sample = Some(sample.clone());
        pad.sample_path = Some(long_name.clone());
        pad.layers = vec![
            SampleLayer {
                path: Some(long_name),
                sample: Some(sample),
                ..Default::default()
            };
            MAX_LAYERS - 1
        ];
        shared.update_waveform(0, 24);

        let snapshot = DisplaySnapshot::from_shared(&shared);
        let mut state = EditorState::default();
        state.pad_workspace.show_inspector = Some(true);
        let mut actions = Vec::new();
        let triggers = std::array::from_fn::<_, NUM_PADS, _>(|_| AtomicU8::new(0));
        let mut render = |events, state: &mut EditorState| {
            let input = egui::RawInput {
                screen_rect: Some(bounds),
                events,
                ..Default::default()
            };
            context.run(input, |ctx| {
                theme::setup_style(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_space(44.0);
                    draw(ui, state, &snapshot, &triggers, &mut actions);
                });
            })
        };
        let _ = render(Vec::new(), &mut state);
        let output = render(Vec::new(), &mut state);
        let mut labels = Vec::new();
        fn text_rects<'a>(shape: &'a egui::Shape, clip: Rect, labels: &mut Vec<(&'a str, Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let rect = Rect::from_min_size(text.pos, text.galley.size());
                    if clip.contains_rect(rect) {
                        labels.push((text.galley.text(), rect));
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text_rects(shape, clip, labels);
                    }
                }
                _ => {}
            }
        }
        for shape in &output.shapes {
            text_rects(&shape.shape, shape.clip_rect.intersect(bounds), &mut labels);
        }
        let visible = |label| {
            labels
                .iter()
                .find(|(text, _)| *text == label)
                .map(|(_, rect)| *rect)
        };
        let wave = state.pad_workspace.drop_target.unwrap().0;
        for label in [
            "Level",
            "Pan",
            "Tune",
            "MIDI learn",
            "Out Main",
            "Layers",
            "Layer gain",
            "Reverse",
            "Sample",
            "Start",
            "End",
            "Length",
            "Start Fade",
            "End Fade",
            "Volume env",
            "Pan env",
            "Release (s)",
        ] {
            let rect =
                visible(label).unwrap_or_else(|| panic!("Inspector control is clipped: {label}"));
            assert!(rect.left() >= wave.left() - 12.0, "{label}");
        }
        let release = visible("Release (s)").unwrap();
        assert!(release.left() > wave.right() && release.top() > wave.bottom());

        let level = visible("Level").unwrap();
        let knob = egui::pos2(level.center().x, level.bottom() + 18.0);
        let layer_two = labels
            .iter()
            .find(|(text, _)| text.starts_with("02   "))
            .unwrap()
            .1
            .center();
        let click = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let drag_to = knob + egui::vec2(4.0, -14.0);
        let _ = render(
            vec![egui::Event::PointerMoved(knob), click(knob, true)],
            &mut state,
        );
        let _ = render(vec![egui::Event::PointerMoved(drag_to)], &mut state);
        let _ = render(vec![click(drag_to, false)], &mut state);
        let _ = render(
            vec![egui::Event::PointerMoved(layer_two), click(layer_two, true)],
            &mut state,
        );
        let _ = render(vec![click(layer_two, false)], &mut state);
        assert_eq!(state.pad_workspace.layer, 1);
        let (wave, pad, layer) = state.pad_workspace.drop_target.unwrap();
        assert_eq!((pad, layer), (0, 1));
        let position = wave.center();
        let _ = render(
            vec![egui::Event::PointerMoved(position), click(position, true)],
            &mut state,
        );
        let _ = render(vec![click(position, false)], &mut state);
        assert!(actions
            .iter()
            .any(|action| matches!(action, GuiAction::PreviewLayer(0, 1))));
        assert!(actions.iter().any(|action| matches!(
            action,
            GuiAction::SetPadParam(0, crate::ui::editor::PadParam::Volume, value)
                if *value > snapshot.pads[0].volume
        )));
    }

    #[test]
    fn both_pad_banks_draw_and_audition_at_the_default_size() {
        let context = egui::Context::default();
        let bounds = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1060.0, 540.0));
        let snapshot = DisplaySnapshot::from_shared(&SharedState::new());
        let mut state = EditorState::default();
        state.pad_workspace.show_inspector = Some(true);
        let mut actions = Vec::new();
        let triggers = std::array::from_fn::<_, NUM_PADS, _>(|_| AtomicU8::new(0));
        let mut render = |state: &mut EditorState, events| {
            let input = egui::RawInput {
                screen_rect: Some(bounds),
                events,
                ..Default::default()
            };
            let _output = context.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_space(44.0);
                    draw(ui, state, &snapshot, &triggers, &mut actions);
                });
            });
        };
        for bank in 0..2 {
            state.pad_workspace.bank = bank;
            state.pad_rects.fill(None);
            render(&mut state, Vec::new());
            let start = bank * BANK_PADS;
            assert_eq!(state.pad_rects.iter().flatten().count(), BANK_PADS);
            let first = state.pad_rects[start].unwrap();
            let last = state.pad_rects[start + BANK_PADS - 1].unwrap();
            assert!(bounds.contains_rect(first) && bounds.contains_rect(last));
            assert!(last.left() > first.left() && last.bottom() < first.bottom());
            let (wave, pad, layer) = state.pad_workspace.drop_target.unwrap();
            assert!(wave.left() > last.right() && bounds.contains_rect(wave));
            assert_eq!((pad, layer), (start, 0));
        }
        let pos = state.pad_rects[31].unwrap().center();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            &mut state,
            vec![egui::Event::PointerMoved(pos), button(true)],
        );
        assert_eq!(state.selected_pad, Some(31));
        assert!(triggers[31].load(Ordering::Relaxed) > 0);
        render(&mut state, vec![button(false)]);
    }
}
