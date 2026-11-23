// SPDX-License-Identifier: GPL-3.0-or-later
//! Playback lighting is UI state. The project and the audio callback never own these envelopes.
use crate::app::{App, Live, Shown, State, Ui, Win};
use os_ui::{Canvas, Color, Rect};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

pub const VISIBILITY: &str = "visible_3036_synthwave";
static WRAPPER_LEVEL: AtomicU8 = AtomicU8::new(0);

pub fn wrapper_level() -> u8 {
    WRAPPER_LEVEL.load(Ordering::Relaxed)
}

#[derive(Clone, Debug, Default)]
pub struct Motion {
    pub enabled: bool,
    pub sensitivity: u8,
    pub level: u8,
    notes: BTreeMap<usize, u8>,
    pub keys: BTreeMap<i32, u8>,
    last: Option<Instant>,
    context: Option<(u16, usize, bool)>,
}

impl Motion {
    #[cfg(test)]
    pub fn elapse(&mut self, seconds: f32) {
        self.last = Some(Instant::now() - std::time::Duration::from_secs_f32(seconds));
    }

    pub fn load() -> Self {
        Self {
            enabled: crate::prefs::check(&crate::prefs::SYNTHWAVE),
            sensitivity: crate::prefs::slider(&crate::prefs::SYNTHWAVE_SENSITIVITY) as u8,
            ..Self::default()
        }
    }
}

pub fn active(s: &State) -> bool {
    s.neon_3036 && s.synthwave.enabled
}

// Map the master peak in dBFS so quiet passages get a useful share of the visual range.
fn response(peak: f32, sensitivity: u8) -> u8 {
    if !peak.is_finite() || peak <= 0. {
        return 0;
    }
    let floor = -40. - 0.48 * f32::from(sensitivity.min(100));
    let db = 20. * peak.log10();
    (((db - floor) / -floor).clamp(0., 1.).powf(0.86) * 255.).round() as u8
}

const KEY_RELEASE: f32 = 0.17;

pub fn flash_key(app: &mut App, key: i32) {
    if !active(&app.state) {
        return;
    }
    let ctx = Some((
        app.state.pattern,
        app.state.selected_channel,
        app.transport.song(),
    ));
    if app.state.synthwave.context != ctx {
        app.state.synthwave.keys.clear();
        app.state.synthwave.notes.clear();
        app.state.synthwave.context = ctx;
    }
    // A short tap after a long idle period gets its own release time.
    let now = Instant::now();
    if app.state.synthwave.last.is_none_or(|t| now.duration_since(t).as_secs_f32() > 0.04) {
        app.state.synthwave.last = Some(now);
    }
    app.state.synthwave.keys.insert(key, 255);
    if let Some(g) = app.piano_geom()
        && g.keys.contains(&key)
    {
        app.damage_rect(ray_rect(&g, key));
    }
    app.damage_live(Live::Sounding);
}

fn ray_rect(g: &crate::piano::Geom, key: i32) -> Rect {
    Rect::new(g.grid.x, g.y_of(key), g.grid.w.min(240.), g.row).intersect(&g.grid)
}

pub fn pitch_rays(c: &mut Canvas, s: &State, g: &crate::piano::Geom) {
    if !active(s) {
        return;
    }
    let [accent, _, pink] = s.neon_colors.inks;
    for (&key, &level) in &s.synthwave.keys {
        if !g.keys.contains(&key) {
            continue;
        }
        let rect = ray_rect(g, key);
        if rect.is_empty() {
            continue;
        }
        let energy = f32::from(level) / 255.;
        let length = rect.w * (0.35 + 0.65 * energy);
        let center = g.y_of(key) + g.row * 0.5;
        // A bright filament leaves the key tip and dissolves into the pitch lane.
        for part in 0..24 {
            let from = part as f32 / 24.;
            let x = rect.x + from * length;
            let width = length / 24.;
            let fade = energy * (1. - from).powi(2);
            let color = accent.mix(pink, from);
            c.fill_cell(
                Rect::new(x, rect.y, width, rect.h),
                color.with_alpha((64. * fade) as u8),
            );
            c.fill_cell(
                Rect::new(x, center, width, 1.3),
                color.with_alpha((180. * fade) as u8),
            );
        }
    }
}

fn decay(old: u8, seconds: f32, release: f32) -> u8 {
    (f32::from(old) * (-seconds / release).exp()).floor() as u8
}

fn pulse(age: f64, duration: f64, speed: f64, repeat: Option<u32>, velocity: u8) -> u8 {
    if age < 0. || age >= duration + speed * 0.48 || velocity == 0 {
        return 0;
    }
    let gate_age = age.min((duration - 0.0001).max(0.));
    let attack = repeat
        .filter(|&v| v > 0)
        .map_or(gate_age, |v| gate_age % f64::from(v));
    let sustain = 0.57 + 0.43 * (-attack / (speed * 0.075)).exp();
    let tail = (-(age - duration).max(0.) / (speed * 0.145)).exp();
    (255. * sustain * tail * (f64::from(velocity) / 100.).min(1.).sqrt()) as u8
}

