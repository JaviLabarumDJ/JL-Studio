// SPDX-License-Identifier: GPL-3.0-or-later
//! Vintage cursor resources with optional local overrides and an embedded default.
use crate::{app::Cursor, playlist::PlTool, playlist_tools as tools};
use os_ui::{Canvas, Color, Rect};
use std::{collections::HashMap, sync::OnceLock};
use winit::window::{CustomCursor, CustomCursorSource};

pub const PACK: u8 = os_theme::appearance::VINTAGE_CURSOR_PACK;
const SIZE: usize = 32;

struct Bitmap {
    rgba: Vec<u8>,
    hotspot: [u16; 2],
}

fn decode(data: &[u8]) -> Option<Bitmap> {
    // RT_CURSOR hotspot, BITMAPINFOHEADER, black/white palette, XOR and AND masks.
    if data.len() != 308 || data[4..24] != [40, 0, 0, 0, 32, 0, 0, 0, 64, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0] {
        return None;
    }
    let hotspot = [
        u16::from_le_bytes(data[0..2].try_into().ok()?),
        u16::from_le_bytes(data[2..4].try_into().ok()?),
    ];
    if hotspot.iter().any(|&p| p >= SIZE as u16) { return None; }
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let index = (SIZE - 1 - y) * 4 + x / 8;
            let bit = 0x80 >> (x % 8);
            let xor = data[52 + index] & bit != 0;
            let and = data[180 + index] & bit != 0;
            let palette = 44 + usize::from(xor) * 4;
            let pixel = if and && !xor { [0; 4] }
                else { [data[palette + 2], data[palette + 1], data[palette], 255] };
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba.chunks_exact(4).any(|p| p[3] != 0).then_some(Bitmap { rgba, hotspot })
}

fn resource(cursor: Cursor) -> Option<&'static str> {
    Some(match cursor {
        Cursor::EwResize | Cursor::Playlist(_, tools::RESIZE) => "SIZEH",
        Cursor::NsResize => "MOVEV",
        Cursor::Playlist(_, tools::STRETCH) => "32765",
        Cursor::Playlist(_, tools::GRAB | tools::GRABBING) => "MOVE",
        Cursor::Playlist(_, tools::COPYING) => "32763",
        Cursor::Playlist(_, 4) => "32766",
        Cursor::Playlist(_, tools::GAIN) => "32761",
        Cursor::Playlist(_, 12) => "EESEL",
        Cursor::Playlist(_, 20) => "EECUT",
        Cursor::Playlist(_, 21) => "EEPEN",
        Cursor::Playlist(_, 22) => "MOVE",
        Cursor::Playlist(_, 23) => "EEZOOM",
        Cursor::Playlist(tool, 0..=3) => match tool {
            PlTool::Draw => "EEPEN",
            PlTool::Paint => "EEBRUSH",
            PlTool::Slice => "EECUT",
            PlTool::Delete => "EEDEL",
            PlTool::Select => "EESEL",
            PlTool::Mute => "32767",
            PlTool::Zoom => "EEZOOM",
            PlTool::Slip => "EESLIDE",
            _ => return None,
        },
        _ => return None,
    })
}

fn bitmap(cursor: Cursor) -> Option<&'static Bitmap> {
    let name = resource(cursor)?;
    static IMAGES: OnceLock<HashMap<&'static str, Bitmap>> = OnceLock::new();
    IMAGES.get_or_init(|| {
        ["SIZEH", "MOVEV", "MOVE", "32763", "32765", "32766", "32761", "EEPEN", "EEBRUSH",
         "EECUT", "EEDEL", "EESEL", "32767", "EEZOOM", "EESLIDE"].into_iter().filter_map(|name| {
            let bitmap = crate::vintage_skin::directories().into_iter().find_map(|dir| {
                let path = dir.join("Cursors").join(format!("{name}.bin"));
                if std::fs::metadata(&path).ok()?.len() != 308 { return None; }
                decode(&std::fs::read(path).ok()?)
            }).or_else(|| decode(crate::vintage_skin::bundled_asset(&format!("Cursors/{name}.bin"))?))?;
            Some((name, bitmap))
        }).collect()
    }).get(name)
}

