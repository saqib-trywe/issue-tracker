//! Left column: the View list.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::select::{Select, SelectState};
use gpui_component::{ActiveTheme, Sizable};

use super::theme_catalogue::ThemeListDelegate;
use super::tracker::IssueTracker;
use crate::domain::View;

impl IssueTracker {
    pub(super) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view();
        let counts: Vec<(View, usize)> = View::ALL
            .into_iter()
            .map(|view| (view, self.count_for(view)))
            .collect();

        div()
            .w(px(232.0))
            .h_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Views"),
            )
            .children({
                let mut rows = Vec::with_capacity(counts.len());
                for (view, count) in counts {
                    rows.push(Self::render_view_row(view, count, view == active, cx));
                }
                rows
            })
            .child(self.render_appearance(cx))
    }

    /// Theme pickers, pinned to the foot of the sidebar.
    ///
    /// Two selectors and no mode switch: which theme applies is the user's
    /// choice, but *when* dark applies stays with the OS.
    fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .mt_auto()
            .pt_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Appearance"),
            )
            .child(Self::render_theme_row("Light", &self.light_select, cx))
            .child(Self::render_theme_row("Dark", &self.dark_select, cx))
    }

    fn render_theme_row(
        label: &'static str,
        state: &Entity<SelectState<ThemeListDelegate>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .px_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            // With nothing stored the app uses gpui-component's built-in
            // theme, which is not one of the embedded files and so has no row
            // to select. "Default" is the honest label for that state.
            .child(
                Select::new(state)
                    .small()
                    .placeholder("Default")
                    .menu_width(px(200.0)),
            )
    }

    fn render_view_row(
        view: View,
        count: usize,
        is_active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .id(SharedString::from(format!("view-{}", view.label())))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .when(is_active, |this| this.bg(cx.theme().accent))
            .when(!is_active, |this| {
                this.hover(|style| style.bg(cx.theme().muted))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_view(view, window, cx);
            }))
            .child(
                div()
                    .text_sm()
                    .when(is_active, |this| {
                        this.text_color(cx.theme().accent_foreground)
                    })
                    .child(view.label()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count.to_string()),
            )
    }
}
