// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Resolve the Browser preview input to the selected insert, with Master as the default.
// spec: ui/settings-choices.md "Mixer: Browser preview track"

use os_model::Project;

pub fn destination(project: &Project, track: u16) -> usize {
    let index = usize::from(track);
    if index + 2 <= project.mixer.len() && project.mixer[index].index == track {
        index
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_project;

    #[test]
    fn browser_preview_route_resolves_master_and_existing_inserts() {
        let project = test_project();
        let last = project.mixer.len() as u16 - 2;
        for track in [0, 2, last] { assert_eq!(destination(&project, track), usize::from(track)); }
        for track in [last + 1, 125, u16::MAX] { assert_eq!(destination(&project, track), 0); }
        assert_eq!(destination(&Project::default(), 2), 0);
    }
}
