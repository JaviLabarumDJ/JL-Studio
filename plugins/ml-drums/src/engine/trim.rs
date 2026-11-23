// SPDX-License-Identifier: GPL-3.0-or-later
use serde::{Deserialize, Serialize};

use super::kit::DrumPad;

const AUTO_FADE_MS: f32 = 1.0;
pub const MAX_FADE_MS: f32 = 50.0;

/// Missing values follow the automatic de-click rule for a trimmed edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgeFades {
    pub start_ms: Option<f32>,
    pub end_ms: Option<f32>,
}

impl EdgeFades {
    pub fn automatic(start: f32, end: f32, length: f32, reverse: bool) -> (f32, f32) {
        let (start_cut, end_cut) = if reverse {
            (end < 1.0, start > 0.0)
        } else {
            (start > 0.0, end < 1.0)
        };
        (
            if start_cut { AUTO_FADE_MS } else { 0.0 },
            if end_cut || length < 1.0 {
                AUTO_FADE_MS
            } else {
                0.0
            },
        )
    }

    pub fn effective(self, automatic: (f32, f32)) -> (f32, f32) {
        (
            self.start_ms.unwrap_or(automatic.0).clamp(0.0, MAX_FADE_MS),
            self.end_ms.unwrap_or(automatic.1).clamp(0.0, MAX_FADE_MS),
        )
    }
}

#[derive(Clone, Copy)]
pub struct Trim {
    pub start: usize,
    pub end: usize,
    pub reverse: bool,
    pub fades: FadeFrames,
}

impl Trim {
    /// Length is counted from the edge playback starts at, including in reverse.
    pub fn new(pad: &DrumPad, len: usize, reverse: bool, sample_rate: f32, speed: f64) -> Self {
        debug_assert!(len > 0);
        let mut start =
            ((f64::from(pad.start.clamp(0.0, 1.0)) * len as f64).floor() as usize).min(len - 1);
        let mut end = ((f64::from(pad.end.clamp(0.0, 1.0)) * len as f64).ceil() as usize)
            .clamp(start + 1, len);
        let length =
            (((end - start) as f64 * f64::from(pad.decay.clamp(0.0, 1.0))).ceil() as usize).max(1);
        if reverse {
            start = end - length;
        } else {
            end = start + length;
        }
        let count = if reverse {
            ((end - start - 1) as f64 / speed).floor() as usize + 1
        } else {
            (((end - start) as f64 / speed).ceil() as usize).max(1)
        };
        let maximum = count.saturating_sub(1) / 2;
        let (in_ms, out_ms) = pad
            .performance
            .edge_fades
            .effective(EdgeFades::automatic(pad.start, pad.end, pad.decay, reverse));
        let duration = |ms: f32| ((ms * sample_rate / 1000.0).round() as usize).min(maximum);
        Self {
            start,
            end,
            reverse,
            fades: FadeFrames {
                attack: duration(in_ms),
                release: duration(out_ms),
            },
        }
    }

    pub fn frames(self) -> usize {
        self.end - self.start
    }

    pub fn index(self, frame: usize) -> usize {
        if self.reverse {
            self.end - 1 - frame
        } else {
            self.start + frame
        }
    }

    pub fn gain(self, frame: usize) -> f32 {
        self.fades.gain(frame, (self.frames() - 1 - frame) as f64)
    }
}

/// Durations in output frames (one gain shared by both channels).
#[derive(Clone, Copy, Default)]
pub struct FadeFrames {
    attack: usize,
    release: usize,
}

impl FadeFrames {
    pub fn gain(self, elapsed: usize, remaining: f64) -> f32 {
        let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
        let mut gain = 1.0;
        if elapsed < self.attack {
            gain *= smooth(elapsed as f32 / self.attack as f32);
        }
        if remaining < self.release as f64 {
            gain *= smooth(remaining.max(0.0) as f32 / self.release as f32);
        }
        gain
    }

    pub fn has_release(self) -> bool {
        self.release != 0
    }
}
