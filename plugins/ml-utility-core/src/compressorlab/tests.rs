// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

const RATE: f32 = 48000.;
const BLOCK: usize = 240;

fn tone(offset: usize) -> [f32; BLOCK] {
    std::array::from_fn(|i| {
        (std::f32::consts::TAU * 220. * (offset + i) as f32 / RATE).sin() * 0.65
    })
}

#[test]
fn adapters_match_both_pinned_donor_engines() {
    let mut fet = Fet::default();
    let mut opto = Opto::default();
    let fet_params = FxParams::new(Fet::SPEC);
    let opto_params = FxParams::new(Opto::SPEC);
    fet.prepare(RATE, &fet_params);
    opto.prepare(RATE, &opto_params);
    let mut reference_fet = fet::Compressor::new(RATE);
    let mut reference_opto = opto::Compressor::new(RATE);
    for block in 0..80 {
        let input = tone(block * BLOCK);
        let right = input.map(|v| v * 0.7);
        let (mut l, mut r) = (input, right);
        let (mut reference_l, mut reference_r) = (input, right);
        reference_fet.process(&mut reference_l, &mut reference_r);
        fet.audio(&mut l, &mut r, &fet_params);
        assert_eq!((l, r), (reference_l, reference_r));
        let (mut l, mut r) = (input, right);
        let (mut reference_l, mut reference_r) = (input, right);
        reference_opto.process_block(&mut reference_l, &mut reference_r);
        opto.audio(&mut l, &mut r, &opto_params);
        assert_eq!((l, r), (reference_l, reference_r));
    }
    assert!(reference_fet.gr_db() < -0.5);
    assert!(reference_opto.gain_reduction_db(0) > 0.5);
    assert_eq!(fet.latency(), reference_fet.latency() as u32);
    fet.prepare(96000., &fet_params);
    assert_eq!(fet.latency(), 0);
}

#[test]
fn visible_meter_switch_parks_then_responds_without_affecting_audio() {
    let mut movement = Movement::default();
    movement.reset(0);
    movement.advance(0, -6., 14400);
    assert!((movement.needle.value() + 6.).abs() < 0.3);
    movement.advance(1, 0., 0);
    assert_eq!(movement.needle.value(), -20.);
    movement.advance(1, 0., 14400);
    assert!(movement.needle.value().abs() < 0.6);
    let reading = movement.reading.load(Ordering::Relaxed);
    assert_eq!(reading >> 32, 1);
    assert_eq!(f32::from_bits(reading as u32), movement.needle.value());

    let mut a = fet::Compressor::new(RATE);
    let mut b = fet::Compressor::new(RATE);
    let settings = fet::Settings {
        meter: fet::MeterMode::Plus4,
        ..Default::default()
    };
    b.configure(&settings);
    let input = tone(0);
    let (mut al, mut ar) = (input, input);
    let (mut bl, mut br) = (input, input);
    a.process(&mut al, &mut ar);
    b.process(&mut bl, &mut br);
    assert_eq!((al, ar), (bl, br));
}
