// SPDX-License-Identifier: GPL-3.0-or-later
//! Short-window observations and bounded, render-verified mix decisions.
use super::*;

pub(super) const STEP: f64 = 0.02;
pub(super) const HZ: [f64; 6] = [100., 250., 650., 1600., 4000., 9000.];

#[derive(Clone, Default, Debug)]
pub(super) struct Frame {
    pub rms: f32,
    pub peak: f32,
    pub bands: [f32; 6],
}

pub(super) struct Observer {
    size: usize,
    count: usize,
    energy: f64,
    peak: f64,
    bands: [f64; 6],
    coefficients: [[f64; 3]; 6],
    state: [[[f64; 2]; 2]; 6],
    pub frames: Vec<Frame>,
}

impl Observer {
    pub fn new(rate: u32) -> Self {
        Self {
            size: (f64::from(rate) * STEP).round() as usize,
            count: 0,
            energy: 0.,
            peak: 0.,
            bands: [0.; 6],
            coefficients: HZ.map(|hz| {
                let w = std::f64::consts::TAU * hz.min(f64::from(rate) * 0.4) / f64::from(rate);
                let alpha = w.sin() / 2.; // Constant-Q, unity peak band-pass.
                [
                    alpha / (1. + alpha),
                    -2. * w.cos() / (1. + alpha),
                    (1. - alpha) / (1. + alpha),
                ]
            }),
            state: [[[0.; 2]; 2]; 6],
            frames: Vec::new(),
        }
    }

    pub fn feed(&mut self, audio: &[f32]) {
        for frame in audio.chunks_exact(2) {
            for (ch, &x) in frame.iter().enumerate() {
                let x = f64::from(x);
                self.energy += x * x;
                self.peak = self.peak.max(x.abs());
                for k in 0..6 {
                    let [b, a1, a2] = self.coefficients[k];
                    let z = &mut self.state[k][ch];
                    let y = b * x + z[0];
                    z[0] = -a1 * y + z[1];
                    z[1] = -b * x - a2 * y;
                    self.bands[k] += y * y;
                }
            }
            self.count += 1;
            if self.count == self.size {
                self.flush();
            }
        }
    }

    pub fn flush(&mut self) {
        if self.count == 0 {
            return;
        }
        let rms = |energy: f64| db((energy / (2 * self.count) as f64).sqrt()) as f32;
        self.frames.push(Frame {
            rms: rms(self.energy),
            peak: db(self.peak) as f32,
            bands: self.bands.map(rms),
        });
        self.count = 0;
        self.energy = 0.;
        self.peak = 0.;
        self.bands = [0.; 6];
    }
}

pub(super) fn quantile(values: impl Iterator<Item = f64>, fraction: f64) -> f64 {
    let mut values: Vec<_> = values.filter(|v| v.is_finite()).collect();
    if values.is_empty() {
        return -90.;
    }
    values.sort_by(f64::total_cmp);
    values[((values.len() - 1) as f64 * fraction.clamp(0., 1.)).round() as usize]
}

impl Level {
    pub(super) fn rms(&self, fraction: f64) -> f64 {
        let top = self.frames.iter().map(|f| f.rms).fold(-90., f32::max);
        quantile(
            self.frames
                .iter()
                .filter(|f| f.rms > (top - 30.).max(-70.))
                .map(|f| f64::from(f.rms)),
            fraction,
        )
    }

    pub(super) fn dynamics(&self) -> f64 {
        (self.rms(0.95) - self.rms(0.5)).max(0.)
    }

    /// Peak excursions relative to the channel's normal active peaks, ignoring silent gaps.
    pub(super) fn peak_excursion(&self) -> f64 {
        let gate=(self.rms(0.5)-18.).max(-65.) as f32;
        let active=||self.frames.iter().filter(|f| f.rms>gate).map(|f|f64::from(f.peak));
        if active().count()<12 {return 0.}
        // A sustained syllable can cover less than one percent of a long performance.
        // Two adjacent active frames reject isolated one-frame clicks.
        let sustained = self.frames.windows(2)
            .filter(|w| w.iter().all(|f| f.rms > gate))
            .map(|w| f64::from(w[0].peak.min(w[1].peak)))
            .fold(-90., f64::max);
        (quantile(active(),0.99).max(sustained)-quantile(active(),0.6)).max(0.)
    }
    pub(super) fn burst_ms(&self) -> f64 {
        let threshold = self.rms(0.75) as f32;
        let mut spans = Vec::new();
        let mut n = 0;
        for f in &self.frames {
            if f.rms > threshold {
                n += 1;
            } else if n > 0 {
                spans.push(n as f64 * STEP * 1000.);
                n = 0;
            }
        }
        if n > 0 {
            spans.push(n as f64 * STEP * 1000.);
        }
        quantile(spans.into_iter(), 0.5).clamp(40., 300.)
    }
}

