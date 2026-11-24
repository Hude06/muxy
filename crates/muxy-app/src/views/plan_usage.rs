use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, div, relative,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use muxy_ui::components::{ButtonInteraction, IconGlyph};
use muxy_ui::icon::Icon;
use muxy_ui::theme::{Metrics, Theme};
use muxy_ui::tr;

use crate::model::AppModel;
use crate::plan_usage::{Limit, Reading, Status};

const WARN_AT: f32 = 75.0;
const FULL_AT: f32 = 95.0;

pub(crate) fn section(model: &AppModel, cx: &Context<AppModel>) -> Option<AnyElement> {
    let usage = &model.plan_usage;
    if !model.appearance.sidebar_expanded || usage.readings.is_empty() {
        return None;
    }
    let theme = &model.theme;
    let m = model.metrics;
    let roomiest = usage.roomiest();
    Some(
        div()
            .debug_selector(|| "sidebar-plan-usage".into())
            .flex()
            .flex_col()
            .flex_none()
            .gap(m.spacing1())
            .px(m.spacing3())
            .pt(m.spacing4())
            .pb(m.spacing3())
            .border_t_1()
            .border_color(theme.fg_alpha(0.06))
            .bg(model.sidebar_background())
            .child(header(model, cx))
            .children(
                usage.readings.iter().enumerate().map(|(index, reading)| {
                    row(index, reading, roomiest == Some(index), model, cx)
                }),
            )
            .into_any_element(),
    )
}

fn header(model: &AppModel, cx: &Context<AppModel>) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    div()
        .flex()
        .items_center()
        .justify_between()
        .px(m.spacing2())
        .child(
            div()
                .text_size(m.font_caption())
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.fg_muted)
                .child(tr!("Claude plans")),
        )
        .child(refresh_button("sidebar-plan-usage-refresh", model, cx))
        .into_any_element()
}

fn refresh_button(id: &'static str, model: &AppModel, cx: &Context<AppModel>) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(tr!("Refresh Claude plans"))
        .flex()
        .items_center()
        .justify_center()
        .size(m.scaled(22.0))
        .rounded(m.radius_sm())
        .cursor_pointer()
        .hover(|style| style.bg(theme.hover))
        .when(model.plan_usage.loading(), |button| button.opacity(0.4))
        .button_interaction(cx.listener(|model, _: &ClickEvent, _, cx| {
            model.refresh_plan_usage(cx);
        }))
        .child(IconGlyph::new(Icon::Refresh, m.icon_sm(), theme.fg_muted))
        .into_any_element()
}

pub(crate) fn overview_cards(model: &AppModel, cx: &Context<AppModel>) -> Option<AnyElement> {
    let usage = &model.plan_usage;
    if usage.readings.is_empty() {
        return None;
    }
    let m = model.metrics;
    let roomiest = usage.roomiest();
    Some(
        div()
            .flex()
            .flex_col()
            .gap(m.spacing7())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(m.spacing6())
                    .child(
                        div()
                            .text_size(m.font_headline())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr!("Claude plans")),
                    )
                    .child(refresh_button("overview-plan-usage-refresh", model, cx)),
            )
            .child(div().flex().flex_wrap().gap(m.spacing6()).children(
                usage.readings.iter().enumerate().map(|(index, reading)| {
                    card(index, reading, roomiest == Some(index), model, cx)
                }),
            ))
            .into_any_element(),
    )
}

