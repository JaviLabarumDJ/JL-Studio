// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen-space light from the visible 3036 artwork. The backing canvas stays unlit so
//! damage repaint, overlapping windows and scrolling never feed yesterday's light back in.
use crate::{text_layer::LetterPixel, Canvas, Color};

type Rgb = [u16; 3];
type Area = (usize, usize, usize, usize);

fn cell(scale: f32) -> usize {
    (3.0 * scale).round().clamp(2.0, 16.0) as usize
}

pub(crate) fn influence(canvas: &Canvas, area: Area) -> Area {
    expanded(
        area,
        canvas.pixmap.width() as usize,
        canvas.pixmap.height() as usize,
        canvas.scale,
        canvas.neon_spread,
    )
}

fn expanded((x, y, w, h): Area, width: usize, height: usize, scale: f32, spread: u8) -> Area {
    // Near kernel radius 1 plus two box blurs and an interpolation cell on either side.
    let reach = if spread > 50 { 11 } else { 7 };
    let edge_radius = scale.round().clamp(1., 4.) as usize;
    let pad = if spread == 0 { edge_radius } else { cell(scale) * reach + edge_radius };
    let left = x.saturating_sub(pad);
    let top = y.saturating_sub(pad);
    (
        left,
        top,
        (x + w + pad).min(width) - left,
        (y + h + pad).min(height) - top,
    )
}

// Separable box kernels. Clamping at the screen edge prevents the bezel from absorbing light.
fn blur(input: &[Rgb], width: usize, height: usize, radius: usize) -> Vec<Rgb> {
    let mut temp = vec![[0; 3]; input.len()];
    let mut dest = temp.clone();
    box_rows(input, &mut temp, height, width, 1, width, radius);
    box_rows(&temp, &mut dest, width, height, width, 1, radius);
    dest
}

fn box_rows(
    src: &[Rgb],
    dst: &mut [Rgb],
    rows: usize,
    cols: usize,
    along: usize,
    across: usize,
    r: usize,
) {
    for row in 0..rows {
        let base = row * across;
        let mut sum = [0u16; 3];
        for i in 0..=2 * r {
            let a = src[base + i.saturating_sub(r).min(cols - 1) * along];
            for k in 0..3 {
                sum[k] += a[k];
            }
        }
        for i in 0..cols {
            let before = src[base + i.saturating_sub(r) * along];
            let after = src[base + (i + r + 1).min(cols - 1) * along];
            let d = &mut dst[base + i * along];
            for k in 0..3 {
                d[k] = sum[k] / (2 * r + 1) as u16;
                sum[k] = sum[k] - before[k] + after[k];
            }
        }
    }
}

// Redisplay the bright side of each surface boundary as a narrow illuminated rail.
// Sampling the original scene (including outside dirty bounds) also picks up custom
// controls, symbols, dividers and disabled widgets drawn by older views.
#[derive(Clone, Copy)]
struct Scene<'a> {
    pixels: &'a [[u8; 4]],
    letters: &'a [LetterPixel],
    width: usize,
    height: usize,
    scale: f32,
    gain: u16,
    spread: u8,
    structure: Option<[u16; 3]>,
    background: Option<ShadowGrade>,
    stock: Option<&'a StockMaterials>,
}

impl Scene<'_> {
    fn lettering(self, index: usize, lit: [u8; 3]) -> [u8; 3] {
        self.letters.get(index).map_or(lit, |letter| letter.over_light(self.pixels[index], lit))
    }
}

// Authored stock editors use everything from palette widgets to painted bitmaps. Map their
// final material by brightness and source hue so the same controls and fonts stay legible.
struct StockMaterials {
    ramps: [[[u8; 3]; 256]; 4],
    header: u8,
}

impl StockMaterials {
    fn new(look: os_theme::design::StockEditorLook) -> Self {
        let structure = look.inks[1];
        let base = [structure.r, structure.g, structure.b].map(|v| 4 + v / 24);
        let ramps = std::array::from_fn(|role| std::array::from_fn(|level| {
            let t = level as f32 / 255.0;
            let ink = look.inks[role.min(2)];
            let ink = [ink.r, ink.g, ink.b];
            std::array::from_fn(|k| {
                let start = f32::from(base[k]);
                let end = f32::from(ink[k]);
                let value = if role == 3 {
                    // Broad metal faces become violet glass. Pale labels retain white contrast.
                    start + t * (0.55 + 0.45 * t) * (244.0 - start) + t * f32::from([structure.r, structure.g, structure.b][k]) * (0.035 + 0.25 * (1.0 - t))
                } else {
                    start + t * (end - start)
                };
                value.round().clamp(0.0, 255.0) as u8
            })
        }));
        Self { ramps, header: look.background.headers.min(100) * 2 / 5 }
    }

