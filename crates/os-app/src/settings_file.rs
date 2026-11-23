// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The program's choices that outlive a run: one file in the user folder, `settings.txt`, never the project. Every
//! app-wide choice the user makes and should find again at the next start is kept here under a stable key; a new
//! feature adds keys, not a file of its own.
//!
//! The file is `key=value` lines. A key's value is its last line's; setting one replaces its first line (dropping any
//! later one) or adds a line at the end; lines of keys this version does not know are written back as they were, so an
//! older or newer Lemon Studio keeps what the other wrote. A missing or unreadable value is the reader's default. The
//! file is read once, at the first read, and written whole whenever a value changes. Tests never read or write the user folder: each test thread has a store of its own, empty at its start.
//!
//! Keys so far: `toolbar_separators`, `toolbar_flat_buttons`, `toolbar_hint_bar` (`views::toolbar::Opts`),
//! `clock_mode`, `clock_middle_unit` (`clock::Clock`), `step_edit` (`crate::step_edit`), the `window_*` positions
//! (`crate::layout`), `factory_folder` (`crate::factory_folder`), the settings window's declared controls
//! (`crate::prefs`), and `recent_project_1` .. `recent_project_50` (`crate::recent`).

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock}; // This code is a part of L e m o n S t u d i o.

type Preview = BTreeMap<&'static str, Option<String>>;

/// The file's lines as read: lines of keys this version does not know are written back as they were.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Store {
    lines: Vec<String>,
    appearance: Option<Preview>,
}

impl Store {
    pub(crate) fn parse(text: &str) -> Store {
        Store { lines: text.lines().map(String::from).collect(), appearance: None }
    }

    fn text(&self) -> String {
        self.lines.iter().map(|l| format!("{l}\n")).collect()
    }

    /// The value of `key`: its last line's.
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        if let Some(value) = self.appearance.as_ref().and_then(|values| values.get(key)) {
            return value.as_deref();
        }
        self.lines.iter().rev().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim())
    }

    fn update(&mut self, key: &str, value: &str) -> bool {
        if let Some(pending) = self.appearance.as_mut().and_then(|values| values.get_mut(key)) {
            *pending = (!value.is_empty()).then(|| value.to_owned());
            return false;
        }
        if self.get(key) == Some(value) { return false; }
        self.set(key, value);
        true
    }

    fn apply_appearance(&mut self) -> bool {
        let Some(pending) = self.appearance.take() else { return false };
        let mut changed = false;
        for (key, value) in pending {
            changed |= self.restore_appearance_value(key, value.as_deref());
        }
        changed
    }

    fn restore_appearance_value(&mut self, key: &str, value: Option<&str>) -> bool {
        if self.appearance.as_ref().is_some_and(|pending| pending.contains_key(key)) {
            return self.update(key, value.unwrap_or(""));
        }
        if self.get(key).filter(|v| !v.is_empty()) == value { return false; }
        if let Some(value) = value {
            self.set(key, value);
        } else {
            self.lines.retain(|line| !line.split_once('=').is_some_and(|(k, _)| k.trim() == key));
        }
        true
    }

    /// Sets `key` to `value`: its first line is replaced (and any later one dropped), or a line is added at the end.
    fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}={value}");
        let mut found = false;
        self.lines.retain_mut(|l| {
            if !l.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
                return true;
            }
            if found {
                return false;
            }
            found = true;
            l.clone_from(&line);
            true
        });
        if !found {
            self.lines.push(line);
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// Tests run in parallel threads: each has its own settings, and none reads or writes the user folder.
    static TEST: std::cell::RefCell<Store> = std::cell::RefCell::new(Store::default());
}

/// Runs `f` on the settings: read from `settings.txt` the first time.
pub(crate) fn read<R>(f: impl FnOnce(&Store) -> R) -> R {
    #[cfg(test)]
    return TEST.with(|s| f(&s.borrow()));
    #[cfg(not(test))]
    f(&STORE.get_or_init(|| Mutex::new(load())).lock().unwrap_or_else(|e| e.into_inner()))
}

fn mutate(f: impl FnOnce(&mut Store) -> bool) {
    #[cfg(test)]
    TEST.with(|s| { f(&mut s.borrow_mut()); });
    #[cfg(not(test))]
    {
        let mut s = STORE.get_or_init(|| Mutex::new(load())).lock().unwrap_or_else(|e| e.into_inner());
        if f(&mut s) { save(&s); }
    }
}

