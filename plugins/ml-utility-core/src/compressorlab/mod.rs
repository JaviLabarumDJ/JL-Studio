// SPDX-License-Identifier: GPL-3.0-or-later
//! Native stock adapters for Ely Erin Fox's CompressorLab engines.

use crate::dsp::{fet, opto, vu};
use crate::{Fx, Unit};
use lemon_small_fx::{editor::Extension, params::FxParams, view::Model};
use nih_plug::prelude::*;
use os_ui::{Canvas, Color, Font, Rect};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

mod panel;
mod specs;
#[cfg(test)]
mod tests;

pub mod ml11766 {
    pub const NAME: &str = "ML 11766";
    pub const ID: &str = "com.mlstudio.11766";
    pub const VISIBILITY: &str = "visible_plugin-ml-11766";
    pub type Effect = super::Fx<super::Fet>;
    os_plugkit::linked_clap_entry!(Effect);
}

pub mod ml_la2aa {
    pub const NAME: &str = "ML LA2AA";
    pub const ID: &str = "com.mlstudio.la2aa";
    pub const VISIBILITY: &str = "visible_plugin-ml-la2aa";
    pub type Effect = super::Fx<super::Opto>;
    os_plugkit::linked_clap_entry!(Effect);
}

struct Movement {
    needle: vu::Vu,
    mode: usize,
    reading: Arc<AtomicU64>,
}

impl Default for Movement {
    fn default() -> Self {
        Self {
            needle: vu::Vu::new(48000.),
            mode: 0,
            reading: Arc::default(),
        }
    }
}

impl Movement {
    fn reset(&mut self, mode: usize) {
        self.mode = mode;
        self.needle.snap(rest(mode));
        self.publish();
    }

    fn advance(&mut self, mode: usize, target: f32, frames: usize) {
        if self.mode != mode {
            self.reset(mode);
        }
        // The meter's scale ends well above numerical silence. Clamp the
        // mechanical target to that scale so a burst can move it immediately.
        self.needle
            .advance(target.clamp(-20., if mode == 0 { 0. } else { 3. }), frames);
        self.publish();
    }

    fn publish(&self) {
        let reading = ((self.mode as u64) << 32) | u64::from(self.needle.value().to_bits());
        self.reading.store(reading, Ordering::Relaxed);
    }

    fn panel(&self) -> Arc<dyn Extension> {
        Arc::new(panel::Meter(self.reading.clone()))
    }
}

fn rest(mode: usize) -> f32 {
    if mode == 0 { 0. } else { -20. }
}

pub struct Fet {
    core: fet::Compressor,
    meter: Movement,
    rate: f32,
}

impl Default for Fet {
    fn default() -> Self {
        Self {
            core: fet::Compressor::new(48000.),
            meter: Movement::default(),
            rate: 48000.,
        }
    }
}

impl Fet {
    fn settings(p: &FxParams, mode: usize) -> fet::Settings {
        fet::Settings {
            input: p.v(0) as f32 * 0.1 + fet::MARK_MAX,
            output: p.v(1) as f32 * 0.1 + fet::MARK_MAX,
            attack: p.v(2) as f32 * 0.01,
            release: p.v(3) as f32 * 0.01,
            ratio: fet::Ratio::from_index(p.v(4) as usize),
            meter: fet::MeterMode::from_index(mode),
            mix: p.v(6) as f32 * 0.01,
            sc_hpf_hz: p.v(7) as f32,
            ..fet::Settings::default()
        }
    }
}

impl Unit for Fet {
    const SPEC: &'static lemon_small_fx::Spec = &specs::FET;
    const ID: &'static str = ml11766::ID;
    const CLASS: [u8; 16] = *b"ML11766000000001";

    fn latency(&self) -> u32 {
        self.core.latency() as u32
    }

    fn prepare(&mut self, rate: f32, p: &FxParams) {
        self.rate = rate;
        self.meter.needle.set_sample_rate(rate);
        self.reset(p);
    }

    fn reset(&mut self, p: &FxParams) {
        self.core.set_sample_rate(self.rate);
        let mode = p.v(5) as usize;
        self.core.configure(&Self::settings(p, mode));
        self.meter.reset(mode);
    }

    fn audio(&mut self, l: &mut [f32], r: &mut [f32], p: &FxParams) {
        let mode = p.v(5) as usize;
        let settings = Self::settings(p, mode);
        self.core.configure(&settings);
        self.core.process(l, r);
        self.meter
            .advance(mode, self.core.take_meter_reading(), l.len());
    }

    fn panel(&self, _: Arc<FxParams>) -> Arc<dyn Extension> {
        self.meter.panel()
    }
}

pub struct Opto {
    core: opto::Compressor,
    meter: Movement,
}

impl Default for Opto {
    fn default() -> Self {
        Self {
            core: opto::Compressor::new(48000.),
            meter: Movement::default(),
        }
    }
}

impl Opto {
    fn settings(p: &FxParams, mode: usize) -> opto::Settings {
        let meter = [opto::METER_GR, opto::METER_OUT4, opto::METER_OUT10][mode];
        opto::Settings {
            gain: p.v(0) as f32 * 0.1,
            peak_reduction: p.v(1) as f32 * 0.1,
            limit: p.v(2) == 1,
            meter,
            mix: p.v(4) as f32 * 0.01,
            sc_hpf: p.v(5) as f32,
            ..opto::Settings::default()
        }
    }
}

impl Unit for Opto {
    const SPEC: &'static lemon_small_fx::Spec = &specs::OPTO;
    const ID: &'static str = ml_la2aa::ID;
    const CLASS: [u8; 16] = *b"MLLA2AA000000001";

    fn prepare(&mut self, rate: f32, p: &FxParams) {
        self.core.set_sample_rate(rate);
        self.meter.needle.set_sample_rate(rate);
        self.reset(p);
    }

    fn reset(&mut self, p: &FxParams) {
        let mode = p.v(3) as usize;
        self.core.configure(Self::settings(p, mode));
        self.core.reset();
        self.meter.reset(mode);
    }

    fn audio(&mut self, l: &mut [f32], r: &mut [f32], p: &FxParams) {
        let mode = p.v(3) as usize;
        self.core.configure(Self::settings(p, mode));
        self.core.process_block(l, r);
        self.meter
            .advance(mode, self.core.meter_frame()[5], l.len());
    }

    fn panel(&self, _: Arc<FxParams>) -> Arc<dyn Extension> {
        self.meter.panel()
    }
}
