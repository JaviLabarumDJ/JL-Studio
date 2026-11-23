use crate::analysis::library::SampleLibrary;
use crate::engine::trim::EdgeFades;
use crate::util::history::PadSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// Sample categories for classification and color coding.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SampleCategory {
    Kick,
    Snare,
    Hihat,
    Clap,
    Tom,
    Perc,
    Cymbal,
    Bass,
    Synth,
    Other,
}

impl SampleCategory {
    pub fn all() -> &'static [SampleCategory] {
        &[
            Self::Kick,
            Self::Snare,
            Self::Hihat,
            Self::Clap,
            Self::Tom,
            Self::Perc,
            Self::Cymbal,
            Self::Bass,
            Self::Synth,
            Self::Other,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Kick => "KICK",
            Self::Snare => "SNARE",
            Self::Hihat => "HIHAT",
            Self::Clap => "CLAP",
            Self::Tom => "TOM",
            Self::Perc => "PERC",
            Self::Cymbal => "CYMBAL",
            Self::Bass => "BASS",
            Self::Synth => "SYNTH",
            Self::Other => "OTHER",
        }
    }
}

/// Number of pads in the kit.
pub const BANK_PADS: usize = 16;
pub const NUM_PADS: usize = 2 * BANK_PADS;
pub const LEGACY_PADS: usize = 8;
pub const MAX_LAYERS: usize = 16;
pub const AUX_OUTPUTS: usize = 15;

