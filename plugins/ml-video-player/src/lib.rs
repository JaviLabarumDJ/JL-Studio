//! Picture fitting and source-frame stepping adapted from DAWvid by Jon Waterschoot
//! (1ba1c57c1f1fc4fef327060966580af424bbb6bd). See DAWvid-LICENSE.txt.
mod appearance;
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui, resizable_window::ResizableWindow, EguiState};
use std::sync::{Arc, LazyLock, Mutex, Weak};

pub const NAME: &str = "ML Video Player";
pub const ID: &str = "de.michaellane.ml-video-player";
pub const VISIBILITY: &str = "visible_plugin-video-player";
pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 360;

#[derive(Default, Clone)]
pub struct Picture {
    pub pixels: Option<Arc<[u32]>>,
    pub title: String,
    pub seconds: f64,
    pub end: f64,
    pub fps: f64,
    pub playing: bool,
    pub message: String,
}

pub enum Request {
    Import,
    Toggle,
    Step(i8),
    Seek(f64),
}

#[derive(Default)]
struct Bridge {
    windows: Vec<Weak<EguiState>>,
    picture: Picture,
    requests: Vec<Request>,
}

// Only the app UI and plugin editor touch the bridge. Audio passes straight through.
static BRIDGE: LazyLock<Mutex<Bridge>> = LazyLock::new(Default::default);

pub fn monitoring() -> bool {
    let mut bridge = BRIDGE.lock().unwrap();
    bridge.windows.retain(|w| w.strong_count() != 0);
    let visible = bridge
        .windows
        .iter()
        .filter_map(Weak::upgrade)
        .any(|w| w.is_open());
    if !visible && bridge.picture.pixels.is_some() {
        bridge.picture = Picture::default();
    }
    visible
}

pub fn publish(picture: Picture) {
    BRIDGE.lock().unwrap().picture = picture;
}
pub fn take_requests() -> Vec<Request> {
    std::mem::take(&mut BRIDGE.lock().unwrap().requests)
}
fn request(cmd: Request) {
    let mut bridge = BRIDGE.lock().unwrap();
    if matches!(cmd, Request::Seek(_)) && matches!(bridge.requests.last(), Some(Request::Seek(_))) {
        bridge.requests.pop();
    }
    if bridge.requests.len() < 64 {
        bridge.requests.push(cmd);
    }
}

#[derive(Params)]
struct PlayerParams {
    #[persist = "window-size"]
    window: Arc<EguiState>,
}
struct VideoPlayer {
    params: Arc<PlayerParams>,
}
impl Default for VideoPlayer {
    fn default() -> Self {
        let window = EguiState::from_size(640, 440);
        Self {
            params: Arc::new(PlayerParams { window }),
        }
    }
}

#[derive(Default)]
struct Monitor {
    palette: Option<Arc<os_theme::Palette>>,
    source: Option<Arc<[u32]>>,
    texture: Option<egui::TextureHandle>,
}
impl Monitor {
    fn draw(&mut self, ui: &mut egui::Ui, picture: &Picture, palette: &os_theme::Palette) {
        if let Some(pixels) = &picture.pixels {
            if self
                .source
                .as_ref()
                .is_none_or(|last| !Arc::ptr_eq(last, pixels))
            {
                let bytes: Vec<u8> = pixels
                    .iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
                    .collect();
                let image = egui::ColorImage::from_rgb([WIDTH, HEIGHT], &bytes);
                if let Some(texture) = &mut self.texture {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.texture = Some(ui.ctx().load_texture(
                        "reference picture",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                self.source = Some(pixels.clone());
            }
        } else {
            self.source = None;
        }
        ui.horizontal(|ui| {
            if appearance::button(ui, palette, "Open video…", false).clicked() {
                request(Request::Import);
            }
            if appearance::button(
                ui,
                palette,
                if picture.playing { "Pause" } else { "Play" },
                picture.playing,
            )
            .clicked()
            {
                request(Request::Toggle);
            }
            if appearance::button(ui, palette, "◀", false)
                .on_hover_text("Previous frame")
                .clicked()
            {
                request(Request::Step(-1));
            }
            if appearance::button(ui, palette, "▶", false)
                .on_hover_text("Next frame")
                .clicked()
            {
                request(Request::Step(1));
            }
            ui.label(format!("{:.3} s", picture.seconds));
        });
        ui.horizontal(|ui| {
            ui.label(if picture.title.is_empty() {
                "Playlist video"
            } else {
                &picture.title
            })
            .on_hover_text(include_str!("../DAWvid-LICENSE.txt"));
            if picture.fps > 0.0 {
                ui.label(format!("{:.3} fps", picture.fps));
            }
        });
        let mut position = picture.seconds;
        let end = picture.end.max(position);
        ui.spacing_mut().slider_width = ui.available_width().max(1.0);
        if ui
            .add_enabled(
                picture.end > 0.0,
                egui::Slider::new(&mut position, 0.0..=end).show_value(false),
            )
            .changed()
        {
            request(Request::Seek(position));
        }
        let (area, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        appearance::bezel(ui, area.shrink(2.), palette);
        let area = area.shrink(4.);
        ui.painter().rect_filled(area, 0.0, egui::Color32::BLACK);
        if let (Some(_), Some(texture)) = (&self.source, &self.texture) {
            let size = egui::vec2(WIDTH as f32, HEIGHT as f32);
            let scale = (area.width() / size.x).min(area.height() / size.y);
            ui.painter().image(
                texture.id(),
                egui::Rect::from_center_size(area.center(), size * scale),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            ui.painter().text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                &picture.message,
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            );
        }
    }
}

impl Plugin for VideoPlayer {
    const NAME: &'static str = NAME;
    const VENDOR: &'static str = "Michael Lane";
    const URL: &'static str = "https://michaellanemusic.de/ml-studio/";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    type SysExMessage = ();
    type BackgroundTask = ();
    fn params(&self) -> Arc<dyn nih_plug::prelude::Params> {
        self.params.clone()
    }
    fn editor(&mut self, _: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let window = self.params.window.clone();
        BRIDGE.lock().unwrap().windows.push(Arc::downgrade(&window));
        create_egui_editor(
            window.clone(),
            Monitor::default(),
            |_, _| {},
            move |ctx, _, monitor| {
                let picture = BRIDGE.lock().unwrap().picture.clone();
                let palette = os_theme::current();
                if monitor
                    .palette
                    .as_ref()
                    .is_none_or(|previous| !Arc::ptr_eq(previous, &palette))
                {
                    ctx.set_visuals(appearance::visuals(&palette));
                    monitor.palette = Some(palette.clone());
                }
                ResizableWindow::new("ml-video-player")
                    .min_size(egui::vec2(360.0, 230.0))
                    .show(ctx, &window, |ui| {
                        monitor.draw(ui, &picture, &palette);
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            },
        )
    }
    fn process(
        &mut self,
        _: &mut Buffer,
        _: &mut AuxiliaryBuffers,
        _: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}
impl ClapPlugin for VideoPlayer {
    const CLAP_ID: &'static str = ID;
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Playlist-synchronized video monitor");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Utility,
        ClapFeature::Stereo,
    ];
}
os_plugkit::linked_clap_entry!(VideoPlayer);