pub(crate) fn begin_appearance_preview(keys: &[&'static str]) {
    mutate(|s| {
        if s.appearance.is_none() {
            s.appearance = Some(keys.iter().map(|&key| (key, s.get(key).filter(|v| !v.is_empty()).map(str::to_owned))).collect());
        }
        false
    });
}

pub(crate) fn finish_appearance_preview(apply: bool) {
    mutate(|s| {
        if apply {
            s.apply_appearance()
        } else {
            s.appearance = None;
            false
        }
    });
}

pub(crate) fn restore_appearance(values: &[(&'static str, Option<String>)]) {
    mutate(|s| {
        let mut changed = false;
        for (key, value) in values {
            changed |= s.restore_appearance_value(key, value.as_deref());
        }
        changed
    });
}

/// Sets `key` to `value` (one line: line breaks become spaces) and keeps the file when that changed it.
pub(crate) fn write(key: &str, value: &str) {
    let value = value.replace(['\r', '\n'], " ");
    mutate(|s| s.update(key, &value));
}

/// Sets each key to its value as `write` does, keeping the file once. A key without a line stays without one when its
/// value is empty.
pub(crate) fn write_all(pairs: &[(String, String)]) {
    mutate(|s| {
        let mut changed = false;
        for (key, value) in pairs {
            let value = value.replace(['\r', '\n'], " ");
            if s.get(key).unwrap_or("") != value {
                changed |= s.update(key, &value);
            }
        }
        changed
    });
}

/// `key`'s value as text, `None` when it is not set or empty.
pub(crate) fn get(key: &str) -> Option<String> {
    read(|s| s.get(key).filter(|v| !v.is_empty()).map(String::from))
}

/// A switch kept as `1` / `0` under `key`; `default` when it is not set or is something else.
pub(crate) fn flag(key: &str, default: bool) -> bool {
    read(|s| match s.get(key) {
        Some("1") => true,
        Some("0") => false,
        _ => default,
    })
}

/// Keeps switch `key` as `1` / `0`.
pub(crate) fn set_flag(key: &str, on: bool) {
    write(key, if on { "1" } else { "0" });
}

#[cfg_attr(test, allow(dead_code))]
fn load() -> Store {
    os_paths::user_file("settings.txt").and_then(|f| std::fs::read_to_string(f).ok()).map(|t| Store::parse(&t)).unwrap_or_default()
}

/// Writes a whole new file beside the old one and then puts it in its place, so a crash part-way leaves the old file.
#[cfg_attr(test, allow(dead_code))]
fn save(s: &Store) {
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let tmp = dir.join("settings.txt.tmp");
        if std::fs::write(&tmp, s.text()).is_ok() && std::fs::rename(&tmp, dir.join("settings.txt")).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_preview_isolation_keeps_unrelated_writes_and_commits_only_on_apply() {
        let mut store = Store::parse("glow=35\nother=first\n");
        store.appearance = Some(BTreeMap::from([("glow", Some("35".into())), ("tint", None)]));
        assert!(!store.update("glow", "70"));
        assert!(!store.update("tint", "plum"));
        assert!(store.update("other", "second"));
        assert_eq!(store.get("glow"), Some("70"));
        assert_eq!(store.text(), "glow=35\nother=second\n");
        store.appearance = None;
        assert_eq!(store.get("tint"), None);
        assert_eq!(store.get("glow"), Some("35"));
        store.appearance = Some(BTreeMap::from([("glow", None), ("tint", Some("plum".into()))]));
        assert!(store.apply_appearance());
        assert_eq!(store.text(), "other=second\ntint=plum\n");
    }

    /// The file's rules: the last line of a key wins on reading; setting replaces its first line and drops the later
    /// ones, or adds a line; unknown lines and lines without `=` are written back as they were.
    #[test]
    fn lines_are_kept_and_the_last_wins() {
        let mut s = Store::parse("theme=dark\nnot a setting\ntoolbar_flat_buttons=0\nfuture_key = x \ntoolbar_flat_buttons=1\n");
        assert_eq!((s.get("toolbar_flat_buttons"), s.get("future_key"), s.get("missing")), (Some("1"), Some("x"), None));
        s.set("toolbar_flat_buttons", "0");
        s.set("clock_mode", "SMPTE 25 fps");
        assert_eq!(s.text(), "theme=dark\nnot a setting\ntoolbar_flat_buttons=0\nfuture_key = x \nclock_mode=SMPTE 25 fps\n");
        assert_eq!(Store::parse(&s.text()), s);
    }

    /// Switches read their default when unset or unreadable; values are one line; a test's store is its own.
    #[test]
    fn flags_and_text() {
        assert!(flag("toolbar_separators", true) && !flag("toolbar_hint_bar", false));
        write("toolbar_hint_bar", "yes");
        assert!(!flag("toolbar_hint_bar", false), "not 1 or 0: the default");
        set_flag("toolbar_hint_bar", true);
        set_flag("toolbar_separators", false);
        assert!(flag("toolbar_hint_bar", false) && !flag("toolbar_separators", true));
        write("clock_mode", "two\nlines");
        assert_eq!(get("clock_mode").as_deref(), Some("two lines"));
        write("clock_mode", "");
        assert_eq!(get("clock_mode"), None);
    }
}
