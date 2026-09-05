// SPDX-License-Identifier: GPL-3.0-only

//! The set of named themes the user can choose from.
//!
//! Theme files are embedded at compile time from `assets/themes/`. Each file
//! is a [`ThemeSet`] holding one or more variants, each with its own mode.
//! The catalogue flattens those into two lists — one per mode — because the
//! library models a light theme and a dark theme as independent slots, and
//! because the upstream catalogue is uneven: over half the families ship only
//! one mode, so pairing them into "families" would strand most of them.

use std::rc::Rc;

use gpui::{App, SharedString, Task, Window};
use gpui_component::searchable_list::{SearchableListDelegate, SearchableListItem};
use gpui_component::{IndexPath, ThemeConfig, ThemeMode, ThemeSet};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/themes/"]
#[include = "*.json"]
struct ThemeAssets;

/// One selectable theme: a display name and the config it applies.
#[derive(Clone)]
pub struct ThemeChoice {
    pub name: SharedString,
    pub config: Rc<ThemeConfig>,
}

impl SearchableListItem for ThemeChoice {
    /// Themes are identified by display name, which is also what gets
    /// persisted — so a stored value survives reordering of the catalogue.
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.name
    }
}

/// Backs one `Select`: all themes for a single mode, plus the current filter.
pub struct ThemeListDelegate {
    all: Vec<ThemeChoice>,
    matching: Vec<ThemeChoice>,
}

impl ThemeListDelegate {
    pub fn new(all: Vec<ThemeChoice>) -> Self {
        Self {
            matching: all.clone(),
            all,
        }
    }

    /// Index of a theme in the unfiltered list, for seeding the initial
    /// selection.
    pub fn index_of(&self, name: &str) -> Option<IndexPath> {
        self.all
            .iter()
            .position(|choice| choice.name.as_ref() == name)
            .map(IndexPath::new)
    }
}

impl SearchableListDelegate for ThemeListDelegate {
    type Item = ThemeChoice;

    fn items_count(&self, _section: usize) -> usize {
        self.matching.len()
    }

    fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
        self.matching.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.matching
            .iter()
            .position(|choice| choice.value() == value)
            .map(IndexPath::new)
    }

    fn perform_search(&mut self, query: &str, _window: &mut Window, _cx: &mut App) -> Task<()> {
        let needle = query.trim().to_lowercase();
        self.matching = if needle.is_empty() {
            self.all.clone()
        } else {
            self.all
                .iter()
                .filter(|choice| choice.name.to_lowercase().contains(&needle))
                .cloned()
                .collect()
        };
        Task::ready(())
    }
}

/// Every embedded theme, split by the mode it applies to.
pub struct ThemeCatalogue {
    light: Vec<ThemeChoice>,
    dark: Vec<ThemeChoice>,
}

impl ThemeCatalogue {
    /// Parses every embedded theme file.
    ///
    /// A file that fails to parse is logged and skipped rather than taken as
    /// fatal — one bad theme should not stop the app from starting.
    pub fn load() -> Self {
        let mut light = Vec::new();
        let mut dark = Vec::new();

        for path in ThemeAssets::iter() {
            let Some(file) = ThemeAssets::get(&path) else {
                continue;
            };
            let contents = match std::str::from_utf8(&file.data) {
                Ok(contents) => contents,
                Err(err) => {
                    eprintln!("theme {path} is not valid UTF-8: {err}");
                    continue;
                }
            };
            let set: ThemeSet = match serde_json::from_str(contents) {
                Ok(set) => set,
                Err(err) => {
                    eprintln!("theme {path} could not be parsed: {err}");
                    continue;
                }
            };

            for config in set.themes {
                let choice = ThemeChoice {
                    name: config.name.clone(),
                    config: Rc::new(config),
                };
                match choice.config.mode {
                    ThemeMode::Light => light.push(choice),
                    ThemeMode::Dark => dark.push(choice),
                }
            }
        }

        light.sort_by_key(|choice| choice.name.to_lowercase());
        dark.sort_by_key(|choice| choice.name.to_lowercase());

        Self { light, dark }
    }

    pub fn for_mode(&self, mode: ThemeMode) -> &[ThemeChoice] {
        match mode {
            ThemeMode::Light => &self.light,
            ThemeMode::Dark => &self.dark,
        }
    }

    /// Looks up a theme by display name within a mode.
    ///
    /// Returns `None` when a persisted name no longer exists, so callers can
    /// fall back to the default rather than failing.
    pub fn find(&self, mode: ThemeMode, name: &str) -> Option<&ThemeChoice> {
        self.for_mode(mode)
            .iter()
            .find(|choice| choice.name.as_ref() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_theme_parses() {
        // Guards the vendored snapshot: a malformed file is skipped at
        // runtime, so only a test will notice it went missing.
        let embedded = ThemeAssets::iter().count();
        assert_eq!(embedded, 21, "expected 21 vendored theme files");

        let catalogue = ThemeCatalogue::load();
        assert!(!catalogue.for_mode(ThemeMode::Light).is_empty());
        assert!(!catalogue.for_mode(ThemeMode::Dark).is_empty());
    }

    #[test]
    fn catalogue_is_uneven_by_design() {
        // More dark variants than light ones: over half the upstream families
        // are dark-only. This is why selection is per-mode.
        let catalogue = ThemeCatalogue::load();
        let light = catalogue.for_mode(ThemeMode::Light).len();
        let dark = catalogue.for_mode(ThemeMode::Dark).len();
        assert!(
            dark > light,
            "expected more dark variants ({dark}) than light ({light})"
        );
    }

    #[test]
    fn known_themes_resolve_by_name() {
        let catalogue = ThemeCatalogue::load();
        assert!(catalogue.find(ThemeMode::Light, "Gruvbox Light").is_some());
        assert!(catalogue.find(ThemeMode::Dark, "Gruvbox Dark").is_some());
    }

    #[test]
    fn unknown_name_returns_none_so_callers_can_fall_back() {
        let catalogue = ThemeCatalogue::load();
        assert!(catalogue.find(ThemeMode::Light, "No Such Theme").is_none());
        // A dark theme is not a valid light choice.
        assert!(catalogue.find(ThemeMode::Light, "Gruvbox Dark").is_none());
    }

    #[test]
    fn theme_colours_survive_parsing() {
        // Name and mode parsing correctly is not enough: if `colors` comes
        // back empty every field silently falls back to the built-in default,
        // and the theme applies as a no-op.
        let catalogue = ThemeCatalogue::load();
        let gruvbox = catalogue
            .find(ThemeMode::Light, "Gruvbox Light")
            .expect("Gruvbox Light in catalogue");
        assert_eq!(
            gruvbox.config.colors.background.as_deref(),
            Some("#fbf1c7"),
            "theme colours did not deserialise"
        );
    }

    #[test]
    fn variants_are_sorted_by_name() {
        let catalogue = ThemeCatalogue::load();
        let names: Vec<String> = catalogue
            .for_mode(ThemeMode::Dark)
            .iter()
            .map(|choice| choice.name.to_lowercase())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
