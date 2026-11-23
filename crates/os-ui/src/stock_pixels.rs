// SPDX-License-Identifier: GPL-3.0-or-later
//! Adapt foreign editor pixel layouts to the same display transform as a stock Canvas.
use std::cell::RefCell;
use os_theme::design::StockEditorLook;

thread_local! {
    static SCRATCH: RefCell<(Vec<[u8; 4]>, Vec<u32>)> = RefCell::new((Vec::new(), Vec::new()));
}

pub fn stock_pixels(data: &mut [u8], width: usize, height: usize, stride: usize, scale: f32,
                    bgra: bool, bottom_up: bool, look: StockEditorLook) {
    SCRATCH.with_borrow_mut(|(source, result)| {
        source.resize(width * height, [0; 4]);
        result.resize(width * height, 0);
        let rows = |y| if bottom_up { height - 1 - y } else { y };
        for (y, line) in data.chunks_exact(stride).enumerate() {
            let start = rows(y) * width;
            for (dst, p) in source[start..start + width].iter_mut().zip(line.chunks_exact(4)) {
                *dst = if bgra { [p[2], p[1], p[0], p[3]] } else { [p[0], p[1], p[2], p[3]] };
            }
        }
        super::bloom::present_stock_rgba(source, result, width, height, scale, look);
        for (y, line) in data.chunks_exact_mut(stride).enumerate() {
            let start = rows(y) * width;
            for (rgb, p) in result[start..start + width].iter().zip(line.chunks_exact_mut(4)) {
                let [_, r, g, b] = rgb.to_be_bytes();
                let alpha = p[3];
                let channels = if bgra { [b,g,r] } else { [r,g,b] };
                p[..3].copy_from_slice(&channels.map(|v| v.min(alpha)));
            }
        }
    });
}