fn illuminate<K: Ord>(map: &mut BTreeMap<K, u8>, key: K, value: u8) {
    if value > 3 {
        let old = map.entry(key).or_default();
        *old = (*old).max(value);
    }
}

pub fn advance(app: &mut App, old: &Shown, now: &Shown) {
    let s = &app.state;
    if !active(s) {
        if s.synthwave.last.is_some() {
            app.state.synthwave = Motion {
                enabled: s.synthwave.enabled,
                sensitivity: s.synthwave.sensitivity,
                ..Motion::default()
            };
        }
        if wrapper_level() != 0 {
            WRAPPER_LEVEL.store(0, Ordering::Relaxed);
        }
        return;
    }
    let instant = Instant::now();
    let dt = s
        .synthwave
        .last
        .map_or(1. / 60., |t| instant.duration_since(t).as_secs_f32());
    let context = Some((s.pattern, s.selected_channel, now.song));
    let running = now.playing && !now.paused;
    let speed = (now.tempo.unwrap_or(s.tempo) * f64::from(s.project.ppq) / 60.).max(1.);
    let reset = s.synthwave.context != context
        || (running
            && (now.tick < old.tick
                || now.tick - old.tick > speed * (f64::from(dt) + 0.12)
                || !old.playing
                || old.paused));
    if !running
        && !reset
        && now.sounding.is_empty()
        && s.piano.held.is_none()
        && s.piano.typed.is_empty()
        && s.synthwave.level == 0
        && s.synthwave.notes.is_empty()
        && s.synthwave.keys.is_empty()
    {
        return;
    }
    if dt < 1. / 45.
        && !reset
        && now.sounding == old.sounding
        && now.playing == old.playing
        && now.paused == old.paused
    {
        return;
    }

    let (l, r) = app.transport.peaks();
    let peak = if running || !now.sounding.is_empty() || s.piano.held.is_some() {
        l.max(r)
    } else {
        0.
    };
    let target = response(peak, s.synthwave.sensitivity);
    let level = target.max(decay(s.synthwave.level, dt, 0.14)) / 4 * 4;
    let mut notes = BTreeMap::new();
    let mut keys = BTreeMap::new();
    let piano_visible = s.shown(Win::PianoRoll) || s.native_detached.contains(&Win::PianoRoll);
    if piano_visible && !reset {
        for (&i, &v) in &s.synthwave.notes {
            illuminate(&mut notes, i, decay(v, dt, 0.105));
        }
        for (&k, &v) in &s.synthwave.keys {
            illuminate(&mut keys, k, decay(v, dt, KEY_RELEASE));
        }
    }
    if piano_visible
        && let Some(ch) = s
            .project
            .channels
            .get(s.selected_channel)
            .filter(|c| c.enabled)
    {
        let tick = now.visual_tick.unwrap_or(now.tick);
        if running && let Some(pattern) = s.project.pattern(s.pattern) {
            let mut light_pattern = |pos: f64, rate: f64, start: f64| {
                for (i, n) in pattern
                    .notes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.channel == ch.index && !n.muted())
                {
                    if f64::from(n.position) + f64::from(crate::piano::shown_len(s, n)) <= start {
                        continue;
                    }
                    let value = pulse(
                        pos - f64::from(n.position),
                        f64::from(crate::piano::shown_len(s, n)),
                        speed * rate,
                        n.repeat_ticks(s.project.ppq),
                        n.velocity,
                    );
                    illuminate(&mut notes, i, value);
                    illuminate(&mut keys, i32::from(n.key), value);
                }
            };
            if now.song {
                for (clip, pos, start) in crate::piano::playing_pattern_clips(s, tick) {
                    light_pattern(pos, clip.pattern_rate(), start);
                }
            } else {
                light_pattern(tick, 1., 0.);
            }
        }
        for k in s.piano.held.into_iter().chain(
            s.piano
                .typed
                .iter()
                .filter(|(c, _)| *c == ch.index)
                .map(|(_, k)| *k),
        ) {
            illuminate(&mut keys, k, if reset { 255 } else { 172 });
        }
        for &(_, key) in now
            .sounding
            .iter()
            .filter(|&&(c, k)| c == ch.index && k < 128)
        {
            illuminate(&mut keys, i32::from(key), if running { 144 } else { 172 });
        }
    }
    let rim_changed = level != s.synthwave.level;
    let keys_changed = keys != s.synthwave.keys;
    let mut damage = Vec::new();
    if keys_changed && let Some(g) = app.piano_geom() {
        for &key in keys
            .keys()
            .chain(s.synthwave.keys.keys().filter(|k| !keys.contains_key(k)))
        {
            if keys.get(&key) != s.synthwave.keys.get(&key) && g.keys.contains(&key) {
                damage.push(ray_rect(&g, key));
            }
        }
    }
    if notes != s.synthwave.notes
        && let Some(g) = app.piano_geom()
        && let Some(pattern) = s.project.pattern(s.pattern)
    {
        for &i in notes
            .keys()
            .chain(s.synthwave.notes.keys().filter(|i| !notes.contains_key(i)))
        {
            if notes.get(&i) != s.synthwave.notes.get(&i)
                && let Some(r) = pattern.notes.get(i).and_then(|n| g.note_rect(n))
            {
                let area = if crate::piano_pitch::shown(s) {
                    Rect::new(r.x - 2., g.grid.y, r.w + 4., g.grid.h)
                } else {
                    r.outset(2.)
                };
                let area = area.intersect(&g.grid);
                if !area.is_empty() {
                    damage.push(area);
                }
            }
        }
    }
    app.state.synthwave = Motion {
        enabled: true,
        sensitivity: s.synthwave.sensitivity,
        level,
        notes,
        keys,
        last: Some(instant),
        context,
    };
    if rim_changed {
        WRAPPER_LEVEL.store(level, Ordering::Relaxed);
        app.damage_live(Live::SynthwaveRim);
    }
    if keys_changed {
        app.damage_live(Live::Sounding);
    }
    for r in damage {
        app.damage_rect(r);
    }
}