pub fn default_note(index: usize) -> u8 {
    if index < 24 { 36 + index as u8 } else { 68 + (index - 24) as u8 }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerMode {
    #[default]
    Stack,
    Cycle,
    Random,
    RandomNoRepeat,
}

impl LayerMode {
    pub const ALL: [Self; 4] = [Self::Stack, Self::Cycle, Self::Random, Self::RandomNoRepeat];
    pub fn label(self) -> &'static str {
        match self {
            Self::Stack => "Stack",
            Self::Cycle => "Cycle",
            Self::Random => "Random",
            Self::RandomNoRepeat => "Avoid repeat",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerControls {
    pub low: u8,
    pub high: u8,
    pub gain: f32,
    pub pan: f32,
    pub pitch: f32,
    pub reverse: bool,
}

impl Default for LayerControls {
    fn default() -> Self {
        Self { low: 1, high: 127, gain: 1.0, pan: 0.0, pitch: 0.0, reverse: false }
    }
}

#[derive(Clone, Default)]
pub struct SampleLayer {
    pub path: Option<String>,
    pub sample: Option<Arc<Vec<f32>>>,
    pub controls: LayerControls,
    pub right: Option<Arc<Vec<f32>>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Envelope {
    pub enabled: bool,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Envelope {
    pub fn value(self, elapsed: f32, duration: f32) -> f32 {
        if !self.enabled { return 1.0; }
        if self.attack > 0.0 && elapsed < self.attack {
            return elapsed / self.attack;
        }
        let decay = ((elapsed - self.attack) / self.decay.max(0.0001)).clamp(0.0, 1.0);
        let level = 1.0 + (self.sustain - 1.0) * decay;
        let tail = if self.release > 0.0 { ((duration - elapsed) / self.release).clamp(0.0, 1.0) } else { 1.0 };
        level * tail
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Performance {
    pub muted: bool,
    pub solo: bool,
    pub velocity_gain: bool,
    pub source_level: bool,
    pub cut: u8,
    pub cut_by: u8,
    pub output: u8,
    pub color: Option<[u8; 3]>,
    pub layer_mode: LayerMode,
    pub volume_envelope: Envelope,
    pub pan_envelope: Envelope,
    pub edge_fades: EdgeFades,
}

impl Default for Performance {
    fn default() -> Self {
        let envelope = Envelope { enabled: false, attack: 0.0, decay: 0.1, sustain: 1.0, release: 0.02 };
        Self { muted: false, solo: false, velocity_gain: true, source_level: false, cut: 0, cut_by: 0, output: 0,
            color: None, layer_mode: LayerMode::Stack, volume_envelope: envelope, pan_envelope: envelope, edge_fades: EdgeFades::default() }
    }
}

/// A single drum pad in the kit.
#[derive(Clone)]
pub struct DrumPad {
    /// Audio sample data (mono, f32, at host sample rate)
    pub sample: Option<Arc<Vec<f32>>>,
    pub right: Option<Arc<Vec<f32>>>,
    /// Source file path
    pub sample_path: Option<String>,
    /// Display name
    pub name: String,
    /// Classification
    pub category: SampleCategory,
    /// Locked pads survive randomization
    pub locked: bool,
    /// MIDI note number (GM drum map: 36=C1 kick through 51=D#2)
    pub midi_note: u8,
    /// Volume (0.0 to 1.0)
    pub volume: f32,
    /// Pan (-1.0 left to 1.0 right)
    pub pan: f32,
    /// Pitch adjustment in semitones
    pub pitch: f32,
    /// Decay / sample length (0.0 = very short, 1.0 = full sample). Default 1.0.
    pub decay: f32,
    /// Start point (0.0 = beginning, 1.0 = end of sample). Default 0.0.
    pub start: f32,
    /// End point (0.0 = beginning, 1.0 = end of sample). Default 1.0.
    pub end: f32,
    pub primary: LayerControls,
    pub layers: Vec<SampleLayer>,
    pub performance: Performance,
}

impl DrumPad {
    pub fn new(index: usize) -> Self {
        Self {
            sample: None,
            right: None,
            sample_path: None,
            name: format!("Pad {}", index + 1),
            category: SampleCategory::Other,
            locked: false,
            midi_note: default_note(index),
            volume: 1.0,
            pan: 0.0,
            pitch: 0.0,
            decay: 1.0,
            start: 0.0,
            end: 1.0,
            primary: LayerControls::default(),
            layers: Vec::new(),
            performance: Performance::default(),
        }
    }

    pub fn layer(&self, index: usize) -> (Option<&Arc<Vec<f32>>>, LayerControls) {
        if index == 0 { (self.sample.as_ref(), self.primary) }
        else { let layer = &self.layers[index - 1]; (layer.sample.as_ref(), layer.controls) }
    }

    pub fn right_channel(&self, index: usize) -> Option<&Arc<Vec<f32>>> {
        if index == 0 { self.right.as_ref() } else { self.layers[index - 1].right.as_ref() }
    }

    pub fn spread_layers(&mut self) {
        let count = self.layers.len() + 1;
        for (index, controls) in std::iter::once(&mut self.primary)
            .chain(self.layers.iter_mut().map(|layer| &mut layer.controls)).enumerate()
        {
            controls.low = (1 + index * 127 / count) as u8;
            controls.high = ((index + 1) * 127 / count) as u8;
        }
    }

    pub fn swap_layers(&mut self, a: usize, b: usize) {
        let (a, b) = (a.min(b), a.max(b));
        if a == b || b > self.layers.len() { return; }
        if a == 0 {
            let layer = &mut self.layers[b - 1];
            std::mem::swap(&mut self.sample, &mut layer.sample);
            std::mem::swap(&mut self.right, &mut layer.right);
            std::mem::swap(&mut self.sample_path, &mut layer.path);
            std::mem::swap(&mut self.primary, &mut layer.controls);
        } else {
            self.layers.swap(a - 1, b - 1);
        }
    }
}

/// The drum kit.
#[derive(Clone)]
pub struct DrumKit {
    pub pads: Vec<DrumPad>,
}

impl DrumKit {
    pub fn new() -> Self {
        Self {
            pads: (0..NUM_PADS).map(DrumPad::new).collect(),
        }
    }

    /// Get pad index for a MIDI note, if mapped.
    pub fn pad_for_note(&self, note: u8) -> Option<usize> {
        self.pads.iter().position(|p| p.midi_note == note).or_else(|| {
            // A new Channel Rack step and the normal channel audition use middle C.
            note.checked_sub(60).map(usize::from).filter(|&index| index < LEGACY_PADS)
        })
    }

    /// Get the MIDI note number for a pad index.
    pub fn note_for_pad(&self, index: usize) -> u8 {
        self.pads.get(index).map(|p| p.midi_note).unwrap_or_else(|| default_note(index.min(NUM_PADS - 1)))
    }

    /// Capture the undoable state of all pads.
    pub fn snapshot(&self) -> Vec<PadSnapshot> {
        self.pads
            .iter()
            .map(|p| PadSnapshot {
                sample: p.sample.clone(),
                right: p.right.clone(),
                sample_path: p.sample_path.clone(),
                name: p.name.clone(),
                category: p.category,
                volume: p.volume,
                pan: p.pan,
                pitch: p.pitch,
                decay: p.decay,
                start: p.start,
                end: p.end,
                primary: p.primary,
                layers: p.layers.clone(),
                performance: p.performance,
                midi_note: Some(p.midi_note),
            })
            .collect()
    }

    /// Toggle lock on a pad. Locked pads survive randomization.
    /// This is NOT an undoable action.
    pub fn toggle_lock(&mut self, index: usize) {
        if index < self.pads.len() {
            self.pads[index].locked = !self.pads[index].locked;
        }
    }

    /// Restore pad state from a snapshot. Locks are independent of edit history.
    pub fn restore(&mut self, snapshot: &[PadSnapshot]) {
        for (pad, snap) in self.pads.iter_mut().zip(snapshot.iter()) {
            pad.sample = snap.sample.clone();
            pad.right = snap.right.clone();
            pad.sample_path = snap.sample_path.clone();
            pad.name = snap.name.clone();
            pad.category = snap.category;
            pad.volume = snap.volume;
            pad.pan = snap.pan;
            pad.pitch = snap.pitch;
            pad.decay = snap.decay;
            pad.start = snap.start;
            pad.end = snap.end;
            pad.primary = snap.primary;
            pad.layers = snap.layers.clone();
            pad.performance = snap.performance;
            if let Some(note) = snap.midi_note { pad.midi_note = note; }
        }
    }

    /// Assign an analyzed sample to a pad, updating audio data, path, name, and category.
    /// Preserves volume, pan, pitch, decay.
    fn assign_sample(pad: &mut DrumPad, sample: &crate::analysis::library::AnalyzedSample, used: &mut HashSet<String>) {
        let path = sample.entry.path.to_string_lossy().to_string();
        used.insert(path.clone());
        pad.sample = Some(Arc::clone(&sample.data));
        pad.right = None;
        pad.sample_path = Some(path);
        pad.name = sample.entry.filename.clone();
        pad.category = sample.entry.category;
    }

    /// Re-roll all unlocked pads from their current category.
    /// Preserves volume, pan, pitch. Avoids assigning the same sample to multiple pads.
    pub fn dice_all(&mut self, library: &SampleLibrary) {
        let mut used: HashSet<String> = self.pads.iter()
            .filter(|p| p.locked)
            .filter_map(|p| p.sample_path.clone())
            .collect();

        for pad in &mut self.pads {
            if pad.locked { continue; }
            if let Some(sample) = library.random_from_excluding(pad.category, &used) {
                Self::assign_sample(pad, &sample, &mut used);
            }
        }
    }

    /// Re-roll one specific pad. No-op if locked or out of range.
    /// Preserves volume, pan, pitch. Avoids paths already used by other pads.
    pub fn dice_pad(&mut self, index: usize, library: &SampleLibrary) {
        if index >= self.pads.len() || self.pads[index].locked { return; }
        let mut used: HashSet<String> = self.pads.iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .filter_map(|(_, p)| p.sample_path.clone())
            .collect();

        let category = self.pads[index].category;
        if let Some(sample) = library.random_from_excluding(category, &used) {
            Self::assign_sample(&mut self.pads[index], &sample, &mut used);
        }
    }

    /// Re-roll all unlocked pads of a given category.
    /// Preserves volume, pan, pitch. Avoids assigning the same sample to multiple pads.
    pub fn dice_category(&mut self, category: SampleCategory, library: &SampleLibrary) {
        let mut used: HashSet<String> = self.pads.iter()
            .filter(|p| p.locked || p.category != category)
            .filter_map(|p| p.sample_path.clone())
            .collect();

        for pad in &mut self.pads {
            if pad.locked || pad.category != category { continue; }
            if let Some(sample) = library.random_from_excluding(category, &used) {
                Self::assign_sample(pad, &sample, &mut used);
            }
        }
    }
}

impl Default for DrumKit {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::features::AudioFeatures;
    use crate::analysis::library::{AnalyzedSample, SampleLibrary};
    use crate::analysis::scanner::SampleEntry;
    use crate::util::history::PadSnapshot;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Build a minimal SampleLibrary with one sample per category.
    fn test_library() -> SampleLibrary {
        let mut by_category: HashMap<SampleCategory, Vec<AnalyzedSample>> = HashMap::new();

        for cat in SampleCategory::all() {
            let entry = SampleEntry {
                path: PathBuf::from(format!("/test/{}.wav", cat.label())),
                filename: format!("test-{}", cat.label()),
                category: *cat,
                folder_hint: None,
                duration_ms: 100,
                is_percussive: true,
            };
            let sample = AnalyzedSample {
                entry,
                features: AudioFeatures {
                    attack_time: 0.001,
                    decay_time: 0.05,
                    spectral_centroid: 1000.0,
                    spectral_flatness: 0.5,
                    sub_energy_ratio: 0.1,
                    high_freq_ratio: 0.1,
                    peak: 1.0,
                    duration: 0.1,
                    is_percussive: true,
                },
                data: Arc::new(vec![0.5; 4410]),
            };
            by_category.entry(*cat).or_default().push(sample);
        }

        SampleLibrary {
            total: 10,
            by_category,
            sample_rate: 44100.0,
        }
    }

    /// Build a library with N unique samples per category.
    fn test_library_multi(samples_per_cat: usize) -> SampleLibrary {
        let mut by_category: HashMap<SampleCategory, Vec<AnalyzedSample>> = HashMap::new();
        let mut total = 0;

        for cat in SampleCategory::all() {
            for i in 0..samples_per_cat {
                let entry = SampleEntry {
                    path: PathBuf::from(format!("/test/{}-{}.wav", cat.label(), i)),
                    filename: format!("test-{}-{}", cat.label(), i),
                    category: *cat,
                    folder_hint: None,
                    duration_ms: 100,
                    is_percussive: true,
                };
                let sample = AnalyzedSample {
                    entry,
                    features: AudioFeatures {
                        attack_time: 0.001,
                        decay_time: 0.05,
                        spectral_centroid: 1000.0,
                        spectral_flatness: 0.5,
                        sub_energy_ratio: 0.1,
                        high_freq_ratio: 0.1,
                        peak: 1.0,
                        duration: 0.1,
                        is_percussive: true,
                    },
                    data: Arc::new(vec![0.5 * i as f32 / samples_per_cat as f32; 4410]),
                };
                by_category.entry(*cat).or_default().push(sample);
                total += 1;
            }
        }

        SampleLibrary {
            total,
            by_category,
            sample_rate: 44100.0,
        }
    }

    #[test]
    fn all_pads_have_unique_notes_and_legacy_aliases_still_work() {
        let kit = DrumKit::new();
        for pad in 0..NUM_PADS {
            assert_eq!(kit.pad_for_note(default_note(pad)), Some(pad));
        }
        for pad in 0..LEGACY_PADS {
            assert_eq!(kit.pad_for_note(60 + pad as u8), Some(pad));
        }
        assert_eq!(kit.pad_for_note(76), None);
    }

    #[test]
    fn snapshot_captures_pad_state() {
        let mut kit = DrumKit::new();
        kit.pads[0].name = "MyKick".to_string();
        kit.pads[0].volume = 0.75;
        kit.pads[0].pan = -0.5;
        kit.pads[0].pitch = 2.0;
        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].sample = Some(Arc::new(vec![1.0; 100]));
        kit.pads[0].locked = true;

        let snap = kit.snapshot();
        assert_eq!(snap.len(), NUM_PADS);
        assert_eq!(snap[0].name, "MyKick");
        assert!((snap[0].volume - 0.75).abs() < 0.001);
        assert!((snap[0].pan - -0.5).abs() < 0.001);
        assert!((snap[0].pitch - 2.0).abs() < 0.001);
        assert_eq!(snap[0].category, SampleCategory::Kick);
        assert!(snap[0].sample.is_some());
    }

    #[test]
    fn restore_applies_snapshot_but_preserves_lock_and_midi() {
        let mut kit = DrumKit::new();
        kit.pads[0].locked = true;
        kit.pads[0].midi_note = 42;

        let snap: Vec<PadSnapshot> = (0..NUM_PADS)
            .map(|i| PadSnapshot {
                sample: None,
                right: None,
                sample_path: Some(format!("/path/{i}.wav")),
                name: format!("Restored-{i}"),
                category: SampleCategory::Snare,
                volume: 0.5,
                pan: 0.3,
                pitch: -1.0,
                decay: 1.0,
                start: 0.0,
                end: 1.0,
                primary: LayerControls::default(),
                layers: Vec::new(),
                performance: Performance::default(),
                midi_note: None,
            })
            .collect();

        kit.restore(&snap);

        assert_eq!(kit.pads[0].name, "Restored-0");
        assert!((kit.pads[0].volume - 0.5).abs() < 0.001);
        assert_eq!(kit.pads[0].category, SampleCategory::Snare);
        // locked and midi_note should be untouched
        assert!(kit.pads[0].locked);
        assert_eq!(kit.pads[0].midi_note, 42);
    }

    #[test]
    fn toggle_lock_flips_flag() {
        let mut kit = DrumKit::new();
        assert!(!kit.pads[0].locked);

        kit.toggle_lock(0);
        assert!(kit.pads[0].locked);

        kit.toggle_lock(0);
        assert!(!kit.pads[0].locked);
    }

    #[test]
    fn dice_all_changes_unlocked_pads() {
        let mut kit = DrumKit::new();
        let lib = test_library();

        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].name = "original".to_string();

        kit.dice_all(&lib);

        assert_ne!(kit.pads[0].name, "original");
    }

    #[test]
    fn dice_all_skips_locked_pads() {
        let mut kit = DrumKit::new();
        let lib = test_library();

        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].name = "locked-kick".to_string();
        kit.pads[0].locked = true;

        kit.dice_all(&lib);

        assert_eq!(kit.pads[0].name, "locked-kick");
    }

    #[test]
    fn dice_all_preserves_volume_pan_pitch() {
        let mut kit = DrumKit::new();
        let lib = test_library();

        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].volume = 0.42;
        kit.pads[0].pan = -0.7;
        kit.pads[0].pitch = 3.5;

        kit.dice_all(&lib);

        assert!((kit.pads[0].volume - 0.42).abs() < 0.001);
        assert!((kit.pads[0].pan - -0.7).abs() < 0.001);
        assert!((kit.pads[0].pitch - 3.5).abs() < 0.001);
    }

    #[test]
    fn dice_pad_changes_specific_pad_only() {
        let mut kit = DrumKit::new();
        let lib = test_library();

        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].name = "original-0".to_string();
        kit.pads[1].category = SampleCategory::Snare;
        kit.pads[1].name = "original-1".to_string();

        kit.dice_pad(0, &lib);

        assert_ne!(kit.pads[0].name, "original-0");
        assert_eq!(kit.pads[1].name, "original-1");
    }

    #[test]
    fn dice_pad_locked_is_noop() {
        let mut kit = DrumKit::new();
        let lib = test_library();

        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].name = "locked".to_string();
        kit.pads[0].locked = true;

        kit.dice_pad(0, &lib);

        assert_eq!(kit.pads[0].name, "locked");
    }

    #[test]
    fn dice_category_only_affects_matching_unlocked_pads() {
        let mut kit = DrumKit::new();
        let lib = test_library();

        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].name = "kick-0".to_string();
        kit.pads[1].category = SampleCategory::Kick;
        kit.pads[1].name = "kick-1".to_string();
        kit.pads[1].locked = true;
        kit.pads[2].category = SampleCategory::Snare;
        kit.pads[2].name = "snare-2".to_string();

        kit.dice_category(SampleCategory::Kick, &lib);

        // Pad 0 (kick, unlocked) should change
        assert_ne!(kit.pads[0].name, "kick-0");
        // Pad 1 (kick, locked) should NOT change
        assert_eq!(kit.pads[1].name, "kick-1");
        // Pad 2 (snare) should NOT change
        assert_eq!(kit.pads[2].name, "snare-2");
    }

    #[test]
    fn dice_all_no_duplicate_samples() {
        // Library with enough unique samples per category to fill the kit without repeats
        let lib = test_library_multi(NUM_PADS);
        let mut kit = DrumKit::new();

        // Assign categories matching the default 8-pad layout
        let layout = [
            SampleCategory::Kick,
            SampleCategory::Snare,
            SampleCategory::Hihat,
            SampleCategory::Clap,
            SampleCategory::Perc,
            SampleCategory::Tom,
            SampleCategory::Cymbal,
            SampleCategory::Synth,
        ];
        for (pad, cat) in kit.pads.iter_mut().zip(layout.iter()) {
            pad.category = *cat;
        }

        kit.dice_all(&lib);

        // Collect all assigned paths
        let paths: Vec<String> = kit
            .pads
            .iter()
            .filter_map(|p| p.sample_path.clone())
            .collect();

        // All pads should have a sample
        assert_eq!(paths.len(), NUM_PADS, "all pads should have a sample");

        // No two pads should share the same path
        let unique: std::collections::HashSet<&String> = paths.iter().collect();
        assert_eq!(unique.len(), paths.len(), "duplicate samples found across pads");
    }

    #[test]
    fn dice_category_no_duplicate_samples() {
        // 3 unique kick samples, 2 kick pads — should be different
        let lib = test_library_multi(3);
        let mut kit = DrumKit::new();

        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[1].category = SampleCategory::Kick;

        kit.dice_category(SampleCategory::Kick, &lib);

        let path0 = kit.pads[0].sample_path.as_deref().unwrap_or("");
        let path1 = kit.pads[1].sample_path.as_deref().unwrap_or("");

        assert!(!path0.is_empty(), "pad 0 should have a sample");
        assert!(!path1.is_empty(), "pad 1 should have a sample");
        assert_ne!(path0, path1, "pad 0 and pad 1 should not share a sample");
    }

    #[test]
    fn dice_pad_no_duplicate_with_other_pads() {
        // Set pad 0 and pad 1 to the same kick, then dice pad 1 — should pick a different one
        let lib = test_library_multi(2);
        let mut kit = DrumKit::new();

        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].sample_path = Some("/test/KICK-0.wav".to_string());
        kit.pads[1].category = SampleCategory::Kick;
        kit.pads[1].sample_path = Some("/test/KICK-0.wav".to_string());

        kit.dice_pad(1, &lib);

        let path0 = kit.pads[0].sample_path.as_deref().unwrap_or("");
        let path1 = kit.pads[1].sample_path.as_deref().unwrap_or("");
        assert_ne!(path0, path1, "dice_pad should not reuse pad 0's sample");
    }
}