/// Only carve a competing instrument when it overlaps an audible focus in time
/// AND frequency. Six broad bands are evidence for gentle cuts, not source separation.
pub(super) fn masking(track: &Track, focus: &Track) -> Option<(usize, f64)> {
    let low_end = track.role == Role::Bass && focus.role == Role::Kick;
    if track.id == focus.id
        || track.role.is_bus()
        || focus.role.is_bus()
        || !(low_end || matches!(track.role.family(), Role::Music | Role::Pad | Role::BackingVocal))
    {
        return None;
    }
    let mut best = None;
    let focus_gate = (focus.level.rms(0.9) - 14.).max(-60.) as f32;
    for band in if low_end { 0..2 } else { 2..5 } {
        let overlap: Vec<_> = track
            .level
            .frames
            .iter()
            .zip(&focus.level.frames)
            .filter(|(a, b)| {
                a.rms > -60.
                    && b.rms > focus_gate
                    && a.bands[band] > a.rms - 12.
                    && b.bands[band] > b.rms - 12.
            })
            .map(|(a, b)| {
                (
                    f64::from(a.bands[band] - b.bands[band]),
                    f64::from((a.bands[band] - a.rms).min(b.bands[band] - b.rms)),
                )
            })
            .collect();
        if overlap.len() < if low_end { 8 } else { 25 } {
            continue;
        }
        let difference = quantile(overlap.iter().map(|p| p.0), 0.5);
        // Prefer shared fundamentals and formants over filter skirts, even
        // when a weak neighbouring band has a higher source-to-focus ratio.
        let score = difference.min(6.) + quantile(overlap.into_iter().map(|p| p.1), 0.5);
        if difference > -3. && best.is_none_or(|(_, _, old)| score > old) {
            best = Some((band, difference, score));
        }
    }
    best.map(|(band, difference, _)| (band, difference))
}

/// Difference between aligned windows. Attenuation at ordinary moments must not
/// hide an enlarged syllable/transient somewhere else in the song.
#[derive(Clone, Debug)]
pub(super) struct Change {
    pub median: f64,
    pub reduction: f64,
    pub surge: f64,
    pub at: usize,
}

