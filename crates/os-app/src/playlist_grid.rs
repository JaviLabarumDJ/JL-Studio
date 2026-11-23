// SPDX-License-Identifier: GPL-3.0-or-later
//! Profile-owned appearance of the optional fine Playlist grid. Snap and layout belong to the Playlist.

use crate::prefs::Combo;
use crate::settings_file::{self, Store};
use os_theme::{Color, Palette};

pub const VISIBILITY: &str = "visible_playlist-grid-appearance";
pub const SECTION: &str = "Playlist grid lines";
pub const STYLE: Combo = Combo {
    key: "playlist_grid_style",
    label: "Playlist grid style",
    choices: &["Standard", "Fine"],
    default: 0,
};

fn is_fine(settings: &Store) -> bool {
    settings.get(STYLE.key) == Some(STYLE.choices[1])
}

pub fn enabled() -> bool {
    settings_file::read(is_fine)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bar,
    Beat,
    Track,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Bar, Self::Beat, Self::Track];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Bar => "Bar lines",
            Self::Beat => "Beats and subdivisions",
            Self::Track => "Track separators",
        }
    }

    pub const fn color_key(self) -> &'static str {
        match self {
            Self::Bar => "playlist_grid_bar_color",
            Self::Beat => "playlist_grid_beat_color",
            Self::Track => "playlist_grid_track_color",
        }
    }

    pub fn from_color_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.color_key() == key)
    }

    pub const fn width(self) -> &'static Combo {
        match self {
            Self::Bar => &BAR_WIDTH,
            Self::Beat => &BEAT_WIDTH,
            Self::Track => &TRACK_WIDTH,
        }
    }

    fn inherited_color(self, t: &Palette) -> Color {
        t.pl_line(match self {
            Self::Bar if t.style.neon_3036 => t.grid.mix(t.grid_bar, 0.53),
            Self::Bar => t.grid_bar,
            Self::Beat => t.grid_gap.mix(t.grid, 0.4),
            Self::Track if t.style.neon_3036 => t.grid.mix(t.grid_gap, 0.62),
            Self::Track => t.grid_gap,
        })
    }

    fn stored_color(self, settings: &Store) -> Option<Color> {
        settings.get(self.color_key()).and_then(Color::parse)
    }

    pub fn displayed_color(self, t: &Palette) -> Color {
        settings_file::read(|st| {
            self.stored_color(st)
                .unwrap_or_else(|| self.inherited_color(t))
        })
    }

    pub fn has_color(self) -> bool {
        settings_file::read(|st| self.stored_color(st).is_some())
    }

    pub fn set_color(self, color: Option<Color>) {
        let value = color
            .map(|c| format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b))
            .unwrap_or_default();
        settings_file::write(self.color_key(), &value);
    }
}

const WIDTHS: &[&str] = &["1 px", "2 px", "3 px", "4 px"];
pub const BAR_WIDTH: Combo = Combo {
    key: "playlist_grid_bar_width",
    label: "Bar line thickness",
    choices: WIDTHS,
    default: 0,
};
pub const BEAT_WIDTH: Combo = Combo {
    key: "playlist_grid_beat_width",
    label: "Beat line thickness",
    choices: WIDTHS,
    default: 0,
};
pub const TRACK_WIDTH: Combo = Combo {
    key: "playlist_grid_track_width",
    label: "Track separator thickness",
    choices: WIDTHS,
    default: 0,
};

#[derive(Clone, Copy)]
pub struct Line {
    pub color: Color,
    pixels: u8,
}

impl Line {
    pub fn width(self, scale: f32) -> f32 {
        f32::from(self.pixels) / scale
    }

    /// Device-aligned edges around a timeline or row boundary. The midpoint
    /// can differ from the boundary by at most half a device pixel.
    pub fn span(self, boundary: f32, scale: f32) -> (f32, f32) {
        let width = f32::from(self.pixels);
        let start = (boundary * scale - width / 2.0 + 0.5).floor();
        (start / scale, (start + width) / scale)
    }
}

#[derive(Clone, Copy)]
pub struct Fine {
    pub bar: Line,
    pub beat: Line,
    pub track: Line,
}

impl Fine {
    pub fn load(t: &Palette) -> Option<Self> {
        settings_file::read(|st| {
            if !is_fine(st) {
                return None;
            }
            let line = |kind: Kind| {
                let pixels = WIDTHS
                    .iter()
                    .position(|&w| Some(w) == st.get(kind.width().key))
                    .map_or(1, |i| i as u8 + 1);
                Line {
                    color: kind
                        .stored_color(st)
                        .unwrap_or_else(|| kind.inherited_color(t)),
                    pixels,
                }
            };
            Some(Self {
                bar: line(Kind::Bar),
                beat: line(Kind::Beat),
                track: line(Kind::Track),
            })
        })
    }
}