    fn pixel(&self, p: [u8; 4], top: bool) -> [u8; 4] {
        let [r,g,b,_] = p;
        let peak = r.max(g).max(b);
        let low = r.min(g).min(b);
        let chroma = u32::from(peak - low);
        let neutral = self.ramps[3][peak as usize];
        let extra = if top { u32::from(self.header) * u32::from(128u8.saturating_sub(peak)) / 128 } else { 0 };
        let mut out = p;
        // Crossfade hue families through antialiased boundaries. The stock blue used for
        // active controls belongs with cyan, while deep blues and purples use structure.
        let green = u32::from(g - low);
        let blue = u32::from(b - low);
        let cyan_blue = blue.min(2 * green);
        let weights = [green + cyan_blue, blue - cyan_blue, u32::from(r - low)];
        let total = weights.iter().sum::<u32>();
        for k in 0..3 {
            let paper = u32::from(neutral[k]);
            let value = if total == 0 {
                paper
            } else {
                let ink = weights.iter().zip(&self.ramps).map(|(w, ramp)| w * u32::from(ramp[peak as usize][k])).sum::<u32>() / total;
                let signal = (ink * 7 + u32::from(p[k])) / 8;
                // Preserve the neutral portion of the source. Pastel lettering and the
                // inside edge of a white label must not turn into saturated light.
                (signal * chroma + paper * u32::from(low)) / u32::from(peak)
            };
            out[k] = (value + extra).min(255) as u8;
        }
        out
    }
}

// Grade the presented shadows after extracting their light. Background lift must not
// create new bloom emitters, nor feed back into damage redraw or scope history.
#[derive(Clone, Copy)]
struct ShadowGrade {
    gains: Option<[u16; 3]>,
    lift: u16,
}

impl ShadowGrade {
    fn new(bg: os_theme::design::NeonBackground) -> Option<Self> {
        if bg.tint.is_none() && bg.brightness == 0 {
            return None;
        }
        let gains = bg.tint.map(|color| {
            let channels = [color.r, color.g, color.b].map(u32::from);
            let mean = (channels[0] + channels[1] * 2 + channels[2]) / 4;
            channels.map(|v| ((v + 16) * 256 / (mean + 16)).min(640) as u16)
        });
        // A small floor lets a selected tint show up even on a pure black panel.
        let tint_floor = if gains.is_some() { 6 } else { 0 };
        let lift = u16::from(bg.brightness.min(100)) * 60 / 100 + tint_floor;
        Some(Self { gains, lift })
    }

    fn apply(self, mut p: [u8; 4]) -> [u8; 4] {
        let peak = p[0].max(p[1]).max(p[2]);
        if peak >= 128 {
            return p;
        }
        let weight = u32::from((128 - peak).min(64));
        let luma = (u32::from(p[0]) + 2 * u32::from(p[1]) + u32::from(p[2])) / 4;
        let lift = u32::from(self.lift);
        for k in 0..3 {
            let old = u32::from(p[k]);
            let lit = old + lift;
            let target = self.gains.map_or(lit, |gain| {
                // Keep some of the original channel colour underneath the chosen tint.
                (lit + 3 * ((luma + lift) * u32::from(gain[k]) / 256)) / 4
            });
            p[k] = ((old * (64 - weight) + target.min(255) * weight + 32) / 64) as u8;
        }
        p
    }
}

fn neutral_glass(hi: u8, low: u8) -> bool {
    hi - low < 18 && (24..155).contains(&hi)
}

fn glass_tint(hi: u8, structure: Option<Rgb>) -> [u8; 4] {
    let [r, g, b] = if let Some(ink) = structure {
        ink.map(|channel| ((u16::from(hi) * (42 + channel / 2)) >> 8) as u8)
    } else {
        let edge = (u16::from(hi) * 3 / 5) as u8;
        [edge, hi / 4, edge]
    };
    [r, g, b, 255]
}

