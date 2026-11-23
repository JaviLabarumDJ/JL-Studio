// SPDX-License-Identifier: GPL-3.0-or-later
//! The CLAP state callback snapshots each instance directly, including its custom oneshots.
use crate::engine::{kit::{DrumKit, NUM_PADS}, sequencer::PatternBank};
use crate::ui::state::SharedState;
use crate::util::{audio_file, default_kit, preset::{self, Preset}};
use base64::{engine::general_purpose::STANDARD, Engine};
use nih_plug::params::persist::PersistentField;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct SavedKit {
    kit: Preset,
    sample_rate: u32,
    audio: Vec<Option<String>>,
    #[serde(default)]
    layer_audio: Vec<Vec<Option<String>>>,
    #[serde(default)]
    right_audio: Vec<Option<String>>,
    #[serde(default)]
    layer_right_audio: Vec<Vec<Option<String>>>,
}

impl SavedKit {
    fn capture(live: &Mutex<SharedState>) -> Self {
        // Release the performance lock before encoding the custom sample buffers.
        let (kit, rate, channels) = {
            let live = live.lock();
            let channels: Vec<_> = live.kit.pads.iter().map(|pad| {
                let base = if pad.sample_path.as_deref().is_some_and(|s| s.starts_with("factory://")) {
                    (None, None)
                } else { (pad.sample.clone(), pad.right.clone()) };
                let layers: Vec<_> = pad.layers.iter().map(|layer| (layer.sample.clone(), layer.right.clone())).collect();
                (base, layers)
            }).collect();
            (preset::from_kit("ML Drums", &live.kit, &live.pattern_bank), live.sample_rate, channels)
        };
        let encode = |data: Option<Arc<Vec<f32>>>| data.map(|pcm| {
            let bytes: Vec<_> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            STANDARD.encode(bytes)
        });
        let mut saved = Self {
            kit, sample_rate: rate as u32, audio: Vec::new(), layer_audio: Vec::new(),
            right_audio: Vec::new(), layer_right_audio: Vec::new(),
        };
        for ((left,right), layers) in channels {
            saved.audio.push(encode(left));
            saved.right_audio.push(encode(right));
            let (left,right) = layers.into_iter().map(|(l,r)| (encode(l), encode(r))).unzip();
            saved.layer_audio.push(left);
            saved.layer_right_audio.push(right);
        }
        saved
    }

    fn install(self, live: &Mutex<SharedState>, rate: f32) {
        let decode = |field: Option<&str>| {
            let bytes = STANDARD.decode(field?).ok()?;
            if bytes.len() % 4 != 0 || !(1..=768000).contains(&self.sample_rate) { return None; }
            let pcm = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
            audio_file::resample_if_needed(pcm, self.sample_rate, rate, "project pad").ok().map(Arc::new)
        };
        let stereo = |left: Option<&Option<String>>, right: Option<&Option<String>>| {
            (decode(left.and_then(Option::as_deref)), decode(right.and_then(Option::as_deref)))
        };
        let audio: Vec<_> = self.kit.pads.iter().take(NUM_PADS).enumerate().map(|(i, pad)| {
            let mut layers = vec![stereo(self.audio.get(i), self.right_audio.get(i))];
            for layer in 0..pad.layers.len().min(crate::engine::kit::MAX_LAYERS - 1) {
                layers.push(stereo(self.layer_audio.get(i).and_then(|v| v.get(layer)),
                    self.layer_right_audio.get(i).and_then(|v| v.get(layer))));
            }
            layers
        }).collect();
        let mut kit = DrumKit::new();
        let mut bank = PatternBank::new();
        preset::apply_with_samples(&self.kit, &mut kit, &mut bank, rate, &audio);
        let mut live = live.lock();
        live.kit = kit;
        live.pattern_bank = bank;
        live.sample_rate = rate;
        live.update_all_waveforms(200);
    }
}

pub struct ProjectState {
    live: Arc<Mutex<SharedState>>,
    pending: Mutex<Option<SavedKit>>,
}

