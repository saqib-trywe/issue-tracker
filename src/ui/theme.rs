//! Keeps the app's appearance in step with the operating system.
//!
//! `gpui_component::init` pins the theme to Light unconditionally, so without
//! this the app stays light no matter what the system is set to.

use gpui::{App, Subscription, Window};
use gpui_component::Theme;

/// Adopts the current system appearance.
///
/// Call this before the first paint, otherwise a machine in dark mode opens a
/// white window and then flips once the observer fires.
pub fn apply_system_appearance(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
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
