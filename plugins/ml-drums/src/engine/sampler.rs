use crate::engine::trim::{FadeFrames, Trim};
use std::sync::Arc;

use nih_plug::util::permit_alloc;

use crate::engine::kit::{DrumKit, Envelope, LayerMode, MAX_LAYERS, NUM_PADS};

/// Number of simultaneous voices.
const MAX_VOICES: usize = 64;

/// Fade-out duration in seconds when re-triggering the same pad.
const RETRIGGER_FADE_SECS: f32 = 0.05; // 50ms

/// A single playback voice.
struct Voice {
    /// Which pad this voice is playing, or None if inactive.
    pad_index: Option<usize>,
    /// Sample data reference (shared with DrumPad).
    sample: Option<Arc<Vec<f32>>>,
    right: Option<Arc<Vec<f32>>>,
    /// Current playback position in source samples (fractional for pitch shifting).
    position: f64,
    /// Playback rate multiplier: 2^(pitch_semitones/12). 1.0 = original pitch.
    rate: f64,
    note_rate: f64,
    source_start: f64,
    source_end: f64,
    edge_fades: FadeFrames,
    cut_by: u8,
    output: usize,
    source_level: bool,
    fade_delay: usize,
    volume_envelope: Envelope,
    pan_envelope: Envelope,
    duration: f32,
    sample_rate: f32,
    /// Velocity gain (0.0–1.0).
    velocity: f32,
    /// Monotonic counter set at trigger time — used for oldest-voice stealing.
    age: u64,
    /// If > 0, voice is fading out. Counts down in samples.
    fade_remaining: usize,
    /// Total fade length in samples (set once at fade start).
    fade_length: usize,
    /// Samples to skip at buffer start (for sample-accurate sequencer triggers).
    start_offset: usize,
    /// Output samples rendered so far (for decay cutoff tracking).
    samples_rendered: usize,
    /// Effective pan value, resolved at trigger time (pad pan or p-lock override).
    pan: f32,
    /// Resolved reverb send (0..1). Step plock overrides pad default at trigger time.
    fx_rvb_send: f32,
    /// Resolved delay send (0..1). Step plock overrides pad default at trigger time.
    fx_dly_send: f32,
    /// Resolved filter routing. Step plock overrides pad default at trigger time.
    fx_to_filter: bool,
}

impl Voice {
    fn new() -> Self {
        Self {
            pad_index: None,
            sample: None,
            right: None,
            position: 0.0,
            rate: 1.0,
            note_rate: 1.0,
            source_start: 0.0,
            source_end: 0.0,
            edge_fades: FadeFrames::default(),
            cut_by: 0,
            output: 0,
            source_level: false,
            fade_delay: 0,
            volume_envelope: Envelope::default(),
            pan_envelope: Envelope::default(),
            duration: 0.0,
            sample_rate: 48000.0,
            velocity: 0.0,
            age: 0,
            fade_remaining: 0,
            fade_length: 0,
            start_offset: 0,
            samples_rendered: 0,
            pan: 0.0,
            fx_rvb_send: 0.0,
            fx_dly_send: 0.0,
            fx_to_filter: false,
        }
    }

    fn is_active(&self) -> bool {
        self.pad_index.is_some()
    }

    fn deactivate(&mut self) {
        self.pad_index = None;
        // Drop the Arc outside the hot loop — permit_alloc wraps this
        // since Arc::drop can deallocate
        permit_alloc(|| {
            self.sample = None;
            self.right = None;
        });
    }

    fn start_fade_out(&mut self, fade_samples: usize) {
        if self.fade_remaining == 0 && self.is_active() {
            self.fade_remaining = fade_samples;
            self.fade_length = fade_samples;
        }
    }

