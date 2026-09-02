//! Keeps the app's appearance in step with the operating system.
//!
//! `gpui_component::init` pins the theme to Light unconditionally, so without
//! this the app stays light no matter what the system is set to.

use std::rc::Rc;

use gpui::{App, Subscription, Window};
use gpui_component::{Theme, ThemeConfig};

/// Adopts the current system appearance.
///
/// Call this before the first paint, otherwise a machine in dark mode opens a
/// white window and then flips once the observer fires.
pub fn apply_system_appearance(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
}

/// Installs the chosen light and dark themes and re-applies the current mode.
///
/// `Theme::change` reads whichever of `light_theme` / `dark_theme` matches the
/// active mode, and only populates those fields when the global is first
/// created. So assigning them here sticks: the system-appearance observer
/// keeps working and simply picks up the new choice on the next switch.
/// Each slot is independent: passing `None` leaves that mode's theme alone,
/// which is what makes it possible to choose a dark theme without also having
/// chosen a light one.
pub fn apply_themes(
    light: Option<Rc<ThemeConfig>>,
    dark: Option<Rc<ThemeConfig>>,
    window: &mut Window,
    cx: &mut App,
) {
    let mode = {
        let theme = Theme::global_mut(cx);
        if let Some(light) = light {
            theme.light_theme = light;
        }
        if let Some(dark) = dark {
            theme.dark_theme = dark;
        }
        theme.mode
    };

    Theme::change(mode, Some(window), cx);
}

/// Follows the system appearance from here on.
///
/// The returned `Subscription` has to be held for the lifetime of the window;
/// dropping it silently stops the observer.
pub fn observe_system_appearance(window: &Window) -> Subscription {
    window.observe_window_appearance(|window, cx| {
        Theme::sync_system_appearance(Some(window), cx);
    })
}
