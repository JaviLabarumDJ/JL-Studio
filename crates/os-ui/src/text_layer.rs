// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{text::ClipPx, Color};

/// The backing canvas still contains the original artwork, including its text.
/// Remember the surface beneath glyphs so the light pass sees clean geometry.
#[derive(Clone, Copy, Default)]
pub(crate) struct LetterPixel {
    surface: [u8; 3],
    drawn: [u8; 3],
    opacity: u8,
}

impl LetterPixel {
    pub(crate) fn surface(self, raw: [u8; 4]) -> [u8; 4] {
        if self.visible(raw) {
            let [r, g, b] = self.surface;
            [r, g, b, raw[3]]
        } else {
            raw
        }
    }

    fn visible(self, raw: [u8; 4]) -> bool {
        // A later decoration or overlapping window can paint over the glyph.
        self.opacity != 0 && raw[..3] == self.drawn
    }

    pub(crate) fn over_light(self, raw: [u8; 4], lit: [u8; 3]) -> [u8; 3] {
        if !self.visible(raw) {
            return lit;
        }
        std::array::from_fn(|k| {
            let delta = i32::from(lit[k]) - i32::from(self.surface[k]);
            let transmitted = delta * i32::from(255 - self.opacity) / 255;
            (i32::from(self.drawn[k]) + transmitted).clamp(0, 255) as u8
        })
    }

    fn record(&mut self, before: [u8; 4], after: [u8; 4], ink: [u8; 3]) {
        if before[..3] == after[..3] {
            return;
        }
        let [r, g, b, _] = before;
        let old = [r, g, b];
        let [r, g, b, _] = after;
        let drawn = [r, g, b];
        // Use the channel with the greatest ink-to-face contrast to estimate
        // how much of the lit background passes through this antialiased pixel.
        let contrast = std::array::from_fn::<_, 3, _>(|k| old[k].abs_diff(ink[k]));
        let k = (0..3).max_by_key(|&k| contrast[k]).unwrap();
        let span = u32::from(contrast[k]).max(1);
        let changed = u32::from(old[k].abs_diff(drawn[k]));
        let opacity = ((changed * 255 + span / 2) / span).clamp(1, 255) as u8;
        let (surface, opacity) = if self.visible(before) {
            let uncovered = u16::from(255 - self.opacity) * u16::from(255 - opacity) / 255;
            (self.surface, 255 - uncovered as u8)
        } else {
            (old, opacity)
        };
        *self = Self { surface, drawn, opacity };
    }
}

#[derive(Default)]
pub(crate) struct TextLayer {
    pub(crate) pixels: Vec<LetterPixel>,
    before: Vec<[u8; 4]>,
}

impl TextLayer {
    pub(crate) fn capture(&mut self, pixels: &[[u8; 4]], stride: usize, (x0, y0, x1, y1): ClipPx) {
        self.pixels.resize(pixels.len(), LetterPixel::default());
        self.before.clear();
        for y in y0 as usize..y1 as usize {
            self.before.extend_from_slice(&pixels[y * stride + x0 as usize..y * stride + x1 as usize]);
        }
    }

    pub(crate) fn record(&mut self, pixels: &[[u8; 4]], stride: usize, (x0, y0, x1, y1): ClipPx, ink: Color) {
        let w = (x1 - x0) as usize;
        let ink = [ink.r, ink.g, ink.b];
        for (row, before) in self.before.chunks_exact(w).enumerate() {
            let start = (y0 as usize + row) * stride + x0 as usize;
            for ((letter, &old), &new) in self.pixels[start..start + w].iter_mut().zip(before).zip(&pixels[start..start + w]) {
                letter.record(old, new, ink);
            }
        }
    }
}