pub fn note(s: &State, i: usize) -> Option<(Color, u8)> {
    if !active(s) {
        return None;
    }
    let value = *s.synthwave.notes.get(&i)?;
    let [accent, _, highlight] = s.neon_colors.inks;
    Some((highlight.mix(accent, f32::from(value) / 255.), value))
}

pub fn light_face(c: &mut Canvas, r: Rect, ink: Color, value: u8) {
    if r.w < 1. || r.h < 1. {
        return;
    }
    let energy = f32::from(value) / 255.;
    let hot = ink.mix(Color::rgb(0xfff7ff), 0.3 + 0.52 * energy);
    c.fill_rect_gradient(
        r,
        hot.with_alpha(value),
        ink.with_alpha((energy * 195.) as u8),
        1.,
    );
    c.fill_rect(
        Rect::new(r.x, r.bottom() - 1., r.w, 1.),
        ink.with_alpha(value),
    );
    c.fill_rect(Rect::new(r.x, r.y, r.w, 1.), hot.with_alpha(value));
    c.fill_rect(
        Rect::new(r.x, r.y, 1.5f32.min(r.w), r.h),
        ink.with_alpha(value),
    );
}

fn edges(r: Rect) -> [Rect; 4] {
    [
        Rect::new(r.x, r.y, r.w, 1.),
        Rect::new(r.x, r.bottom() - 1., r.w, 1.),
        Rect::new(r.x, r.y + 1., 1., (r.h - 2.).max(0.)),
        Rect::new(r.right() - 1., r.y + 1., 1., (r.h - 2.).max(0.)),
    ]
}

pub fn rim_pixels(c: &mut Canvas, r: Rect, color: Color, level: u8) {
    if level > 0 {
        for edge in edges(r) {
            c.fill_rect(edge, color.with_alpha(level / 4));
        }
    }
}

pub fn rim(ui: &mut Ui, s: &State, r: Rect) {
    if !active(s) || r.w < 3. || r.h < 3. {
        return;
    }
    for edge in edges(r) {
        ui.live(Live::SynthwaveRim, edge);
    }
    rim_pixels(ui.c, r, s.neon_colors.inks[0], s.synthwave.level);
}

#[cfg(test)]
#[test]
fn neon_3036_synthwave_envelopes() {
    assert_eq!(response(0., 100), 0);
    assert_eq!(response(f32::NAN, 65), 0);
    assert!(response(0.003, 65) > 75 && response(0.01, 65) > 105);
    assert!(response(0.001, 100) > response(0.001, 65) && response(0.001, 65) > response(0.001, 0));
    assert_eq!(response(1., 65), 255);
    let at = |age, repeat| pulse(age, 100., 200., repeat, 100);
    assert_eq!(at(-1., None), 0);
    assert_eq!(at(0., None), 255);
    assert!(at(49., Some(50)) < at(50., Some(50)));
    assert!(at(90., None) > at(115., None) && at(115., None) > at(140., None));
    assert_eq!(at(200., None), 0);
    assert_eq!(pulse(0., 100., 200., None, 0), 0);
    assert!(decay(200, 0.03, 0.14) > decay(200, 0.14, 0.14));
}