pub(crate) fn tint() -> Option<Color> {
    let theme = os_theme::current();
    if crate::vintage::enabled() || theme.style.neon_3036 { None } else { theme.appearance.cursor_color }
}

impl Bitmap {
    fn scaled(&self, side: usize, tint: Option<Color>) -> Vec<u8> {
        // Integrate the original mask over each device pixel. Whole-number
        // enlargement copies the source exactly. Fractions smooth just the
        // crossed edges, using premultiplied weights to keep transparent
        // padding from darkening white cursor artwork.
        if side == SIZE && tint.is_none() {
            return self.rgba.clone();
        }
        let taps: Vec<_> = (0..side).map(|p| {
            let (lo, hi) = (p * SIZE, (p + 1) * SIZE);
            (lo / side..hi.div_ceil(side)).map(|source| {
                let overlap = hi.min((source + 1) * side) - lo.max(source * side);
                (source, overlap as u32)
            }).collect::<Vec<_>>()
        }).collect();
        let mut pixels = Vec::with_capacity(side * side * 4);
        let area = (SIZE * SIZE) as u32;
        for rows in &taps {
            for columns in &taps {
                let mut coverage = 0;
                let mut rgb = [0; 3];
                for &(y, wy) in rows {
                    for &(x, wx) in columns {
                        let at = (y * SIZE + x) * 4;
                        if self.rgba[at + 3] == 0 { continue; }
                        let [r, g, b] = [self.rgba[at], self.rgba[at + 1], self.rgba[at + 2]];
                        let ink = if r > 127 { tint.map_or([r, g, b], |c| [c.r, c.g, c.b]) } else { [r, g, b] };
                        let weight = wy * wx;
                        coverage += weight;
                        for k in 0..3 {
                            rgb[k] += u32::from(ink[k]) * weight;
                        }
                    }
                }
                let alpha = (255 * coverage + area / 2) / area;
                if alpha == 0 {
                    pixels.extend_from_slice(&[0; 4]);
                } else {
                    pixels.extend(rgb.map(|v| ((v + coverage / 2) / coverage) as u8));
                    pixels.push(alpha as u8);
                }
            }
        }
        pixels
    }

    fn hotspot_at(&self, side: usize) -> [u16; 2] {
        self.hotspot.map(|p| ((usize::from(p) * side + SIZE / 2) / SIZE).min(side - 1) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vintage_embedded_cursors_decode_without_local_installation() {
        for name in ["32761", "32762", "32763", "32764", "32765", "32766", "32767", "EEBRUSH", "EECUT",
            "EEDEL", "EEIP", "EEPEN", "EESEL", "EESLIDE", "EEZOOM", "MOVE", "MOVEH", "MOVEV",
            "SELDOWN", "SELUP", "SELUPDOWN", "SIZEH"] {
            let bytes = crate::vintage_skin::bundled_asset(&format!("Cursors/{name}.bin")).unwrap();
            let bitmap = decode(bytes).unwrap();
            let exact = bitmap.scaled(32, None);
            let doubled = bitmap.scaled(64, None);
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let index = (SIZE - 1 - y) * 4 + x / 8;
                    let bit = 0x80 >> (x % 8);
                    let and = bytes[180 + index] & bit != 0;
                    let xor = bytes[52 + index] & bit != 0;
                    let color = match (and, xor) {
                        (true, false) => [0; 4],
                        (false, true) => [255; 4],
                        (false, false) => [0, 0, 0, 255],
                        (true, true) => panic!("unexpected inverse pixel in {name}"),
                    };
                    assert_eq!(exact[(y * SIZE + x) * 4..][..4], color, "{name}");
                    for j in 0..2 {
                        for i in 0..2 {
                            assert_eq!(doubled[((y * 2 + j) * 64 + x * 2 + i) * 4..][..4], color, "{name}");
                        }
                    }
                }
            }
            assert_eq!(bitmap.hotspot_at(32), bitmap.hotspot);
            assert_eq!(bitmap.hotspot_at(64), bitmap.hotspot.map(|p| p * 2));
        }
    }

