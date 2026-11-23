// SPDX-License-Identifier: GPL-3.0-or-later
use crate::engine::{
    kit::{DrumKit, LayerMode},
    sequencer::{Pattern, NUM_PATTERNS, NUM_STEPS},
};
use std::fmt::Write;
use std::path::Path;

pub fn import(bytes: &[u8], kit: &DrumKit) -> Result<(Vec<Pattern>, usize), String> {
    let midi = os_smf::read(bytes).map_err(|error| error.to_string())?;
    if midi.ppq == 0 {
        return Err("The MIDI groove has an invalid time division".into());
    }
    let ppq = u64::from(midi.ppq);
    let mut patterns = Vec::new();
    for event in midi.tracks.iter().flat_map(|track| &track.events) {
        let os_smf::Kind::NoteOn { key, velocity, .. } = event.kind else {
            continue;
        };
        if velocity == 0 {
            continue;
        }
        let Some(pad) = kit.pad_for_note(key) else {
            continue;
        };
        let step = (u64::from(event.tick) * 4 + ppq / 2) / ppq;
        let bar = step / NUM_STEPS as u64;
        if bar >= NUM_PATTERNS as u64 {
            return Err("ML Drums imports up to 16 bars into its pattern bank".into());
        }
        while patterns.len() <= bar as usize {
            patterns.push(Pattern::new());
        }
        let trigger =
            &mut patterns[bar as usize].lanes[pad].steps[(step % NUM_STEPS as u64) as usize];
        let velocity = f32::from(velocity) / 127.0;
        trigger.velocity = if trigger.enabled {
            trigger.velocity.max(velocity)
        } else {
            velocity
        };
        trigger.enabled = true;
    }
    if patterns.is_empty() {
        return Err("The MIDI groove contains no notes mapped to ML Drums pads".into());
    }
    let count = patterns.len();
    Ok((patterns, count))
}

pub fn export_sfz(path: &Path, kit: &DrumKit, rate: u32) -> Result<(), String> {
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("ML Drums");
    let samples_name = format!("{stem} samples");
    let samples = folder.join(&samples_name);
    if !kit
        .pads
        .iter()
        .any(|pad| pad.sample.is_some() || pad.layers.iter().any(|layer| layer.sample.is_some()))
    {
        return Err("Load at least one sample before exporting a kit".into());
    }
    std::fs::create_dir(&samples).map_err(|e| format!("Create {}: {e}", samples.display()))?;
    let mut sfz = String::new();
    writeln!(&mut sfz, "<control> default_path={samples_name}/").unwrap();
    for (index, pad) in kit.pads.iter().enumerate() {
        let p = pad.performance;
        let count = 1 + pad.layers.len();
        for layer in 0..count {
            let (pcm, controls) = pad.layer(layer);
            let Some(pcm) = pcm.filter(|pcm| !pcm.is_empty()) else {
                continue;
            };
            let name = format!("pad-{:02}-layer-{:02}.wav", index + 1, layer + 1);
            let trim =
                crate::engine::trim::Trim::new(pad, pcm.len(), controls.reverse, rate as f32, 1.0);
            let right = pad.right_channel(layer).filter(|pcm| pcm.len() >= trim.end);
            write_pcm(
                &samples.join(&name),
                pcm,
                right.map(|r| r.as_slice()),
                rate,
                trim,
            )?;
            let gain = (pad.volume * controls.gain).max(0.00001);
            let pan = (pad.pan + controls.pan).clamp(-1., 1.) * 100.;
            writeln!(&mut sfz, "<region> sample={name} key={} lovel={} hivel={} volume={:.2} pan={:.2} tune={:.0} amp_veltrack={}",
                pad.midi_note, controls.low, controls.high, 20. * gain.log10(), pan, (pad.pitch + controls.pitch)*100.,
                if p.velocity_gain { 100 } else { 0 }).unwrap();
            if p.cut != 0 {
                writeln!(&mut sfz, "group={}", p.cut).unwrap();
            }
            if p.cut_by != 0 {
                writeln!(&mut sfz, "off_by={}", p.cut_by).unwrap();
            }
            if p.volume_envelope.enabled {
                let a = p.volume_envelope;
                writeln!(
                    &mut sfz,
                    "ampeg_attack={} ampeg_decay={} ampeg_sustain={} ampeg_release={}",
                    a.attack,
                    a.decay,
                    a.sustain * 100.,
                    a.release
                )
                .unwrap();
            }
            if p.layer_mode == LayerMode::Cycle {
                writeln!(&mut sfz, "seq_length={count} seq_position={}", layer + 1).unwrap();
            }
        }
    }
    std::fs::write(path, sfz).map_err(|e| format!("Write {}: {e}", path.display()))
}

fn write_pcm(
    path: &Path,
    pcm: &[f32],
    right: Option<&[f32]>,
    rate: u32,
    trim: crate::engine::trim::Trim,
) -> Result<(), String> {
    let channels = if right.is_some() { 2u16 } else { 1u16 };
    let align = channels * 2;
    let length = u32::try_from(trim.frames())
        .ok()
        .and_then(|n| n.checked_mul(u32::from(align)))
        .filter(|n| *n < u32::MAX - 36)
        .ok_or_else(|| "Wave sample is too large to export".to_owned())?;
    let mut wav = Vec::with_capacity(length as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + length).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
    wav.extend_from_slice(&align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&length.to_le_bytes());
    for i in 0..trim.frames() {
        let index = trim.index(i);
        let gain = trim.gain(i);
        let encode = |s: f32| (((s * gain).clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes();
        wav.extend_from_slice(&encode(pcm[index]));
        if let Some(right) = right {
            wav.extend_from_slice(&encode(right[index]));
        }
    }
    std::fs::write(path, wav).map_err(|e| format!("Write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_smf::{Event, Kind, Smf, Track};

    #[test]
    fn midi_groove_loads_both_banks_and_keeps_bar_boundaries() {
        let kit = DrumKit::new();
        let note = |tick, pad, velocity| Event {
            tick,
            kind: Kind::NoteOn {
                channel: 9,
                key: kit.note_for_pad(pad),
                velocity,
            },
        };
        let smf = Smf {
            format: 1,
            ppq: 96,
            tracks: vec![Track {
                events: vec![note(0, 0, 110), note(96, 31, 76), note(384, 1, 100)],
            }],
        };
        let (patterns, bars) = import(&os_smf::write(&smf), &kit).unwrap();
        assert_eq!(bars, 2);
        assert!(patterns[0].lanes[0].steps[0].enabled && patterns[0].lanes[31].steps[4].enabled);
        assert!(patterns[1].lanes[1].steps[0].enabled);
    }
}
