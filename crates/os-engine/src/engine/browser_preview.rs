// SPDX-License-Identifier: GPL-3.0-or-later
//! Independent sample audition injected into a live Mixer track.
use std::{sync::Arc, time::Duration};
use std::sync::atomic::{AtomicU32, Ordering};

use os_dsp::resample::{self, Quality};

use crate::Sample;

pub(super) struct Voice {
    sample: Arc<Sample>,
    pub track: usize,
    start: f64,
    ratio: f64,
    remaining: u64,
    rendered: u64,
    fade: u64,
    target_gain: Arc<AtomicU32>,
    gain: f32,
    gain_step: f32,
    clip: bool,
}

impl Voice {
    pub fn new(
        sample: Arc<Sample>,
        track: usize,
        seconds: f64,
        limit: Option<Duration>,
        rate: u32,
        target_gain: Arc<AtomicU32>,
        clip: bool,
    ) -> Self {
        let start = seconds.max(0.0) * f64::from(sample.rate);
        let ratio = f64::from(sample.rate.max(1)) / f64::from(rate.max(1));
        let available =
            ((sample.frames().min(sample.right.len()) as f64 - start) / ratio).ceil() as u64;
        let remaining = limit.map_or(available, |limit| {
            available.min((limit.as_secs_f64() * f64::from(rate)) as u64)
        });
        Self {
            sample,
            track,
            start,
            ratio,
            remaining,
            rendered: 0,
            fade: u64::from(rate / 500).max(1),
            gain: f32::from_bits(target_gain.load(Ordering::Relaxed)),
            target_gain,
            gain_step: 1.0 / (rate.max(1) as f32 * 0.002).max(1.0),
            clip,
        }
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    pub fn release(&mut self) {
        self.remaining = self.remaining.min(self.fade);
    }

    pub fn mix(&mut self, out: &mut [f32], quality: Quality) {
        let target = f32::from_bits(self.target_gain.load(Ordering::Relaxed));
        for frame in out.chunks_exact_mut(2).take(self.remaining as usize) {
            let at = self.start + self.rendered as f64 * self.ratio;
            let (left, right) = resample::read(
                &self.sample.left,
                &self.sample.right,
                at,
                self.ratio,
                quality,
            );
            self.gain += (target - self.gain).clamp(-self.gain_step, self.gain_step);
            let left = left * self.gain;
            let right = right * self.gain;
            let (left, right) = if self.clip {
                (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0))
            } else {
                (left, right)
            };
            let fade = self.remaining.min(self.fade) as f32 / self.fade as f32;
            frame[0] += left * fade;
            frame[1] += right * fade;
            self.rendered += 1;
            self.remaining -= 1;
        }
    }
}