    /// Render one sample. Returns (left, right) or None if voice is done.
    fn next_sample(&mut self, pan: f32) -> Option<(f32, f32)> {
        if !self.is_active() {
            return None;
        }

        let data = match &self.sample {
            Some(s) => s,
            None => {
                self.pad_index = None;
                return None;
            }
        };

        // Need at least one sample to read (pos_floor must be a valid index)
        let pos_floor = self.position as usize;
        if pos_floor >= data.len() || self.position < self.source_start || self.position >= self.source_end {
            self.pad_index = None;
            return None;
        }

        // Linear interpolation between adjacent samples
        let frac = (self.position - pos_floor as f64) as f32;
        let interpolate = |samples: &[f32]| {
            let a = samples.get(pos_floor).copied().unwrap_or(0.0);
            let b = samples.get(pos_floor + 1).copied().unwrap_or(0.0);
            a + (b - a) * frac
        };
        let stereo = self.right.as_ref().map(|samples| interpolate(samples));
        let elapsed = self.samples_rendered as f32 / self.sample_rate;
        let step = self.rate * self.note_rate;
        let remaining = if !self.edge_fades.has_release() {
            f64::INFINITY
        } else if step < 0.0 {
            ((self.position - self.source_start) / -step).floor()
        } else {
            ((self.source_end - self.position) / step).ceil() - 1.0
        };
        let mut gain = self.velocity * self.volume_envelope.value(elapsed, self.duration)
            * self.edge_fades.gain(self.samples_rendered, remaining);
        let left = interpolate(data);
        self.position += step;
        self.samples_rendered += 1;

        // Choke changes take effect at the MIDI event's position in this block.
        if self.fade_delay > 0 {
            self.fade_delay -= 1;
        } else if self.fade_remaining > 0 {
            let fade_gain = self.fade_remaining as f32 / self.fade_length as f32;
            gain *= fade_gain;
            self.fade_remaining -= 1;
            if self.fade_remaining == 0 {
                self.pad_index = None;
                return Some((0.0, 0.0));
            }
        }

        // Constant-power pan: pan in [-1, 1], center = 0
        let pan = if self.pan_envelope.enabled {
            (pan + 2.0 * self.pan_envelope.value(elapsed, self.duration) - 1.0).clamp(-1.0, 1.0)
        } else { pan };
        let angle = (pan + 1.0) * 0.25 * std::f32::consts::PI; // 0..PI/2
        let pan_l = angle.cos();
        let pan_r = angle.sin();
        Some(if let Some(right) = stereo {
            let root_two = std::f32::consts::SQRT_2;
            (left * gain * (pan_l * root_two).min(1.0), right * gain * (pan_r * root_two).min(1.0))
        } else {
            (left * gain * pan_l, left * gain * pan_r)
        })
    }
}

/// Pool of voices that mixes triggered drum samples into stereo output.
pub struct VoicePool {
    voices: Vec<Voice>,
    /// Monotonic counter for age-based voice stealing.
    trigger_counter: u64,
    /// Fade-out length in samples (computed from sample rate).
    fade_samples: usize,
    sample_rate: f32,
    last_layers: [u8; NUM_PADS],
    random: u32,
}

impl VoicePool {
    pub fn clear(&mut self) {
        for voice in &mut self.voices { *voice = Voice::new(); }
        self.trigger_counter = 0;
        self.last_layers.fill(u8::MAX);
    }

    pub fn new(sample_rate: f32) -> Self {
        let fade_samples = (RETRIGGER_FADE_SECS * sample_rate) as usize;
        tracing::info!(MAX_VOICES, fade_samples, "VoicePool created");
        Self {
            voices: (0..MAX_VOICES).map(|_| Voice::new()).collect(),
            trigger_counter: 0,
            fade_samples,
            sample_rate,
            last_layers: [u8::MAX; NUM_PADS],
            random: 0x734ad025,
        }
    }

