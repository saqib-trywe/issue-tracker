mod domain;
mod store;
mod ui;

use gpui::*;
use gpui_component::Root;

use store::Store;
use ui::IssueTracker;

fn main() -> anyhow::Result<()> {
    let store = Store::open()?;

    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);

    app.run(move |cx| {
        // Registers gpui-component's global state, theme, and key bindings.
        // Must run before any window is opened.
        gpui_component::init(cx);
        ui::init(cx);

        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Issues".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.0), px(0.0)),
                    size: size(px(1200.0), px(720.0)),
                })),
                ..Default::default()
            },
            |window, cx| {
                // Before the first paint: gpui_component::init pins the theme
                // to Light, so a dark-mode machine would otherwise flash white.
                ui::apply_system_appearance(window, cx);

                let view = cx.new(|cx| IssueTracker::new(store, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open window");
    });

    Ok(())
}
