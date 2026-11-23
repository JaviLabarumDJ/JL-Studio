// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Group, Row, SAct, TileList, combo, g, gf, txt};
use crate::advanced_theme::{self as theme, Area, Row as Edit};
use crate::app::{Action, State, Ui};
use crate::{prefs, vintage, workspace_visibility as visibility};
use os_ui::{Font, Rect};

pub const HEADER: f32 = 76.0;
pub const FOOTER: f32 = 42.0;
pub const STANDARD: &str = "Use standard theme";

fn button(ui: &mut Ui, r: Rect, label: &str, action: SAct, selected: bool) {
    let hover = ui.hit(r, Action::Settings(action), label.to_owned());
    let t = ui.t;
    let face = if selected { t.accent } else if hover { t.button_hi } else { t.button };
    let ink = if selected { os_ui::kit::lemon::ink_on(face) } else { os_ui::kit::button_ink_on(face, t.text) };
    ui.c.fill_rrect(r, 4.0, face);
    ui.c.text_center(Font::new(13), r, ink, label);
}

pub fn reset_slider(ui: &mut Ui, bounds: Rect, label: &'static str) {
    button(ui, bounds, "Reset", SAct::ResetAppearanceSlider(label), false);
}

pub fn header(ui: &mut Ui, s: &State, r: Rect) {
    let search = Rect::new(r.x, r.y + 7.0, r.w - 184.0, 29.0);
    theme::paint_search(ui, s, search);
    let scope = if s.neon_3036 { vintage::NEON_LABEL }
        else if s.vintage_2005 { vintage::LABEL } else { "Standard theme" };
    let label = format!("Editing · {scope}");
    ui.c.text_center(Font::new(12), Rect::new(search.right() + 6.0, search.y, 178.0, search.h), ui.t.text, &label);
    let edit = s.settings.advanced_theme.as_ref();
    let searching = edit.is_some_and(|e| !e.query.text.is_empty() || e.target.is_some());
    let mut x = r.x;
    for (area, width) in Area::ALL.into_iter().zip([76., 58., 64., 75., 93., 67., 69., 78.]) {
        let b = Rect::new(x, r.y + 43.0, width, 25.0);
        button(ui, b, area.label(), SAct::AppearanceArea(area), !searching && s.settings.appearance_area == area);
        x += width + 4.0;
    }
}

pub fn footer(ui: &mut Ui, s: &State, r: Rect) {
    let edit = s.settings.advanced_theme.is_some();
    let mut x = r.x;
    for (label, width) in [(theme::APPLY, 84.), (theme::CANCEL, 89.),
        (super::THEME_UNDO, 56.), (super::THEME_REDO, 56.)] {
        if edit || matches!(label, super::THEME_UNDO | super::THEME_REDO) {
            button(ui, Rect::new(x, r.y + 7.0, width, 28.0), label, SAct::Button(label), label == theme::APPLY);
            x += width + 6.0;
        }
    }
    if !s.vintage_2005 {
        let label = if s.neon_3036 { theme::SAVE_BASE } else { theme::SAVE_NEW };
        button(ui, Rect::new(x, r.y + 7.0, 155.0, 28.0), label, SAct::Button(label), false);
    }
    if edit && s.neon_3036 {
        ui.c.text_right(Font::new(12), r.right(), r.y + 13.0, ui.t.text_dim, "3036 profile settings");
    }
}