    /// Trigger a pad. Fades out any existing voices on the same pad,
    /// then allocates a new voice.
    /// Called from audio thread — all allocating ops wrapped in permit_alloc.
    pub fn trigger(
        &mut self,
        pad_index: usize,
        velocity: f32,
        kit: &DrumKit,
        start_offset: usize,
        pan_override: Option<f32>,
        pitch_override: Option<f32>,
        rvb_override: Option<f32>,
        dly_override: Option<f32>,
        filter_override: Option<bool>,
        lane_rvb: f32,
        lane_dly: f32,
        lane_filter: bool,
    ) {
        let pad = &kit.pads[pad_index];
        let p = pad.performance;
        if p.muted || (!p.solo && kit.pads.iter().any(|pad| pad.performance.solo)) { return; }
        let midi_velocity = (velocity * 127.0).round().clamp(1.0, 127.0) as u8;
        let mut matches = [0; MAX_LAYERS];
        let mut count = 0;
        for index in 0..(1 + pad.layers.len()).min(MAX_LAYERS) {
            let (sample, settings) = pad.layer(index);
            if sample.is_some_and(|sample| !sample.is_empty()) && (settings.low..=settings.high).contains(&midi_velocity) {
                matches[count] = index;
                count += 1;
            }
        }
        if count == 0 { return; }
        let mut chosen = &matches[..count];
        if p.layer_mode != LayerMode::Stack {
            let previous = chosen.iter().position(|&i| i == usize::from(self.last_layers[pad_index]));
            let offset = match p.layer_mode {
                LayerMode::Cycle => previous.map_or(0, |p| (p + 1) % count),
                LayerMode::RandomNoRepeat if count > 1 && previous.is_some() => {
                    let previous = previous.unwrap();
                    let random = self.random_index(count - 1);
                    random + usize::from(random >= previous)
                }
                _ => self.random_index(count),
            };
            self.last_layers[pad_index] = chosen[offset] as u8;
            chosen = &chosen[offset..offset+1];
        }
        for voice in self.voices.iter_mut().filter(|voice| voice.is_active() && voice.fade_remaining == 0) {
            let same_pad = voice.pad_index == Some(pad_index);
            let choke = p.cut != 0 && p.cut == voice.cut_by;
            if choke || (same_pad && p.cut_by == 0) {
                let fade = if choke { (self.sample_rate * 0.003) as usize } else { self.fade_samples };
                voice.start_fade_out(fade.max(1));
                voice.fade_delay = start_offset.saturating_sub(voice.start_offset);
            }
        }
        for &index in chosen {
            let (sample, layer) = pad.layer(index);
            let sample = sample.expect("matched layer has audio");
            let slot = self.find_free_or_steal();
            self.trigger_counter += 1;
            let pitch = (pitch_override.unwrap_or(pad.pitch) + layer.pitch).clamp(-96.0, 96.0);
            let rate = 2.0_f64.powf(pitch as f64 / 12.0);
            let voice = &mut self.voices[slot];
            voice.pad_index = Some(pad_index);
            permit_alloc(|| {
                voice.sample = Some(Arc::clone(sample));
                voice.right = pad.right_channel(index).cloned();
            });

            let trim = Trim::new(pad, sample.len(), layer.reverse, self.sample_rate, rate);
            voice.source_start = trim.start as f64;
            voice.source_end = trim.end as f64;
            voice.position = trim.index(0) as f64;
            voice.edge_fades = trim.fades;
            voice.rate = if layer.reverse { -rate } else { rate };
            voice.note_rate = 1.0;
            voice.velocity = (if p.velocity_gain { velocity } else { 1.0 }) * pad.volume * layer.gain;
            voice.age = self.trigger_counter;
            voice.cut_by = p.cut_by;
            voice.output = usize::from(p.output);
            voice.source_level = p.source_level;
            voice.fade_remaining = 0;
            voice.fade_delay = 0;
            voice.fade_length = self.fade_samples;
            voice.start_offset = start_offset;
            voice.samples_rendered = 0;
            voice.pan = (pan_override.unwrap_or(pad.pan) + layer.pan).clamp(-1.0, 1.0);
            voice.fx_rvb_send = rvb_override.unwrap_or(lane_rvb).clamp(0.0, 1.0);
            voice.fx_dly_send = dly_override.unwrap_or(lane_dly).clamp(0.0, 1.0);
            voice.fx_to_filter = filter_override.unwrap_or(lane_filter);
            voice.volume_envelope = p.volume_envelope;
            voice.pan_envelope = p.pan_envelope;
            voice.sample_rate = self.sample_rate;
            voice.duration = (trim.frames() as f64 / rate) as f32 / self.sample_rate;
        }
    }