fn card(
    index: usize,
    reading: &Reading,
    roomiest: bool,
    model: &AppModel,
    cx: &Context<AppModel>,
) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    let name = reading.account.name.clone();
    div()
        .id(SharedString::from(format!("overview-plan-{index}")))
        .role(Role::Button)
        .aria_label(tr!("Start %@ in a new tab", &name))
        .flex_1()
        .min_w(m.scaled(280.0))
        .flex()
        .flex_col()
        .gap(m.spacing5())
        .p(m.spacing6())
        .rounded(m.radius_lg())
        .border_1()
        .border_color(if roomiest {
            theme.accent.opacity(0.45)
        } else {
            theme.fg_alpha(0.08)
        })
        .bg(theme.fg_alpha(0.03))
        .cursor_pointer()
        .hover(|style| style.bg(theme.hover))
        .button_interaction(cx.listener(move |model, _: &ClickEvent, _, cx| {
            model.start_plan_account(index, cx);
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(m.spacing3())
                .text_size(m.font_body())
                .child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                .children(
                    reading
                        .plan
                        .clone()
                        .map(|plan| div().text_color(theme.fg_dim).child(plan)),
                )
                .child(div().flex_1())
                .when(roomiest, |line| {
                    line.child(
                        div()
                            .px(m.spacing3())
                            .py(gpui::px(1.0))
                            .rounded_full()
                            .text_size(m.font_caption())
                            .bg(theme.accent_soft)
                            .text_color(theme.accent)
                            .child(tr!("most room")),
                    )
                }),
        )
        .child(body(reading, theme, m))
        .child(
            div()
                .text_size(m.font_footnote())
                .text_color(theme.fg_muted)
                .child(tr!("Start %@ →", &name)),
        )
        .into_any_element()
}

fn row(
    index: usize,
    reading: &Reading,
    roomiest: bool,
    model: &AppModel,
    cx: &Context<AppModel>,
) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    let name = reading.account.name.clone();
    let title = tr!("Start %@ in a new tab", &name);
    div()
        .id(SharedString::from(format!("sidebar-plan-{index}")))
        .role(Role::Button)
        .aria_label(title)
        .flex()
        .flex_col()
        .gap(m.spacing2())
        .px(m.spacing2())
        .py(m.spacing3())
        .rounded(m.radius_md())
        .cursor_pointer()
        .hover(|style| style.bg(theme.hover))
        .button_interaction(cx.listener(move |model, _: &ClickEvent, _, cx| {
            model.start_plan_account(index, cx);
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(m.spacing3())
                .text_size(m.font_footnote())
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.fg)
                        .child(name),
                )
                .children(
                    reading
                        .plan
                        .clone()
                        .map(|plan| div().text_color(theme.fg_dim).child(plan)),
                )
                .child(div().flex_1())
                .when(roomiest, |line| {
                    line.child(
                        div()
                            .px(m.spacing2())
                            .rounded_full()
                            .text_size(m.font_caption())
                            .bg(theme.accent_soft)
                            .text_color(theme.accent)
                            .child(tr!("most room")),
                    )
                }),
        )
        .child(body(reading, theme, m))
        .into_any_element()
}

fn body(reading: &Reading, theme: &Theme, m: Metrics) -> AnyElement {
    let note = |text: SharedString| {
        div()
            .text_size(m.font_caption())
            .text_color(theme.fg_muted)
            .child(text)
            .into_any_element()
    };
    match &reading.status {
        Status::SignedOut => note(tr!(
            "Not signed in. Run %@ and log in.",
            &reading.account.name
        )),
        Status::Expired => note(tr!(
            "Sign-in expired. Run %@ once to renew it.",
            &reading.account.name
        )),
        Status::Failed(error) => note(tr!("Couldn't check usage: %@", error)),
        Status::Ready { session, week } => div()
            .flex()
            .flex_col()
            .gap(m.spacing2())
            .children(session.map(|limit| meter(&tr!("5h"), limit, theme, m)))
            .children(week.map(|limit| meter(&tr!("Week"), limit, theme, m)))
            .into_any_element(),
    }
}

fn meter(label: &SharedString, limit: Limit, theme: &Theme, m: Metrics) -> AnyElement {
    let color = level_color(limit.used, theme);
    let reset = limit.resets_at.map(resets).unwrap_or_default();
    div()
        .flex()
        .items_center()
        .gap(m.spacing3())
        .text_size(m.font_caption())
        .child(
            div()
                .w(m.scaled(30.0))
                .flex_none()
                .text_color(theme.fg_muted)
                .child(label.clone()),
        )
        .child(
            div()
                .flex_1()
                .h(m.scaled(5.0))
                .rounded_full()
                .overflow_hidden()
                .bg(theme.fg_alpha(0.08))
                .child(
                    div()
                        .h_full()
                        .w(relative(limit.used / 100.0))
                        .rounded_full()
                        .bg(color),
                ),
        )
        .child(
            div()
                .w(m.scaled(34.0))
                .flex_none()
                .flex()
                .justify_end()
                .text_color(if limit.used >= WARN_AT {
                    color
                } else {
                    theme.fg
                })
                .child(format!("{:.0}%", limit.used)),
        )
        .child(
            div()
                .w(m.scaled(52.0))
                .flex_none()
                .flex()
                .justify_end()
                .text_color(theme.fg_dim)
                .child(reset),
        )
        .into_any_element()
}

fn level_color(used: f32, theme: &Theme) -> Hsla {
    if used >= FULL_AT {
        theme.danger
    } else if used >= WARN_AT {
        theme.warning
    } else {
        theme.accent
    }
}

fn resets(at: Timestamp) -> String {
    let seconds = at.as_second() - Timestamp::now().as_second();
    if seconds <= 0 {
        return String::new();
    }
    if seconds < 86_400 {
        let hours = seconds / 3_600;
        let minutes = seconds % 3_600 / 60;
        return if hours == 0 {
            format!("{minutes}m")
        } else {
            format!("{hours}h {minutes}m")
        };
    }
    TimeZone::system()
        .to_datetime(at)
        .strftime("%a %-I%P")
        .to_string()
}