pub(super) fn change(before: &Level, after: &Level) -> Change {
    let gate = (before.rms(0.5) - 20.).max(-65.) as f32;
    let pairs: Vec<_> = before
        .frames
        .iter()
        .zip(&after.frames)
        .enumerate()
        .filter(|(_, (a, b))| a.rms > gate && b.rms.is_finite())
        .collect();
    if pairs.is_empty() {
        return Change {median:0.,reduction:0.,surge:0.,at:0}
    }
    let median = quantile(
        pairs.iter().map(|(_, (a, b))| f64::from(b.rms - a.rms)),
        0.5,
    );
    let reduction = -quantile(
        pairs.iter().map(|(_, (a, b))| f64::from(b.rms - a.rms)),
        0.1,
    );
    let (at, surge) = pairs
        .iter()
        .map(|(i, (a, b))| (*i, f64::from(b.peak - a.peak) - median))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0, 0.));
    Change {
        median,
        reduction,
        surge,
        at,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Correction {
    Processing,
    LowerGain,
    RestoreLevel,
}

#[derive(Clone)]
pub(super) struct Issue {
    pub track: u16,
    pub at: usize,
    pub reason: &'static str,
    pub correction: Correction,
}

#[cfg(test)]
pub(super) fn validate(before: &Render, after: &Render, tracks: &[Track]) -> Vec<Issue> {
    validate_proposal(before, after, tracks, &[], None)
}

/// Loudness leveling deliberately lifts quiet phrases relative to the median.
/// Flag an audible overshoot when it changes the shape of a prominent attack,
/// or when the whole syllable jumps substantially beyond the intended fader move.
fn prominent_attack(
    before: &Level,
    after: &Level,
    fader: f64,
    median: f64,
    tolerance: f64,
) -> Option<usize> {
    let gate = (before.rms(0.5) - 20.).max(-65.) as f32;
    before
        .frames
        .iter()
        .zip(&after.frames)
        .enumerate()
        .filter_map(|(i, (old, new))| {
            if old.rms <= gate || f64::from(new.peak) < (after.true_peak - 10.).max(-48.) {
                return None;
            }
            // An EQ or compressor can shift the loudest sample across a 20 ms
            // boundary. Compare the nearby input peak while retaining per-window
            // overload checks and the oversampled absolute peak check separately.
            let start = i.saturating_sub(1);
            let end = (i + 2).min(before.frames.len());
            let input_peak = before.frames[start..end]
                .iter()
                .map(|f| f.peak)
                .fold(old.peak, f32::max);
            let growth = f64::from(new.peak - input_peak);
            let excess = growth - median;
            let crest_growth = f64::from((new.peak - new.rms) - (old.peak - old.rms));
            (growth - fader > 1.
                && (growth > 0.5 || excess > 7. || new.peak > -3.)
                && excess > tolerance
                && (crest_growth > 2. || growth - fader > 7.))
                .then_some((i, excess))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

pub(super) fn validate_proposal(
    before: &Render,
    after: &Render,
    tracks: &[Track],
    moves: &[Move],
    control: Option<&Render>,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    for (id, a) in &before.levels {
        let Some(b) = after.levels.get(id) else {
            issues.push(Issue {
                track: *id,
                at: 0,
                reason: "became inaudible",
                correction: Correction::RestoreLevel,
            });
            continue;
        };
        let is_bus = tracks.iter().any(|t| t.id == *id && t.role.is_bus());
        let reference = control
            .filter(|_| is_bus)
            .and_then(|r| r.levels.get(id))
            .unwrap_or(a);
        let c = change(reference, b);
        let vocal = tracks.iter().any(|t| {
            t.id == *id
                && matches!(
                    t.role,
                    Role::LeadVocal | Role::BackingVocal | Role::VocalBus
                )
        });
        let fader = if is_bus && control.is_some() {
            0.
        } else {
            moves
                .iter()
                .find(|m| m.track == *id)
                .map_or(0., |m| m.delta)
        };
        let attack = prominent_attack(reference, b, fader, c.median, if vocal { 3. } else { 4.5 });
        let reason = if b.true_peak > -0.1 && b.true_peak > a.true_peak + 0.6 {
            Some(("increased an over-ceiling peak", Correction::LowerGain))
        } else if attack.is_some() {
            Some(("made a short peak stand out more", Correction::Processing))
        } else if c.reduction + c.median > 7. {
            Some((
                "over-controlled the track's dynamics",
                Correction::Processing,
            ))
        } else if c.median < -9.
            && c.median
                < moves
                    .iter()
                    .find(|m| m.track == *id)
                    .map_or(0., |m| m.delta.min(0.))
                    - 4.
        {
            Some(("reduced the track too much", Correction::RestoreLevel))
        } else {
            None
        };
        if let Some((reason, correction)) = reason {
            issues.push(Issue {
                track: *id,
                at: if b.true_peak > -0.1 && b.true_peak > a.true_peak + 0.6 {
                    b.frames
                        .iter()
                        .enumerate()
                        .max_by(|(_, x), (_, y)| x.peak.total_cmp(&y.peak))
                        .map_or(c.at, |(i, _)| i)
                } else {
                    attack.unwrap_or(c.at)
                },
                reason,
                correction,
            });
        }
    }
    if let (Some(a), Some(b)) = (&before.output, &after.output) {
        // A newly clipping passage matters even if the original song already
        // had a higher peak elsewhere. Compare to the original for this check.
        let new_over = a
            .frames
            .iter()
            .zip(&b.frames)
            .position(|(old, new)| new.peak > 0. && new.peak > old.peak + 0.6);
        let expected = control.and_then(|r| r.output.as_ref()).unwrap_or(a);
        let c = change(expected, b);
        // A cut can leave an isolated quiet passage unchanged while lowering
        // the rest of the mix. Only flag a relative surge in the sum when
        // that passage actually grew and now peaks near the top of the mix.
        let prominent_surge = prominent_attack(expected, b, 0., c.median, 4.5);
        if new_over.is_some()
            || b.true_peak > -0.1 && b.true_peak > a.true_peak + 0.6
            || prominent_surge.is_some()
        {
            issues.push(Issue {
                track: 0,
                at: new_over.or(prominent_surge).unwrap_or(c.at),
                reason: "increased a Master output peak",
                correction: if new_over.is_some()
                    || b.true_peak > -0.1 && b.true_peak > a.true_peak + 0.6
                {
                    Correction::LowerGain
                } else {
                    Correction::Processing
                },
            });
        }
    } else if before.output.is_some() && after.output.is_none() {
        issues.push(Issue {
            track: 0,
            at: 0,
            reason: "made the Master output inaudible",
            correction: Correction::RestoreLevel,
        });
    }
    if after.peak > 0. && after.peak > before.peak + 0.6 {
        issues.push(Issue {
            track: 0,
            at: 0,
            reason: "increased overload before Master",
            correction: Correction::LowerGain,
        });
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_assistant_short_loud_syllable_survives_long_song_percentiles() {
        let mut level=Level {frames:vec![Frame {rms:-25.,peak:-18.,bands:[-36.;6]};1000],..Default::default()};
        level.frames[300].peak=-2.;
        assert_eq!(level.peak_excursion(),0.);
        level.frames[301].peak=-3.;
        assert_eq!(level.peak_excursion(),15.);
    }
    #[test]
    fn mix_assistant_silent_window_has_neutral_change() {
        let level=Level::default();
        let result=change(&level,&level);
        assert_eq!([result.median,result.reduction,result.surge],[0.;3]);
    }
    #[test]
    fn mix_assistant_distinguishes_phrase_leveling_from_a_new_attack() {
        let mut before = Level {
            frames: vec![
                Frame {
                    rms: -20.,
                    peak: -14.,
                    bands: [-32.; 6]
                };
                100
            ],
            true_peak: -14.,
            ..Default::default()
        };
        // A quieter phrase gets makeup while high-level passages are compressed.
        for f in &mut before.frames[50..60] {
            f.rms = -32.;
            f.peak = -26.;
        }
        let mut after = before.clone();
        for f in &mut after.frames {
            f.rms -= 3.;
            f.peak -= 3.;
        }
        for f in &mut after.frames[50..60] {
            f.rms = -28.;
            f.peak = -22.;
        }
        after.true_peak = -17.;
        let c = change(&before, &after);
        assert!(c.surge > 6.);
        assert_eq!(prominent_attack(&before, &after, 0., c.median, 3.), None);
        after.frames[54].peak = -13.;
        after.true_peak = -13.;
        assert_eq!(
            prominent_attack(&before, &after, 0., c.median, 3.),
            Some(54)
        );
    }

    #[test]
    fn mix_assistant_detects_a_late_surge_even_when_global_peak_is_unchanged() {
        let mut a = Level {
            frames: vec![
                Frame {
                    rms: -26.,
                    peak: -20.,
                    bands: [-40.; 6]
                };
                1000
            ],
            true_peak: -2.,
            ..Default::default()
        };
        a.frames[50].peak = -2.;
        let mut b = a.clone();
        b.frames[800].peak = -5.;
        b.frames[800].rms = -11.;
        let render = |level| Render {
            levels: BTreeMap::from([(1, level)]),
            output: None,
            peak: -2.,
            excerpt_at: 0,
            excerpt: vec![],
            rate: 48000,
        };
        let mut before = render(a.clone());
        let mut after = render(b.clone());
        let issues = validate(&before, &after, &[]);
        // An even larger change in a quiet tail must not hide the genuinely
        // prominent later transient in the Master.
        a.frames[200].rms = -40.;
        a.frames[200].peak = -40.;
        b.frames[200].rms = -23.;
        b.frames[200].peak = -22.;
        before.output = Some(a);
        after.output = Some(b);
        let master = validate(&before, &after, &[])
            .into_iter()
            .find(|i| i.track == 0)
            .unwrap();
        assert_eq!(master.at, 800);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].at, 800);
        assert_eq!(timestamp(issues[0].at), "0:16.0");
    }

    #[test]
    fn mix_assistant_short_window_meter_is_block_independent_at_both_rates() {
        for rate in [44100, 48000] {
            let audio: Vec<_> = (0..rate * 2)
                .flat_map(|i| {
                    let x = (f64::from(i) * std::f64::consts::TAU * 1600. / f64::from(rate)).sin()
                        as f32
                        * 0.2;
                    [x, x]
                })
                .collect();
            let mut all = Observer::new(rate);
            all.feed(&audio);
            all.flush();
            let mut blocks = Observer::new(rate);
            for chunk in audio.chunks(514) {
                blocks.feed(chunk);
            }
            blocks.flush();
            assert_eq!(all.frames.len(), 100);
            for (a, b) in all.frames.iter().zip(&blocks.frames) {
                assert_eq!(a.rms, b.rms);
                assert_eq!(a.bands, b.bands);
                assert!((f64::from(a.rms) + 16.99).abs() < 0.1);
            }
        }
    }
}

pub(super) fn timestamp(frame: usize) -> String {
    let seconds = frame as f64 * STEP;
    format!("{}:{:04.1}", seconds as usize / 60, seconds % 60.)
}
