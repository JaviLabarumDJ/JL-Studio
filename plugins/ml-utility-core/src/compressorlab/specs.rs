// SPDX-License-Identifier: GPL-3.0-or-later
use super::{fet, ml_la2aa, ml11766};
use lemon_small_fx::{Card, Control, Mark, Setting, Spec};
use os_ui::Rect;

const fn setting(id: &'static str, name: &'static str, min: i32, max: i32, start: i32) -> Setting {
    Setting {
        id,
        name,
        range: (min, max),
        start,
        magnet: None,
    }
}

const fn knob(id: &'static str, cx: f32, cy: f32, r: f32, caption: &'static str) -> Control {
    Control::Knob {
        id,
        cx,
        cy,
        r,
        caption,
        bipolar: false,
    }
}

const RATIOS: &[&str] = &["4:1", "8:1", "12:1", "20:1", "All"];
const MODES: &[&str] = &["Compress", "Limit"];
const FET_METERS: &[&str] = &["Gain reduction", "Output +4", "Output +8"];
const OPTO_METERS: &[&str] = &["Gain reduction", "Output +4", "Output +10"];
const MIX: Setting = setting("mix", "Mix", 0, 100, 100);
const HPF: Setting = setting("sc_hpf", "Detector high-pass", 0, 300, 0);
const MIX_KNOB: Control = knob("mix", 214., 198., 24., "Mix");
const HPF_KNOB: Control = knob("sc_hpf", 325., 198., 24., "Detector HP");
const CARDS: &[Card] = &[
    Card {
        x0: 12.,
        x1: 393.,
        y0: 24.,
        y1: 133.,
        heading: "COMPRESSOR",
    },
    Card {
        x0: 12.,
        x1: 140.,
        y0: 155.,
        y1: 254.,
        heading: "MODE",
    },
    Card {
        x0: 151.,
        x1: 393.,
        y0: 155.,
        y1: 254.,
        heading: "MIX AND DETECTOR",
    },
    Card {
        x0: 405.,
        x1: 651.,
        y0: 24.,
        y1: 254.,
        heading: "METER",
    },
];

pub(super) static FET: Spec = Spec {
    name: ml11766::NAME,
    word: "11766",
    about: "FET compressor adapted from Noob CompressorLab by Ely Erin Fox under the MIT license.",
    settings: &[
        setting("input", "Input", -480, 0, -240),
        setting("output", "Output", -480, 0, -240),
        setting("attack", "Attack", 100, 700, 400),
        setting("release", "Release", 100, 700, 400),
        setting("ratio", "Ratio", 0, 4, 0),
        setting("meter", "Meter", 0, 2, 0),
        MIX,
        HPF,
    ],
    controls: &[
        knob("input", 57., 73., 25., "Input"),
        knob("output", 154., 73., 25., "Output"),
        knob("attack", 250., 73., 25., "Attack"),
        knob("release", 346., 73., 25., "Release"),
        Control::Choice {
            id: "ratio",
            x: 29.,
            top: 165.,
            names: RATIOS,
        },
        Control::Choice {
            id: "meter",
            x: 431.,
            top: 195.,
            names: FET_METERS,
        },
        MIX_KNOB,
        HPF_KNOB,
    ],
    text: fet_text,
    parse: fet_parse,
    ..COMMON
};

pub(super) static OPTO: Spec = Spec {
    name: ml_la2aa::NAME,
    word: "LA2AA",
    about: "Optical compressor adapted from Noob CompressorLab by Ely Erin Fox under the MIT license.",
    settings: &[
        setting("gain", "Gain", 0, 1000, 320),
        setting("peak_reduction", "Peak Reduction", 0, 1000, 400),
        setting("limit", "Mode", 0, 1, 0),
        setting("meter", "Meter", 0, 2, 0),
        MIX,
        HPF,
    ],
    controls: &[
        knob("gain", 108., 70., 30., "Gain"),
        knob("peak_reduction", 296., 70., 30., "Peak Reduction"),
        Control::Choice {
            id: "limit",
            x: 29.,
            top: 183.,
            names: MODES,
        },
        Control::Choice {
            id: "meter",
            x: 431.,
            top: 195.,
            names: OPTO_METERS,
        },
        MIX_KNOB,
        HPF_KNOB,
    ],
    text: opto_text,
    parse: opto_parse,
    ..COMMON
};