pub(crate) fn flat_surface(canvas: &Canvas, color: Color) -> Color {
    let hi = color.r.max(color.g).max(color.b);
    let low = color.r.min(color.g).min(color.b);
    let pixel = if neutral_glass(hi, low) {
        glass_tint(hi, canvas.neon_structure.map(|c| [c.r, c.g, c.b].map(u16::from)))
    } else {
        [color.r, color.g, color.b, 255]
    };
    let [r, g, b, _] = ShadowGrade::new(canvas.neon_background).map_or(pixel, |grade| grade.apply(pixel));
    Color { r, g, b, ..color }
}

fn rails(scene: Scene<'_>, (x, y, w, h): Area) -> Vec<[u8; 4]> {
    let Scene {
        width: pw,
        height: ph,
        scale,
        gain,
        structure,
        ..
    } = scene;
    let radius = scale.round().clamp(1., 4.) as usize;
    // Restore the surface under each glyph once per band, including the few
    // neighboring pixels sampled by the edge detector.
    let restored;
    let (left, top, width, height, pixels) = if scene.letters.is_empty() {
        (0, 0, pw, ph, scene.pixels)
    } else {
        let left = x.saturating_sub(radius);
        let top = y.saturating_sub(radius);
        let right = (x + w + radius).min(pw);
        let bottom = (y + h + radius).min(ph);
        let mut surface = Vec::with_capacity((right - left) * (bottom - top));
        for row in top..bottom {
            let span = row * pw + left..row * pw + right;
            surface.extend(scene.pixels[span.clone()].iter().zip(&scene.letters[span])
                .map(|(&pixel, letter)| letter.surface(pixel)));
        }
        restored = surface;
        (left, top, right - left, bottom - top, restored.as_slice())
    };
    // The surface boundary emits light even at minimum intensity. Flat faces keep their grade.
    let edge_gain = 96 + gain * 7 / 10;
    let mut result = Vec::with_capacity(w * h);
    let peak = |p: [u8; 4]| p[0].max(p[1]).max(p[2]);
    for py in y..y + h {
        let local_y = py - top;
        let row = &pixels[local_y * width..][..width];
        let up = &pixels[local_y.saturating_sub(radius) * width..][..width];
        let down = &pixels[(local_y + radius).min(height - 1) * width..][..width];
        for px in x - left..x - left + w {
            let raw = row[px];
            let p = scene.stock.map_or(raw, |m| m.pixel(raw, py < (24.0 * scale) as usize));
            let hi = peak(p);
            let low = p[0].min(p[1]).min(p[2]);
            let neutral = scene.stock.is_none() && neutral_glass(hi, low);
            let mut glass = if neutral { glass_tint(hi, structure) } else { p };
            if hi < 202 {
                let dark = peak(row[px.saturating_sub(radius)])
                    .min(peak(row[(px + radius).min(width - 1)]))
                    .min(peak(up[px]))
                    .min(peak(down[px]));
                let slope = u16::from(peak(raw).saturating_sub(dark));
                if slope > 0 {
                    let power = ((slope * 2).min(41) + slope.min(39))
                        * u16::from((202 - hi).min(104))
                        / 104;
                    // Unassigned metal and graphite use violet tubing. Coloured controls
                    // retain the hue of their track, group or operating state.
                    let tube = if let Some(ink) = structure {
                        if hi - low < 12 {
                            ink
                        } else {
                            [p[0], p[1], p[2]].map(|v| u16::from(v) * 255 / u16::from(hi.max(1)))
                        }
                    } else if p[1].saturating_sub(low) > 24 && p[1] >= p[0] {
                        [48, 255, if p[2] >= p[1] { 255 } else { 175 }]
                    } else if neutral || p[2] > p[0].saturating_add(4) {
                        [145, 68, 255]
                    } else if p[0] > p[2].saturating_add(25) {
                        [255, 100, 127]
                    } else {
                        [255, 48, 229]
                    };
                    let power = (power * edge_gain) >> 8;
                    // Subpixel-sized neutral transitions in stock artwork are commonly
                    // lettering. Keep their coverage instead of tracing a second glyph.
                    let power = if scene.stock.is_some() {
                        let chroma = peak(raw) - raw[0].min(raw[1]).min(raw[2]);
                        // Preserve the violet rails on dark panel bevels. A large luminance
                        // step is generally the antialiased edge of a letter or symbol.
                        let bevel = 95u8.saturating_sub(peak(raw)).saturating_mul(4)
                            .min(40u8.saturating_sub(slope as u8).saturating_mul(7));
                        power * u16::from(chroma.max(bevel)) / 255
                    } else {
                        power
                    };
                    for k in 0..3 {
                        glass[k] = glass[k].saturating_add((tube[k] * power / 255) as u8);
                    }
                }
            }
            result.push(glass);
        }
    }
    result
}