fn grid_controls(area: Area, query: &str) -> Option<Group> {
    use crate::playlist_grid::{Kind, SECTION, STYLE, VISIBILITY};
    if !visibility::shown(VISIBILITY) {
        return None;
    }
    let matches = |label, words| {
        if query.is_empty() { area == Area::Playlist }
        else { theme::search_matches(query, label, words, SECTION) }
    };
    let style_matches = matches(STYLE.label, "fine standard thin lines timeline playlist");
    let colors_match = matches("Grid line colors", "colour bars beats subdivisions tracks separators");
    let widths = Kind::ALL.map(Kind::width);
    let matching_widths = widths.into_iter().filter(|width| matches(width.label, "line weight thin thick pixels"));
    let show_custom = prefs::index(&STYLE) == 1;
    let mut rows: Vec<_> = matching_widths.map(|width| combo(prefs::choice(width), 104.0, width.label)).collect();
    if !style_matches && !colors_match && rows.is_empty() { return None; }
    if !show_custom { rows.clear(); }
    else if colors_match { rows.push(Row::GridLineColors); }
    rows.insert(0, combo(prefs::choice(&STYLE), 126.0, STYLE.label));
    rows.insert(0, Row::ThemeEditor(Edit::AreaPreview(Area::Playlist)));
    Some(g(SECTION, rows))
}

fn profile(s: &State, area: Area, query: &str) -> Vec<Group> {
    use Row::{BackgroundTints, Buttons, Check, NeonInks, Slider, ThemeSteps};
    let allow = |area_ok: bool, label, aliases, group| {
        if query.is_empty() { area_ok } else { theme::search_matches(query, label, aliases, group) }
    };
    let mut groups = Vec::new();
    let mut rows = Vec::new();
    if allow(area == Area::Overview, "Featured appearance", "skin look preset vintage neon", "Appearance") {
        rows.push(if s.neon_3036 || visibility::shown(vintage::NEON_VISIBILITY) {
            Row::Check2(vintage::LABEL, s.vintage_2005, vintage::NEON_LABEL, s.neon_3036)
        } else { Check(vintage::LABEL, s.vintage_2005) });
    }
    if s.neon_3036 {
        for (slider, visible, words) in [
            (&prefs::NEON_GLOW, prefs::NEON_GLOW_VISIBILITY, "neon intensity text lighting"),
            (&prefs::NEON_SPREAD, prefs::NEON_SPREAD_VISIBILITY, "neon blur halo"),
        ] {
            if visibility::shown(visible) && allow(area == Area::Overview, slider.label, words, "3036") {
                rows.push(Slider(slider.label, 0.0));
            }
        }
        if visibility::shown(vintage::INKS_VISIBILITY)
            && allow(area == Area::Overview, "Main colors", "accent structure highlights neon", "3036") {
            rows.push(NeonInks);
        }
        if visibility::shown(crate::synthwave::VISIBILITY) {
            if allow(area == Area::Overview, prefs::SYNTHWAVE.label, "playback pulse glow", "3036") {
                rows.push(Check(prefs::SYNTHWAVE.label, false));
            }
            if (s.synthwave.enabled || !query.is_empty())
                && allow(area == Area::Overview, prefs::SYNTHWAVE_SENSITIVITY.label, "playback pulse", "3036") {
                rows.push(Slider(prefs::SYNTHWAVE_SENSITIVITY.label, 0.0));
            }
        }
    }
    if !rows.is_empty() {
        groups.push(g("Featured appearance", rows));
    }
    if s.neon_3036 && visibility::shown(vintage::BACKGROUND_VISIBILITY) {
        let mut rows = Vec::new();
        let backgrounds = area == Area::Overview;
        if allow(backgrounds, vintage::BACKGROUND_TINT, "workspace panel tint color", "3036") {
            rows.push(BackgroundTints);
        }
        if allow(backgrounds || area == Area::Playlist, prefs::BACKGROUND_LIGHT.label,
            "playlist grid darken workspace panel color", "3036") {
            rows.push(Slider(prefs::BACKGROUND_LIGHT.label, 0.0));
        }
        if allow(backgrounds || area == Area::Toolbar, prefs::HEADER_LIGHT.label,
            "toolbar background lighter title bar", "3036") {
            rows.push(Slider(prefs::HEADER_LIGHT.label, 0.0));
        }
        if !rows.is_empty() {
            rows.push(txt("Applies to 3036 throughout the workspace."));
            if backgrounds && query.is_empty() { rows.push(Buttons(vec![(vintage::RESET_BACKGROUNDS, 154.0)])); }
            groups.push(g("3036 backgrounds", rows));
        }
    }
    if s.neon_3036 && allow(matches!(area, Area::Overview | Area::Rack), "Step colors", "even odd channel rack", "3036") {
        groups.push(g("Channel Rack steps", vec![ThemeSteps]));
    }
    let mut shared = Vec::new();
    if visibility::shown(prefs::PLAYLIST_SCROLLBAR_STYLE_VISIBILITY)
        && allow(area == Area::Playlist, prefs::PLAYLIST_SCROLLBAR_STYLE.label, "overview scrollbar", "Playlist") {
        let style = &prefs::PLAYLIST_SCROLLBAR_STYLE;
        shared.push(combo(prefs::choice(style), 154.0, style.label));
    }
    if visibility::shown(crate::theme_adjust::PLAYLIST_BRIGHTNESS_VISIBILITY)
        && allow(area == Area::Playlist, "Playlist clip brightness", "darken clips", "Playlist") {
        shared.push(Row::PlaylistBrightness);
    }
    if !shared.is_empty() { groups.push(g("Playlist display", shared)); }
    if !s.neon_3036 && !s.vintage_2005
        && allow(area == Area::Mixer, prefs::MIXER_STYLE.label, "finish original clean modern analog", "Mixer")
    {
        let text = s.fl_text.as_deref().and_then(|text| os_theme::fl::get_key(text, crate::mixer_skin::FINISH))
            .filter(|value| crate::mixer_skin::Scope::ALL.iter().any(|scope| scope.label() == *value))
            .unwrap_or_else(|| prefs::choice(&prefs::MIXER_STYLE));
        groups.push(g("Mixer style", vec![combo(text, 170.0, prefs::MIXER_STYLE.label)]));
    }
    groups
}