    #[test]
    fn legacy_tool_mapping_and_fractional_cursor_pixels() {
        use tools::{COPYING, GRAB, RESIZE, STRETCH};
        for (cursor, name) in [
            (Cursor::Playlist(PlTool::Draw, 3), "EEPEN"),
            (Cursor::Playlist(PlTool::Paint, 3), "EEBRUSH"),
            (Cursor::Playlist(PlTool::Slice, 3), "EECUT"),
            (Cursor::Playlist(PlTool::Slip, 3), "EESLIDE"),
            (Cursor::Playlist(PlTool::Select, GRAB), "MOVE"),
            (Cursor::Playlist(PlTool::Draw, 4), "32766"),
            (Cursor::Playlist(PlTool::Draw, COPYING), "32763"),
            (Cursor::Playlist(PlTool::Slip, RESIZE), "SIZEH"),
            (Cursor::Playlist(PlTool::Slip, STRETCH), "32765"),
            (Cursor::NsResize, "MOVEV"),
        ] {
            assert_eq!(resource(cursor), Some(name));
            let bitmap = bitmap(cursor).unwrap();
            for scale in [1.25, 1.5, 1.75] {
                let side = (SIZE as f32 * scale) as usize;
                let pixels = bitmap.scaled(side, None);
                assert!(pixels.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255), "{name} at {scale}");
                assert!(pixels.chunks_exact(4).any(|p| *p == [255; 4]), "{name} at {scale}");
                assert!(pixels.chunks_exact(4).all(|p| p[0] == p[1] && p[1] == p[2]));
                let hot = bitmap.hotspot_at(side);
                for (i, pos) in hot.into_iter().enumerate() {
                    assert!((f32::from(pos) - f32::from(bitmap.hotspot[i]) * scale).abs() <= 0.5);
                }
                assert!(source(cursor, scale).is_some());
            }
        }
        let pencil = bitmap(Cursor::Playlist(PlTool::Draw, 3)).unwrap();
        let custom = Color::rgb(0x1085dd);
        assert!(pencil.scaled(40, Some(custom)).chunks_exact(4).any(|p| *p == [0x10,0x85,0xdd,255]));
        assert!(resource(Cursor::Default).is_none());
        assert!(resource(Cursor::NwseResize).is_none());
        assert!(resource(Cursor::Playlist(PlTool::Comp, 3)).is_none());
    }
    #[test]
    fn vintage_cursor_pack_is_default_and_portable() {
        let appearance = os_theme::design::vintage_2005().appearance;
        assert_eq!(tools::resolved_cursor_pack(appearance, Cursor::Default), PACK);
        let appearance = os_theme::fl::FlTheme::parse(
            "LemonStudioStyle.cursor-family=28\nLemonStudioStyle.cursor-1-family=29",
        ).appearance;
        assert_eq!(appearance.cursor_family, Some(PACK));
        assert_eq!(appearance.cursor_variants[0][1], PACK + 1);
    }
}

pub fn preview(canvas: &mut Canvas, cursor: Cursor, rect: Rect) -> bool {
    let Some(bitmap) = bitmap(cursor) else { return false };
    let scale = canvas.scale;
    let side = (rect.w.min(rect.h) * scale).round().clamp(1.0, 128.0) as usize;
    let half = side as f32 * 0.5;
    let (x, y) = ((rect.cx() * scale - half).round() / scale, (rect.cy() * scale - half).round() / scale);
    let rect = Rect::new(x, y, side as f32 / scale, side as f32 / scale);
    canvas.image_rgba(rect, side, side, &bitmap.scaled(side, tint()));
    true
}

pub fn source(cursor: Cursor, scale: f32) -> Option<CustomCursorSource> {
    let bitmap = bitmap(cursor)?;
    let side = (SIZE as f32 * scale.clamp(0.5, 4.0)).round() as usize;
    let [x, y] = bitmap.hotspot_at(side);
    CustomCursor::from_rgba(bitmap.scaled(side, tint()), side as u16, side as u16, x, y).ok()
}