pub(crate) fn present(
    canvas: &Canvas,
    out: &mut [u32],
    stride: usize,
    area: Area,
    (ox, oy): (usize, usize),
) {
    let (_, _, w, h) = area;
    if w == 0 || h == 0 {
        return;
    }
    let stock = canvas.stock_editor.map(StockMaterials::new);
    let scene = Scene {
        stock: stock.as_ref(),
        pixels: canvas.pixmap.data().as_chunks::<4>().0,
        letters: if stock.is_none() { &canvas.neon_text.pixels } else { &[] },
        width: canvas.pixmap.width() as usize,
        height: canvas.pixmap.height() as usize,
        scale: canvas.scale,
        spread: canvas.neon_spread.min(100),
        gain: u16::from(canvas.neon_glow_percent.min(125)) * 256 / 100,
        background: ShadowGrade::new(canvas.neon_background),
        structure: canvas
            .neon_structure
            .map(|c| [c.r, c.g, c.b].map(u16::from)),
    };
    present_scene(scene, out, stride, area, (ox, oy));
}

pub(crate) fn present_stock_rgba(pixels: &[[u8; 4]], out: &mut [u32], width: usize, height: usize,
                                 scale: f32, look: os_theme::design::StockEditorLook) {
    let stock = StockMaterials::new(look);
    let ink = look.inks[1];
    let scene = Scene {
        pixels, letters: &[], width, height, scale, stock: Some(&stock),
        spread: look.spread.min(100), gain: u16::from(look.glow.min(125)) * 256 / 100,
        background: ShadowGrade::new(look.background),
        structure: Some([ink.r, ink.g, ink.b].map(u16::from)),
    };
    present_scene(scene, out, width, (0, 0, width, height), (0, 0));
}

fn present_scene(scene: Scene<'_>, out: &mut [u32], stride: usize, area: Area, (ox, oy): (usize, usize)) {
    let (x, y, w, h) = area;
    let Some(dest) = out.get_mut((y + oy) * stride..) else {
        return;
    };
    let needed = (h - 1) * stride + x + ox + w;
    let Some(dest) = dest.get_mut(..needed) else {
        return;
    };
    // Only large redraws occupy helper threads. Meter and cursor damage stay local;
    // workers see immutable pixels and never access GUI objects or the audio engine.
    static WORKERS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let workers = if w * h >= 800_000 {
        *WORKERS.get_or_init(|| {
            std::thread::available_parallelism().map_or(1, |n| (n.get() / 3).clamp(1, 3))
        })
    } else {
        1
    };
    if workers == 1 {
        return present_band(scene, dest, stride, area, ox);
    }
    let rows = h.div_ceil(workers);
    std::thread::scope(|scope| {
        let mut bands = dest.chunks_mut(rows * stride).enumerate();
        let (_, first) = bands.next().unwrap();
        for (i, band) in bands {
            let start = i * rows;
            scope.spawn(move || {
                present_band(
                    scene,
                    band,
                    stride,
                    (x, y + start, w, rows.min(h - start)),
                    ox,
                )
            });
        }
        present_band(scene, first, stride, (x, y, w, rows), ox);
    });
}

