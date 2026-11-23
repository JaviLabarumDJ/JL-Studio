// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct Meter(pub Arc<AtomicU64>);

impl Extension for Meter {
    fn paint(&self, c: &mut Canvas, m: &dyn Model) {
        let mode = m.plain("meter").round() as usize;
        let reading = self.0.load(Ordering::Relaxed);
        let value = if (reading >> 32) as usize == mode {
            f32::from_bits(reading as u32)
        } else {
            rest(mode)
        };
        draw(c, mode == 0, value);
    }

    fn click(&self, _: f32, _: f32, _: Option<&Arc<dyn GuiContext>>) -> bool {
        false
    }
}

fn draw(c: &mut Canvas, gain_reduction: bool, value: f32) {
    use os_ui::kit::lemon::{self, plug};

    let well = Rect::new(415., 35., 226., 148.);
    lemon::plug_well(c, well, 8.);
    let pivot = (well.cx(), 166.);
    let ticks: &[(f32, &str)] = if gain_reduction {
        &[
            (-20., "20"),
            (-10., "10"),
            (-7., "7"),
            (-5., "5"),
            (-3., "3"),
            (-1., "1"),
            (0., "0"),
        ]
    } else {
        &[
            (-20., "-20"),
            (-10., "-10"),
            (-7., "-7"),
            (-5., "-5"),
            (-3., "-3"),
            (-1., "-1"),
            (0., "0"),
            (1., "+1"),
            (2., "+2"),
            (3., "+3"),
        ]
    };
    let steps = (ticks.len() - 1) as f32;
    let point = |step: f32, radius: f32| {
        let angle = (step / steps - 0.5) * 1.94;
        (
            pivot.0 + angle.sin() * radius,
            pivot.1 - angle.cos() * radius,
        )
    };
    let accent = Color::rgb(0xe4b365);
    for (i, &(db, label)) in ticks.iter().enumerate() {
        let ink = if db >= 0. { accent } else { plug::DIM };
        let (x0, y0) = point(i as f32, 90.);
        let (x1, y1) = point(i as f32, 97.);
        c.line(x0, y0, x1, y1, 1., ink);
        let (x, y) = point(i as f32, 110.);
        c.text_center(
            Font::new(10),
            Rect::new(x - 12., y - 7., 24., 14.),
            ink,
            label,
        );
    }

    let value = if value.is_finite() {
        value
    } else {
        rest(usize::from(!gain_reduction))
    };
    let value = value.clamp(ticks[0].0, ticks[ticks.len() - 1].0);
    let step = ticks
        .windows(2)
        .enumerate()
        .find_map(|(i, marks)| {
            let low = marks[0].0;
            let high = marks[1].0;
            (value <= high).then(|| i as f32 + (value - low) / (high - low))
        })
        .unwrap_or(steps);
    let (x, y) = point(step, 89.);
    c.line(pivot.0, pivot.1, x, y, 1.6, accent);
    c.fill_circle(pivot.0, pivot.1, 4.5, Color::rgb(0xd8d6cf));
    c.fill_circle(pivot.0, pivot.1, 2.4, plug::WELL);
    let label = if gain_reduction {
        "GAIN REDUCTION · dB"
    } else {
        "OUTPUT · VU"
    };
    c.text_center(
        Font::new(10).bold(),
        Rect::new(423., 170., 210., 11.),
        plug::DIM,
        label,
    );
}
