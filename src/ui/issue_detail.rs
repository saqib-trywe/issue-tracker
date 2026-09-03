//! Right column: inline editing of the selected Issue.
//!
//! Edits auto-save on a debounce; there is no save button. Status and
//! Priority are segmented buttons rather than dropdowns — with five fixed
//! values each, a menu costs a click and buys nothing.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::combobox::Combobox;
use gpui_component::input::{Input, Textarea};
use gpui_component::tag::Tag as TagChip;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, WindowExt};

use super::tag_colour::colour_for;
use super::tracker::IssueTracker;
use crate::domain::{IssueId, Priority, Status, Tag};

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
        let tags = issue.tags.clone();
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
            .child(self.render_tag_editor(tags, cx))
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

    /// Tags: the chips are the display and the way to remove one, the
    /// Combobox is the way to add one.
    ///
    /// Removal deliberately does not go through the Combobox's own selection
    /// API, which mutates its state without emitting a change event — the
    /// Issue stays the single source of truth and the picker is re-pointed at
    /// it afterwards.
    fn render_tag_editor(
        &self,
        tags: Vec<Tag>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let this = cx.entity();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .children(tags.into_iter().map(|tag| {
                        let colour = colour_for(&tag);
                        let key = tag.key();
                        let name = tag.as_str().to_owned();
                        TagChip::color(colour).small().child(name).child(
                            Button::new(SharedString::from(format!("remove-tag-{key}")))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::Close).xsmall())
                                .tab_stop(false)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.remove_tag(tag.clone(), window, cx);
                                })),
                        )
                    })),
            )
            .child(
                Combobox::new(&self.tag_select)
                    .small()
                    .placeholder("Add a tag…")
                    .search_placeholder("Find or create a tag…")
                    .menu_width(px(220.0))
                    // The library's create recipe only adds the typed name to
                    // the dropdown; this puts it straight on the Issue.
                    //
                    // The label cannot quote what you typed: this closure runs
                    // inside `ComboboxState::render`, so reading that entity
                    // for its query would be a double lease, and the callback
                    // is handed no context to reach it another way. Clicking
                    // with an empty search box is a no-op.
                    .footer(move |_, cx| {
                        let this = this.clone();
                        Button::new("create-tag")
                            .ghost()
                            .w_full()
                            .justify_start()
                            .text_color(cx.theme().foreground)
                            .icon(Icon::new(IconName::Plus))
                            .label("Create tag from search")
                            .on_click(move |_, window, cx| {
                                this.update(cx, |this, cx| this.create_tag_from_query(window, cx));
                            })
                    }),
            )
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