fn edits(rows: impl IntoIterator<Item = Edit>) -> Vec<Row> {
    rows.into_iter().map(Row::ThemeEditor).collect()
}

fn theme_tiles(s: &State, all: bool) -> Group {
    let st = &s.settings;
    let count = if all { st.themes.len() } else { 5 };
    let names = st.themes.iter().take(count).map(|(name, _)| name.clone()).collect();
    g(if all { "Saved and built-in themes" } else { "Choose a theme" }, vec![Row::Tiles(TileList::Themes, names, st.preset)])
}

fn available(s: &State, row: Edit) -> bool {
    if s.vintage_2005 { return false; }
    !s.neon_3036 || matches!(row, Edit::Color("mixer_separator"))
}

fn common(s: &State, area: Area) -> Vec<Edit> {
    if !visibility::shown(theme::QUICK_COLORS_VISIBILITY) { return Vec::new(); }
    area.common().iter().copied().filter(|&key| theme::color_shown(key))
        .map(Edit::Color).filter(|&row| available(s, row)).collect()
}

pub fn page(s: &State) -> Vec<Group> {
    let st = &s.settings;
    let area = st.appearance_area;
    let edit = st.advanced_theme.as_ref();
    let query = edit.map_or("", |edit| edit.query.text.trim());
    if let Some(key) = edit.and_then(|edit| edit.target) {
        let controls = if area == Area::Playlist { grid_controls(area, "") } else { None };
        let rows = if controls.is_some() { edits([Edit::Color(key)]) }
            else { edits([Edit::AreaPreview(area), Edit::Color(key)]) };
        return controls.into_iter().chain([g(area.label(), rows)]).collect();
    }
    let grid = grid_controls(area, query);
    let has_grid = grid.is_some();
    let mut groups: Vec<_> = grid.into_iter().chain(profile(s, area, query)).collect();
    let featured = s.neon_3036 || s.vintage_2005;
    if !query.is_empty() {
        let edit = edit.expect("search session");
        let mut inactive = false;
        for (title, rows) in &edit.groups {
            let controls: Vec<_> = rows.iter().copied().filter(|&row| available(s, row)).collect();
            if !controls.is_empty() { groups.push(g(title, edits(controls))); }
            inactive |= featured && rows.iter().any(|&row| matches!(row, Edit::Color(_) | Edit::Style(_)) && !available(s, row));
        }
        if !featured && theme::search_matches(query, "Interface font icons cursors", "typography text pointer", "Interface") {
            groups.push(g("Interface", vec![Row::Buttons(vec![(crate::theme_identity::OPEN, 138.0)])]));
        }
        if !featured && theme::search_matches(query, "Mixer skin designer", "custom artwork fader knobs buttons meters", "Mixer") {
            groups.push(g("Mixer Skin Designer", vec![Row::Buttons(vec![(crate::mixer_skin::OPEN, 166.0)])]));
        }
        if !featured && theme::search_matches(query, "Import export save theme", "load flstheme preset file", "Theme files") {
            groups.push(files());
        }
        if inactive {
            groups.push(g("Standard theme settings", vec![txt("The featured appearance supplies these colors and styles."), Row::Buttons(vec![(STANDARD, 148.0)])]));
        }
        if groups.is_empty() {
            groups.push(g("Search", vec![txt("No settings match. Try an area or control name.")]));
        }
        return groups;
    }
    if area == Area::Overview {
        if !featured { groups.insert(0, theme_tiles(s, false)); }
        let colors = common(s, area);
        if !colors.is_empty() {
            groups.push(g("Common changes", edits(std::iter::once(Edit::Preview).chain(colors))));
        }
        if st.unreadable.is_some() && !featured { groups.push(g("", vec![Row::ThemeWarn])); }
        if !featured {
            groups.push(gf("Advanced", true, vec![Row::ThemeKnobs]));
        }
        return groups;
    }
    if area == Area::Files {
        if featured {
            groups.push(g("Standard themes", vec![txt("Theme files store the standard appearance."), Row::Buttons(vec![(STANDARD, 148.0)])]));
        } else {
            groups.insert(0, files());
            groups.push(g("Base skin", vec![Row::Tiles(TileList::Skins, os_theme::Design::ALL.iter().map(|d| d.name().into()).collect(), st.skin)]));
            groups.push(theme_tiles(s, true));
        }
        return groups;
    }
    let colors = common(s, area);
    let mut rows = if colors.is_empty() { Vec::new() }
        else { edits((!has_grid).then_some(Edit::AreaPreview(area)).into_iter().chain(colors.iter().copied())) };
    if area == Area::Mixer && !featured {
        rows.push(Row::Buttons(vec![(crate::mixer_skin::OPEN, 166.0)]));
    }
    if area == Area::Interface && !featured {
        rows.push(Row::Buttons(vec![(crate::theme_identity::OPEN, 138.0)]));
    }
    if !rows.is_empty() {
        groups.push(g(area.label(), rows));
    }
    if featured {
        groups.push(g("Standard theme settings", vec![txt("The featured appearance supplies this area's other colors and styles."),
            Row::Buttons(vec![(STANDARD, 148.0)])]));
    } else {
        let all;
        let source = if let Some(edit) = edit { &edit.groups } else {
            all = theme::groups("");
            &all
        };
        for (title, rows) in source.iter().filter(|(title, _)| area.includes(title)) {
            let rows = rows.iter().copied().filter(|row| !matches!(row, Edit::Color(key)
                if colors.iter().any(|color| matches!(color, Edit::Color(common) if common == key))));
            let title = if *title == area.label() { "Advanced" } else { title };
            groups.push(gf(title, true, edits(rows)));
        }
        if area == Area::Playlist {
            groups.push(gf("Clip color options", true, vec![Row::Check(super::NOTE_COLORS_CHECK, s.fl_theme().override_clips)]));
        }
    }
    groups
}

fn files() -> Group {
    g("Theme files", vec![Row::Buttons(vec![("Load theme...", 112.0), (theme::EXPORT, 116.0),
        (theme::WALLPAPER, 140.0), (theme::CLEAR_WALLPAPER, 112.0)]),
        Row::Buttons(vec![("Reset", 74.0)])])
}