    fn random_index(&mut self, count: usize) -> usize {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        self.random as usize % count
    }

    fn find_free_or_steal(&mut self) -> usize {
        // Prefer an inactive voice
        if let Some(i) = self.voices.iter().position(|v| !v.is_active()) {
            return i;
        }

        // Steal oldest (lowest age counter)
        let (oldest_idx, _) = self
            .voices
            .iter()
            .enumerate()
            .min_by_key(|(_, v)| v.age)
            .unwrap(); // safe: MAX_VOICES > 0

        // Deactivate stolen voice (drops its Arc, which may deallocate)
        self.voices[oldest_idx].deactivate();
        oldest_idx
    }

    /// Mix all active voices into the output buffer.
    /// Pan values are resolved at trigger time — no kit reference needed.
    pub fn process(&mut self, output_left: &mut [f32], output_right: &mut [f32]) {
        for voice in self.voices.iter_mut() {
            if !voice.is_active() {
                continue;
            }

            let pan = voice.pan;

            for (i, (l, r)) in output_left.iter_mut().zip(output_right.iter_mut()).enumerate() {
                if i < voice.start_offset {
                    continue;
                }
                match voice.next_sample(pan) {
                    Some((vl, vr)) => {
                        *l += vl;
                        *r += vr;
                    }
                    None => break,
                }
            }

            // Reset offset so next buffer plays from sample 0
            voice.start_offset = 0;

            // Clean up finished voices
            if !voice.is_active() {
                voice.deactivate();
            }
        }
    }