fn present_band(scene: Scene<'_>, out: &mut [u32], stride: usize, area: Area, ox: usize) {
    let (x, y, w, h) = area;
    let (pw, ph) = (scene.width, scene.height);
    if scene.spread == 0 || scene.gain == 0 {
        let glass = rails(scene, area);
        for (row, src) in glass.chunks_exact(w).enumerate() {
            let start = row * stride + x + ox;
            for (col, (d, p)) in out[start..start + w].iter_mut().zip(src).enumerate() {
                let p = scene.background.map_or(*p, |grade| grade.apply(*p));
                let [r, g, b] = scene.lettering((y + row) * pw + x + col, [p[0], p[1], p[2]]);
                *d = u32::from_be_bytes([0, r, g, b]);
            }
        }
        return;
    }
    let size = cell(scene.scale);
    let (l, t, aw, ah) = expanded(area, pw, ph, scene.scale, scene.spread);
    let (gx, gy) = (l / size, t / size);
    let (gw, gh) = ((l + aw).div_ceil(size) - gx, (t + ah).div_ceil(size) - gy);
    let (left, top) = (gx * size, gy * size);
    let width = ((gx + gw) * size).min(pw) - left;
    let height = ((gy + gh) * size).min(ph) - top;
    let pixels = rails(scene, (left, top, width, height));
    let mut emission = vec![[0; 3]; gw * gh];
    for j in 0..gh {
        let tile_y = j * size;
        for i in 0..gw {
            let tile_x = i * size;
            let mut high = [0u16; 3];
            // Peak pooling preserves the radiance of single-pixel scope traces and knob arcs.
            for py in tile_y..(tile_y + size).min(height) {
                for p in &pixels[py * width + tile_x..py * width + (tile_x + size).min(width)] {
                    let peak = p[0].max(p[1]).max(p[2]);
                    if peak < 85 {
                        continue;
                    }
                    let low = p[0].min(p[1]).min(p[2]);
                    let chroma = peak - low;
                    if chroma < 48 {
                        continue;
                    }
                    for k in 0..3 {
                        let light = u16::from(p[k].saturating_sub(low / 2))
                            * u16::from(
                                (peak - 60)
                                    .saturating_mul(2)
                                    .min(chroma.saturating_sub(30).saturating_mul(2)),
                            )
                            >> 8;
                        high[k] = high[k].max(light);
                    }
                }
            }
            emission[j * gw + i] = high;
        }
    }
    let near = blur(&emission, gw, gh, 1);
    let blurred = |radius| blur(&blur(&near, gw, gh, radius), gw, gh, radius);
    let standard = blurred(2);
    let wide = (scene.spread > 50).then(|| blurred(4));
    let mix = |a: u16, b: u16, t: u8| (a * u16::from(50 - t) + b * u16::from(t)) / 50;
    // Use only filtered light for the halo. Limit its peak across RGB together
    // so neighboring emitters keep their hue and leave headroom in the gaps.
    let strength = u32::from(scene.gain) * u32::from(scene.spread);
    for (i, e) in emission.iter_mut().enumerate() {
        let field: Rgb = std::array::from_fn(|k| match &wide {
            Some(w) => mix(standard[i][k], w[i][k], scene.spread - 50),
            None => mix(near[i][k], standard[i][k], scene.spread),
        });
        let peak = u32::from(*field.iter().max().unwrap()).max(1);
        let limit = (peak * strength / 25_600).min(32);
        *e = field.map(|v| (u32::from(v) * limit / peak) as u16);
    }
    // Interpolate from cell centres, keeping the grid anchored to device coordinates
    // so recomposing a damaged rectangle is pixel-identical to a full presentation.
    let coord = |v: usize, start: usize, count: usize| {
        let pos = (v * 2 + 1).saturating_sub(size);
        let index = (pos / (size * 2)).saturating_sub(start).min(count - 1);
        (
            index,
            (index + 1).min(count - 1),
            ((pos % (size * 2)) * 256 / (size * 2)) as u32,
        )
    };
    let xs: Vec<_> = (x..x + w).map(|px| coord(px, gx, gw)).collect();
    let mut scanline = vec![[0u32; 3]; gw];
    for py in y..y + h {
        let (a, b, fy) = coord(py, gy, gh);
        for ((light, upper), lower) in scanline
            .iter_mut()
            .zip(&emission[a * gw..])
            .zip(&emission[b * gw..])
        {
            for k in 0..3 {
                light[k] = (u32::from(upper[k]) * (256 - fy) + u32::from(lower[k]) * fy) >> 8;
            }
        }
        let start = (py - y) * stride + x + ox;
        let Some(row) = out.get_mut(start..start + w) else {
            return;
        };
        for (col, ((d, p), &(i, j, fx))) in row
            .iter_mut()
            .zip(&pixels[(py - top) * width + x - left..])
            .zip(&xs)
            .enumerate()
        {
            let p = scene.background.map_or(*p, |grade| grade.apply(*p));
            let lit = std::array::from_fn(|k| {
                let glow = (scanline[i][k] * (256 - fx) + scanline[j][k] * fx) >> 8;
                let raw = u32::from(p[k]);
                (raw + ((255 - raw) * glow >> 8)) as u8
            });
            let [r, g, b] = scene.lettering(py * pw + x + col, lit);
            *d = u32::from_be_bytes([0, r, g, b]);
        }
    }
}
