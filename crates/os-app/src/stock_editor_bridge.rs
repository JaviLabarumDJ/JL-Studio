// SPDX-License-Identifier: GPL-3.0-or-later
//! UI-thread ABI shared with the bundled native editors. A null buffer only queries the look.
// An OpenGL child does not implement Windows PrintWindow. Only the isolated stock
// preview helper opts in to receiving the editor's own output before the buffer swap.
type Frame = (usize, usize, Vec<u32>);
struct Preview {
    ready: std::time::Instant,
    frame: std::sync::Mutex<Option<Frame>>,
}
static PREVIEW: std::sync::OnceLock<Preview> = std::sync::OnceLock::new();

#[cfg(windows)]
pub(crate) fn request_preview() {
    PREVIEW.get_or_init(|| Preview {
        ready: std::time::Instant::now() + std::time::Duration::from_millis(650),
        frame: Default::default(),
    });
}

#[cfg(windows)]
pub(crate) fn take_preview() -> Option<Frame> {
    PREVIEW.get()?.frame.lock().ok()?.take()
}

fn look_key(look: os_theme::design::StockEditorLook) -> u64 {
    let mut key = 14695981039346656037u64;
    let tint = look.background.tint.unwrap_or(os_theme::Color::rgb(0));
    for byte in look.inks.into_iter().chain([tint]).flat_map(|c| [c.r,c.g,c.b])
        .chain([look.glow, look.spread, look.background.brightness, look.background.headers,
                u8::from(look.background.tint.is_some())]) {
        key = (key ^ u64::from(byte)).wrapping_mul(1099511628211);
    }
    key.max(1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ml_stock_editor_pixels_v1(pixels: *mut u8, width: u32, height: u32,
                                                    stride: u32, scale: f32, layout: u32) -> u64 {
    let look = os_theme::stock_editor_look();
    let preview = PREVIEW.get();
    if look.is_none() && preview.is_none() { return 0; }
    let key = look.map_or(1, look_key);
    if !pixels.is_null() {
        let (w, h, stride) = (width as usize, height as usize, stride as usize);
        if w == 0 || h == 0 || w > 16384 || h > 16384 || w * h > 16_777_216
            || stride < 4 * w || stride > 4 * w + 4096 || !scale.is_finite() { return 0; }
        // Caller owns stride * h writable bytes until the synchronous call returns.
        let bytes = unsafe { std::slice::from_raw_parts_mut(pixels, stride * h) };
        let bgra = layout & 1 != 0;
        let bottom_up = layout & 2 != 0;
        if let Some(look) = look {
            os_ui::stock_pixels(bytes, w, h, stride, scale.clamp(0.5, 8.0), bgra, bottom_up, look);
        }
        if let Some(preview) = preview.filter(|p| std::time::Instant::now() >= p.ready)
            && let Ok(mut frame) = preview.frame.try_lock()
            && frame.is_none() {
            let mut rgb = Vec::with_capacity(w * h);
            for y in 0..h {
                let row = if bottom_up { h - 1 - y } else { y };
                for p in bytes[row * stride..row * stride + w * 4].chunks_exact(4) {
                    rgb.push(if bgra { u32::from_be_bytes([0,p[2],p[1],p[0]]) }
                             else { u32::from_be_bytes([0,p[0],p[1],p[2]]) });
                }
            }
            *frame = Some((w, h, rgb));
        }
    }
    key.max(1)
}