impl ProjectState {
    pub fn new(live: Arc<Mutex<SharedState>>) -> Self {
        Self { live, pending: Mutex::new(None) }
    }

    pub fn activate(&self, rate: f32) {
        let old_rate = self.live.lock().sample_rate;
        let saved = self.pending.lock().take();
        if let Some(saved) = saved {
            saved.install(&self.live, rate);
        } else if old_rate == 0.0 {
            let mut live = self.live.lock();
            default_kit::apply_to_kit(&mut live, rate);
            live.sample_rate = rate;
        } else if rate != old_rate {
            SavedKit::capture(&self.live).install(&self.live, rate);
        }
        if old_rate != 0.0 && old_rate != rate {
            let mut live = self.live.lock();
            live.library = None;
            live.bg_rx = None;
            live.scan_status = crate::ui::state::ScanStatus::NeedsSetup { suggested_path: None };
        }
    }
}

impl<'a> PersistentField<'a, SavedKit> for ProjectState {
    fn set(&self, value: SavedKit) { *self.pending.lock() = Some(value); }

    fn map<F: Fn(&SavedKit) -> R, R>(&self, f: F) -> R {
        if let Some(saved) = self.pending.lock().as_ref() { return f(saved); }
        f(&SavedKit::capture(&self.live))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ml_drums_custom_pad_moves_with_project_at_new_rate() {
        let live = Arc::new(Mutex::new(SharedState::new()));
        let field = ProjectState::new(live.clone());
        field.activate(48000.0);
        {
            let mut state = live.lock();
            let pad = &mut state.kit.pads[1];
            pad.sample_path = Some("/no-longer-present/snare.wav".into());
            pad.sample = Some(Arc::new(vec![0.25; 480]));
            pad.locked = true;
            pad.pan = -0.5;
            let bank_b = &mut state.kit.pads[18];
            bank_b.performance.output = 5;
            bank_b.performance.edge_fades.start_ms = Some(2.5);
            bank_b.performance.edge_fades.end_ms = Some(4.0);
            bank_b.layers.push(crate::engine::kit::SampleLayer {
                path: Some("/no-longer-present/stereo.wav".into()),
                sample: Some(Arc::new(vec![0.4; 480])),
                right: Some(Arc::new(vec![-0.2; 480])),
                controls: crate::engine::kit::LayerControls { low: 90, ..Default::default() },
            });
            state.pattern_bank.patterns[3].lanes[1].steps[5].enabled = true;
            state.pattern_bank.patterns[3].lanes[18].steps[7].enabled = true;
            state.pattern_bank.active = 3;
        }
        let bytes = field.map(|s| serde_json::to_vec(s).unwrap());
        let next = Arc::new(Mutex::new(SharedState::new()));
        let restored = ProjectState::new(next.clone());
        restored.set(serde_json::from_slice(&bytes).unwrap());
        restored.activate(96000.0);
        let state = next.lock();
        let pad = &state.kit.pads[1];
        assert_eq!(pad.pan, -0.5);
        assert!(pad.locked && state.pattern_bank.active_pattern().lanes[1].steps[5].enabled);
        let pcm = pad.sample.as_ref().unwrap();
        assert_eq!(pcm.len(), 960);
        let bank_b = &state.kit.pads[18];
        assert_eq!(bank_b.performance.output, 5);
        assert_eq!(bank_b.performance.edge_fades.start_ms, Some(2.5));
        assert_eq!(bank_b.performance.edge_fades.end_ms, Some(4.0));
        assert!(state.pattern_bank.active_pattern().lanes[18].steps[7].enabled);
        let layer = &bank_b.layers[0];
        assert_eq!(layer.controls.low, 90);
        for (samples, expected) in [(layer.sample.as_ref().unwrap(), 0.4), (layer.right.as_ref().unwrap(), -0.2)] {
            assert_eq!(samples.len(), 960);
            assert!((samples[300] - expected).abs() < 0.005);
        }
        assert!((pcm[300] - 0.25).abs() < 0.005);
        assert!(state.kit.pads[0].sample.as_ref().unwrap().iter().any(|v| v.abs() > 0.01));
    }
}