const COMMON: Spec = Spec {
    name: "",
    word: "",
    about: "",
    settings: &[],
    studio_settings: 0,
    version: Some(1),
    any_version: false,
    fields: &[],
    tail: &[],
    size: (663, 289),
    cards: CARDS,
    controls: &[],
    display: None,
    mark: Mark {
        x: 646.,
        y: 272.,
        anchor: 1.,
        stacked: false,
    },
    hint: Some(Rect::new(14., 262., 440., 22.)),
    labels: &[],
    text: |_, _| unreachable!(),
    parse: |_, _| unreachable!(),
    sound: |_, _| unreachable!("these effects share the utility host wrapper"),
    rings: false,
};

fn fet_text(index: usize, value: i32) -> String {
    match index {
        0 | 1 => format!("{:.1} dB", value as f32 * 0.1),
        2 => format!(
            "{:.0} µs",
            fet::attack_seconds(value as f32 * 0.01).unwrap_or_default() * 1e6
        ),
        3 => format!("{:.0} ms", fet::release_seconds(value as f32 * 0.01) * 1e3),
        4 => RATIOS[value as usize].into(),
        5 => FET_METERS[value as usize].into(),
        6 => format!("{value}%"),
        _ => hpf_text(value),
    }
}

fn opto_text(index: usize, value: i32) -> String {
    match index {
        0 | 1 => format!("{:.1}", value as f32 * 0.1),
        2 => MODES[value as usize].into(),
        3 => OPTO_METERS[value as usize].into(),
        4 => format!("{value}%"),
        _ => hpf_text(value),
    }
}

fn hpf_text(value: i32) -> String {
    if value == 0 {
        "Off".into()
    } else {
        format!("{value} Hz")
    }
}

fn choice(text: &str, labels: &[&str]) -> Option<i32> {
    labels
        .iter()
        .position(|label| label.eq_ignore_ascii_case(text.trim()))
        .map(|i| i as i32)
}

fn number(text: &str, unit: &str) -> Option<f32> {
    let text = text.trim().trim_end_matches(unit).trim();
    text.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn scaled(text: &str, unit: &str, scale: f32) -> Option<i32> {
    Some((number(text, unit)? * scale).round() as i32)
}

fn timing(text: &str, unit: &str, slow: f32, fast: f32) -> Option<i32> {
    let time = number(text, unit)?.clamp(fast, slow);
    let mark = 1. + 6. * (time / slow).ln() / (fast / slow).ln();
    Some((mark * 100.).round() as i32)
}

fn parse_hpf(text: &str) -> Option<i32> {
    if text.trim().eq_ignore_ascii_case("Off") {
        Some(0)
    } else {
        scaled(text, "Hz", 1.)
    }
}

fn fet_parse(index: usize, text: &str) -> Option<i32> {
    match index {
        0 | 1 => scaled(text, "dB", 10.),
        2 => timing(text, "µs", 800., 20.),
        3 => timing(text, "ms", 1100., 50.),
        4 => choice(text, RATIOS),
        5 => choice(text, FET_METERS),
        6 => scaled(text, "%", 1.),
        _ => parse_hpf(text),
    }
}

fn opto_parse(index: usize, text: &str) -> Option<i32> {
    match index {
        0 | 1 => scaled(text, "", 10.),
        2 => choice(text, MODES),
        3 => choice(text, OPTO_METERS),
        4 => scaled(text, "%", 1.),
        _ => parse_hpf(text),
    }
}