    /// Mix voices into four parallel buses:
    ///   - `dry_bypass_*`: direct (unfiltered) dry — voices with `fx_to_filter == false`.
    ///   - `dry_filter_*`: direct dry destined for the master DJ filter insert —
    ///     voices with `fx_to_filter == true`.
    ///   - `send_rvb_*`: reverb send — each voice's direct output scaled by
    ///     `voice.fx_rvb_send` and summed.
    ///   - `send_dly_*`: delay send — same for `voice.fx_dly_send`.
    ///
    /// Every voice carries its own resolved sends (pad default, optionally
    /// overridden by a step plock), so per-hit FX routing works automatically.
    ///
    /// Zero-alloc: the caller pre-allocates all eight buffers.
    pub fn process_sends(
        &mut self,
        dry_bypass_l: &mut [f32],
        dry_bypass_r: &mut [f32],
        dry_filter_l: &mut [f32],
        dry_filter_r: &mut [f32],
        send_rvb_l: &mut [f32],
        send_rvb_r: &mut [f32],
        send_dly_l: &mut [f32],
        send_dly_r: &mut [f32],
        source_l: &mut [f32],
        source_r: &mut [f32],
        note_tuning: &[(usize, usize, f64)],
        auxiliary: &mut [nih_plug::prelude::Buffer<'_>],
    ) {
        let num_samples = dry_bypass_l.len();
        for voice in self.voices.iter_mut() {
            if !voice.is_active() {
                continue;
            }
            let pan = voice.pan;
            let rvb_g = voice.fx_rvb_send;
            let dly_g = voice.fx_dly_send;
            let to_filter = voice.fx_to_filter;

            let pad_index = voice.pad_index;
            let start = voice.start_offset;
            let mut changes = note_tuning.iter().copied().filter(|&(frame, pad, _)| {
                Some(pad) == pad_index && frame >= start
            }).peekable();
            for i in voice.start_offset..num_samples {
                while changes.peek().is_some_and(|&(frame, _, _)| frame <= i) {
                    voice.note_rate = changes.next().unwrap().2;
                }
                match voice.next_sample(pan) {
                    Some((vl, vr)) => {
                        if let Some(port) = voice.output.checked_sub(1).and_then(|output| auxiliary.get_mut(output)) {
                            let channels = port.as_slice();
                            channels[0][i] += vl;
                            channels[1][i] += vr;
                        } else if to_filter {
                            dry_filter_l[i] += vl;
                            dry_filter_r[i] += vr;
                        } else if voice.source_level {
                            source_l[i] += vl;
                            source_r[i] += vr;
                        } else {
                            dry_bypass_l[i] += vl;
                            dry_bypass_r[i] += vr;
                        }
                        if rvb_g > 0.0 {
                            send_rvb_l[i] += vl * rvb_g;
                            send_rvb_r[i] += vr * rvb_g;
                        }
                        if dly_g > 0.0 {
                            send_dly_l[i] += vl * dly_g;
                            send_dly_r[i] += vr * dly_g;
                        }
                    }
                    None => break,
                }
            }

            voice.start_offset = 0;
            if !voice.is_active() {
                voice.deactivate();
            }
        }
    }

    /// Number of currently active voices (for debug display).
    pub fn active_count(&self) -> usize {
        self.voices.iter().filter(|v| v.is_active()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::kit::{DrumKit, SampleCategory};

    /// Build a minimal kit with one pad loaded with a known sample.
    fn test_kit() -> DrumKit {
        let mut kit = DrumKit::new();
        // 8 samples of 1.0 — easy to verify in output
        kit.pads[0].sample = Some(Arc::new(vec![1.0; 8]));
        kit.pads[0].volume = 1.0;
        kit.pads[0].pan = 0.0;
        kit.pads[0].category = SampleCategory::Kick;
        kit
    }

    #[test]
    fn velocity_range_selects_a_stereo_layer_and_respects_reverse() {
        let mut kit = test_kit();
        let pad = &mut kit.pads[0];
        pad.performance.velocity_gain = false;
        pad.primary.high = 79;
        pad.layers.push(crate::engine::kit::SampleLayer {
            path: None,
            sample: Some(Arc::new(vec![0.1, 0.2, 0.3, 0.4])),
            right: Some(Arc::new(vec![0.9, 0.8, 0.7, 0.6])),
            controls: crate::engine::kit::LayerControls { low: 80, reverse: true, ..Default::default() },
        });
        let render = |velocity| {
            let mut voices = VoicePool::new(48_000.0);
            voices.trigger(0, velocity, &kit, 0, None, None, None, None, None, 0.0, 0.0, false);
            let (mut l, mut r) = ([0.0; 2], [0.0; 2]);
            voices.process(&mut l, &mut r);
            (l, r)
        };
        let (left, right) = render(0.5);
        assert_eq!(left, right);
        assert!(left[0] > 0.7);
        let (left, right) = render(1.0);
        assert!((left[0] - 0.4).abs() < 1e-6 && (left[1] - 0.3).abs() < 1e-6);
        assert!((right[0] - 0.6).abs() < 1e-6 && (right[1] - 0.7).abs() < 1e-6);
    }

    #[test]
    fn trigger_with_zero_offset_plays_from_start() {
        let kit = test_kit();
        let mut pool = VoicePool::new(44100.0);
        pool.trigger(0, 1.0, &kit, 0, None, None, None, None, None, 0.0, 0.0, false);

        let mut left = vec![0.0f32; 8];
        let mut right = vec![0.0f32; 8];
        pool.process(&mut left, &mut right);

        // Pan center: constant-power pan gives cos(PI/4) ≈ 0.7071
        let expected = (0.25 * std::f32::consts::PI).cos();
        assert!((left[0] - expected).abs() < 0.001, "first sample should be non-zero");
        assert!((left[7] - expected).abs() < 0.001, "last sample should be non-zero");
    }

    #[test]
    fn trigger_with_offset_delays_playback() {
        let kit = test_kit();
        let mut pool = VoicePool::new(44100.0);
        pool.trigger(0, 1.0, &kit, 4, None, None, None, None, None, 0.0, 0.0, false); // start at sample 4

        let mut left = vec![0.0f32; 12];
        let mut right = vec![0.0f32; 12];
        pool.process(&mut left, &mut right);

        // Samples 0..4 should be silent
        for i in 0..4 {
            assert_eq!(left[i], 0.0, "sample {i} should be silent (before offset)");
        }

        // Samples 4..12 should have audio (8 samples of the pad)
        let expected = (0.25 * std::f32::consts::PI).cos();
        assert!((left[4] - expected).abs() < 0.001, "sample 4 should have audio");
    }

    #[test]
    fn start_offset_resets_after_first_buffer() {
        let mut pool = VoicePool::new(44100.0);
        let mut kit = DrumKit::new();
        kit.pads[0].sample = Some(Arc::new(vec![1.0; 100]));
        kit.pads[0].volume = 1.0;
        kit.pads[0].pan = 0.0;
        kit.pads[0].category = SampleCategory::Kick;

        pool.trigger(0, 1.0, &kit, 4, None, None, None, None, None, 0.0, 0.0, false);

        // First buffer: 8 samples, offset should apply (0..4 silent)
        let mut left1 = vec![0.0f32; 8];
        let mut right1 = vec![0.0f32; 8];
        pool.process(&mut left1, &mut right1);
        assert_eq!(left1[0], 0.0, "first buffer: sample 0 should be silent");

        // Second buffer: offset should be reset, audio starts at sample 0
        let mut left2 = vec![0.0f32; 8];
        let mut right2 = vec![0.0f32; 8];
        pool.process(&mut left2, &mut right2);
        let expected = (0.25 * std::f32::consts::PI).cos();
        assert!((left2[0] - expected).abs() < 0.001, "second buffer: sample 0 should have audio (offset reset)");
    }

    #[test]
    fn velocity_scales_output_amplitude() {
        let kit = test_kit();
        let pan_gain = (0.25 * std::f32::consts::PI).cos(); // ~0.7071 for center pan

        // Full velocity
        let mut pool = VoicePool::new(44100.0);
        pool.trigger(0, 1.0, &kit, 0, None, None, None, None, None, 0.0, 0.0, false);
        let mut left_full = vec![0.0f32; 4];
        let mut right_full = vec![0.0f32; 4];
        pool.process(&mut left_full, &mut right_full);

        // Half velocity
        let mut pool = VoicePool::new(44100.0);
        pool.trigger(0, 0.5, &kit, 0, None, None, None, None, None, 0.0, 0.0, false);
        let mut left_half = vec![0.0f32; 4];
        let mut right_half = vec![0.0f32; 4];
        pool.process(&mut left_half, &mut right_half);

        // Zero velocity
        let mut pool = VoicePool::new(44100.0);
        pool.trigger(0, 0.0, &kit, 0, None, None, None, None, None, 0.0, 0.0, false);
        let mut left_zero = vec![0.0f32; 4];
        let mut right_zero = vec![0.0f32; 4];
        pool.process(&mut left_zero, &mut right_zero);

        // Full velocity should give pan_gain (sample=1.0 * vel=1.0 * pan)
        assert!((left_full[0] - pan_gain).abs() < 0.001,
            "full velocity: expected {pan_gain}, got {}", left_full[0]);

        // Half velocity should give half of full
        assert!((left_half[0] - pan_gain * 0.5).abs() < 0.001,
            "half velocity: expected {}, got {}", pan_gain * 0.5, left_half[0]);

        // Zero velocity should be silent
        assert_eq!(left_zero[0], 0.0, "zero velocity should be silent");
    }

    #[test]
    fn trimmed_and_shortened_hits_fade_to_zero_without_changing_the_middle() {
        let mut kit = DrumKit::new();
        let pad = &mut kit.pads[0];
        pad.sample = Some(Arc::new(vec![0.8; 1024]));
        pad.right = Some(Arc::new(vec![-0.4; 1024]));
        pad.start = 0.25;
        pad.end = 0.75;
        let render = |kit: &DrumKit| {
            let mut voice = VoicePool::new(48_000.0);
            voice.trigger(0, 1.0, kit, 0, None, None, None, None, None, 0.0, 0.0, false);
            let (mut left, mut right) = ([0.0; 520], [0.0; 520]);
            voice.process(&mut left, &mut right);
            (left, right)
        };
        let (left, right) = render(&kit);
        assert_eq!((left[0], left[511], right[0], right[511]), (0.0, 0.0, 0.0, 0.0));
        assert!(left[1] > 0.0 && left[1] < left[24] && left[24] < left[48]);
        assert!((left[48] - 0.8).abs() < 1e-6 && (right[48] + 0.4).abs() < 1e-6);
        assert!(left[510] < left[488] && left[488] < left[460]);
        assert!(left[512..].iter().all(|&sample| sample == 0.0));

        let pad = &mut kit.pads[0];
        pad.primary.reverse = true;
        pad.decay = 0.5;
        pad.performance.edge_fades.start_ms = Some(0.0);
        pad.performance.edge_fades.end_ms = Some(2.0);
        let (left, _) = render(&kit);
        assert!((left[0] - 0.8).abs() < 1e-6);
        assert!((left[159] - 0.8).abs() < 1e-6 && (left[207] - 0.4).abs() < 1e-6);
        assert_eq!(left[255], 0.0);
        assert!(left[256..].iter().all(|&sample| sample == 0.0));
    }

    #[test]
    fn start_point_offsets_playback_into_sample() {
        // Sample: [0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.25]
        let mut kit = DrumKit::new();
        kit.pads[0].sample = Some(Arc::new(vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.25]));
        kit.pads[0].volume = 1.0;
        kit.pads[0].pan = 0.0;
        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].start = 0.5; // Start halfway = sample index 4
        kit.pads[0].performance.edge_fades.start_ms = Some(0.0);

        let mut pool = VoicePool::new(44100.0);
        pool.trigger(0, 1.0, &kit, 0, None, None, None, None, None, 0.0, 0.0, false);

        let mut left = vec![0.0f32; 4];
        let mut right = vec![0.0f32; 4];
        pool.process(&mut left, &mut right);

        let pan_gain = (0.25 * std::f32::consts::PI).cos();
        // First output sample should come from index 4 (value 1.0)
        assert!((left[0] - 1.0 * pan_gain).abs() < 0.01,
            "start=0.5 should play from mid-sample: expected {}, got {}", 1.0 * pan_gain, left[0]);
    }

    #[test]
    fn end_point_stops_playback_early() {
        // 100 samples of 1.0
        let mut kit = DrumKit::new();
        kit.pads[0].sample = Some(Arc::new(vec![1.0; 100]));
        kit.pads[0].volume = 1.0;
        kit.pads[0].pan = 0.0;
        kit.pads[0].category = SampleCategory::Kick;
        kit.pads[0].end = 0.1; // End at 10% = 10 source samples

        let mut pool = VoicePool::new(44100.0);
        pool.trigger(0, 1.0, &kit, 0, None, None, None, None, None, 0.0, 0.0, false);

        let mut left = vec![0.0f32; 20];
        let mut right = vec![0.0f32; 20];
        pool.process(&mut left, &mut right);

        let pan_gain = (0.25 * std::f32::consts::PI).cos();
        // First sample should have audio
        assert!((left[0] - pan_gain).abs() < 0.01, "should have audio at start");
        // Well past the end point, should be silent
        assert!((left[15]).abs() < 0.01,
            "should be silent past end point, got {}", left[15]);
    }
}
