//! Right column: inline editing of the selected Issue.
//!
//! Edits auto-save on a debounce; there is no save button. Status and
//! Priority are segmented buttons rather than dropdowns — with five fixed
//! values each, a menu costs a click and buys nothing.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, Textarea};
use gpui_component::{ActiveTheme, Sizable, WindowExt};

use super::tracker::IssueTracker;
use crate::domain::{IssueId, Priority, Status};

impl IssueTracker {
    pub(super) fn render_issue_detail(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(issue) = self.selected_issue() else {
            return div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Select an issue, or press c to create one.")
                .into_any_element();
        };

        let id = issue.id;
        let title = issue.display_title().to_string();
        let status = issue.status;
        let priority = issue.priority;
        let created = issue.created_at.format("%Y-%m-%d %H:%M").to_string();
        let updated = issue.updated_at.format("%Y-%m-%d %H:%M").to_string();

        div()
            .id("detail-scroll")
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("#{id}")),
                    )
                    .child(
                        Button::new("delete-issue")
                            .danger()
                            .small()
                            .label("Delete")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_delete(id, title.clone(), window, cx);
                            })),
                    ),
            )
            .child(Input::new(&self.title_input))
            .child(self.render_status_row(status, cx))
            .child(self.render_priority_row(priority, cx))
            .child(Textarea::new(&self.body_input))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("Created {created} · Updated {updated}")),
            )
            .into_any_element()
    }

    fn render_status_row(&self, active: Status, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .gap_1()
            .children(Status::ALL.into_iter().map(|status| {
                Button::new(SharedString::from(format!("status-{}", status.label())))
                    .small()
                    .label(status.label())
                    .when(status == active, |button| button.primary())
                    .when(status != active, |button| button.outline())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_status(status, window, cx);
                    }))
            }))
    }

    fn render_priority_row(&self, active: Priority, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .gap_1()
            .children(Priority::ALL.into_iter().map(|priority| {
                Button::new(SharedString::from(format!("priority-{}", priority.label())))
                    .small()
                    .label(priority.label())
                    .when(priority == active, |button| button.primary())
                    .when(priority != active, |button| button.outline())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_priority(priority, window, cx);
                    }))
            }))
    }

    /// Deleting erases an Issue outright, so it is always confirmed. To
    /// abandon work while keeping the record, set the Status to Cancelled.
    pub(super) fn confirm_delete(
        &mut self,
        id: IssueId,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .confirm()
                .title("Delete this issue?")
                .description(format!(
                    "#{id} “{title}” will be erased. To abandon it but keep the \
                     record, set its status to Cancelled instead."
                ))
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.delete_issue(id, window, cx));
                    true
                })
        });
    }
}
