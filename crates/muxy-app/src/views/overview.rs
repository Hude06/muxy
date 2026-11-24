use std::ops::Range as Span;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, HighlightStyle, Hsla, InteractiveElement,
    IntoElement, ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, StyledText,
    div, px,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use muxy_app_core::insights::{Cell, Digest, Range, Summary, model_name};
use muxy_app_core::{PaneId, Project};
use muxy_ui::components::ButtonInteraction;
use muxy_ui::theme::{Metrics, Theme};
use muxy_ui::tr;

use crate::model::AppModel;
use crate::model::insights::ago;

const CELL: f32 = 11.0;
const GAP: f32 = 3.0;
const LEVEL_ALPHA: [f32; 4] = [0.3, 0.5, 0.75, 1.0];
const AGENT_HUE_SHIFT: f32 = -0.12;
const ACTIVE_WITHIN_SECONDS: u64 = 120;
const MAX_TILES: usize = 5;

pub(crate) fn button(model: &AppModel, cx: &mut Context<AppModel>) -> AnyElement {
    let name = tr!("Show Overview");
    let tooltip = model.tooltip(name.clone(), muxy_core::shortcuts::ShortcutId::ShowOverview);
    super::titlebar::icon_button(
        "show-overview",
        muxy_ui::icon::Icon::Dashboard,
        tooltip,
        model,
    )
    .aria_label(name)
    .debug_selector(|| "show-overview-button".into())
    .on_click(cx.listener(|model, _, _, cx| {
        cx.stop_propagation();
        model.open_overview(cx);
    }))
    .into_any_element()
}

pub(crate) fn render(id: PaneId, model: &AppModel, cx: &Context<AppModel>) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    let insights = &model.insights;
    let body = match &insights.digest {
        Some(digest) => report_body(digest, model, cx),
        None => skeleton(theme, m),
    };
    div()
        .id(SharedString::from(format!("overview-{id}")))
        .debug_selector(|| "overview".into())
        .size_full()
        .bg(model.chrome_background())
        .text_color(theme.fg)
        .overflow_y_scroll()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |model, _, _, cx| model.focus_pane(id, cx)),
        )
        .child(
            div()
                .w_full()
                .max_w(m.scaled(820.0))
                .mx_auto()
                .px(m.scaled(40.0))
                .pt(m.scaled(36.0))
                .pb(m.scaled(48.0))
                .flex()
                .flex_col()
                .gap(m.scaled(32.0))
                .child(header(model, cx))
                .child(body),
        )
        .into_any_element()
}

fn header(model: &AppModel, cx: &Context<AppModel>) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    let zone = TimeZone::system();
    let now = zone.to_datetime(Timestamp::now());
    let date = now.strftime("%A, %B %-d").to_string();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(m.spacing6())
        .child(
            div()
                .text_size(m.font_body())
                .text_color(theme.fg_muted)
                .child(date),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(m.spacing4())
                .when(model.insights.loading(), |row| {
                    row.child(
                        div()
                            .text_size(m.font_footnote())
                            .text_color(theme.fg_dim)
                            .child(tr!("Updating…")),
                    )
                })
                .child(range_picker(model, cx)),
        )
        .into_any_element()
}

fn range_picker(model: &AppModel, cx: &Context<AppModel>) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    let selected = model.insights.range;
    let mut group = div()
        .id("overview-range")
        .role(Role::RadioGroup)
        .aria_label(tr!("Range"))
        .flex()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(m.radius_lg())
        .bg(theme.hover);
    for range in Range::ALL {
        let active = range == selected;
        let label = match range {
            Range::Today => tr!("Today"),
            Range::Week => tr!("Week"),
            Range::Month => tr!("Month"),
        };
        group = group.child(
            div()
                .id(SharedString::from(format!("overview-range-{range:?}")))
                .role(Role::RadioButton)
                .aria_label(label.clone())
                .px(m.spacing6())
                .py(m.spacing2())
                .rounded(m.radius_md())
                .text_size(m.font_footnote())
                .text_color(if active { theme.fg } else { theme.fg_muted })
                .when(active, |segment| {
                    segment
                        .bg(theme.surface)
                        .font_weight(FontWeight::MEDIUM)
                        .shadow(muxy_ui::theme::Elevation::Elevated.shadow(theme.bg))
                })
                .when(!active, |segment| {
                    segment
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.fg).bg(theme.hover))
                })
                .focus(|style| style.border_1().border_color(theme.accent))
                .button_interaction(cx.listener(move |model, _: &ClickEvent, _, cx| {
                    model.set_insights_range(range, cx);
                }))
                .child(label),
        );
    }
    group.into_any_element()
}

fn report_body(digest: &Digest, model: &AppModel, cx: &Context<AppModel>) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    let range = model.insights.range;
    let period = digest.period(range);
    let summary = &period.current;
    let projects = worked_on(summary, model);
    div()
        .flex()
        .flex_col()
        .gap(m.scaled(32.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(m.spacing6())
                .child(headline(summary, &projects, range, model))
                .when(!projects.is_empty(), |column| {
                    column.child(project_chips(&projects, model))
                })
                .child(trends(summary, &period.previous, range, theme, m)),
        )
        .when_some(
            super::plan_usage::overview_cards(model, cx),
            |body, plans| body.child(rule(theme)).child(plans),
        )
        .child(rule(theme))
        .child(commits(digest, summary, range, theme, m))
        .child(rule(theme))
        .child(agents(digest, summary, theme, m))
        .when(!model.insights.recent.is_empty(), |body| {
            body.child(rule(theme)).child(recent(model, cx))
        })
        .into_any_element()
}

fn rule(theme: &Theme) -> impl IntoElement {
    div().h(px(1.0)).w_full().bg(theme.fg_alpha(0.08))
}

fn worked_on<'a>(summary: &Summary, model: &'a AppModel) -> Vec<&'a Project> {
    model
        .state
        .projects()
        .iter()
        .filter(|project| project.server_id.is_local() && !project.home)
        .filter(|project| {
            let directory = project.directory.as_path();
            summary
                .directories
                .iter()
                .any(|path| path.starts_with(directory))
        })
        .collect()
}

fn headline(
    summary: &Summary,
    projects: &[&Project],
    range: Range,
    model: &AppModel,
) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    let when = match range {
        Range::Today => tr!("so far"),
        Range::Week => tr!("this week"),
        Range::Month => tr!("this month"),
    };
    let parts = [
        (format!("{}, {}. ", greeting(), first_name()), true),
        (tr!("You used").to_string() + " ", false),
        (tr!("%@ tokens", &compact(summary.tokens)).to_string(), true),
        (format!(" {when}, "), false),
        (tr!("made").to_string() + " ", false),
        (
            count(u64::from(summary.commits), &tr!("commit"), &tr!("commits")),
            true,
        ),
        (", ".to_owned() + &tr!("and worked on") + " ", false),
        (
            count(
                u64::try_from(projects.len()).unwrap_or(0),
                &tr!("project"),
                &tr!("projects"),
            ),
            true,
        ),
    ];
    let mut text = String::new();
    let mut highlights: Vec<(Span<usize>, HighlightStyle)> = Vec::new();
    for (part, strong) in parts {
        let start = text.len();
        text.push_str(&part);
        if strong {
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(theme.fg),
                    ..HighlightStyle::default()
                },
            ));
        }
    }
    div()
        .w_full()
        .text_size(m.scaled(34.0))
        .line_height(m.scaled(42.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.fg_dim)
        .child(StyledText::new(text).with_highlights(highlights))
        .into_any_element()
}

fn project_chips(projects: &[&Project], model: &AppModel) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    let hidden = projects.len().saturating_sub(MAX_TILES);
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(m.spacing3())
        .children(projects.iter().take(MAX_TILES).map(|project| {
            div()
                .flex()
                .items_center()
                .gap(m.spacing3())
                .pl(m.spacing2())
                .pr(m.spacing5())
                .py(m.spacing2())
                .rounded_full()
                .bg(theme.hover)
                .text_size(m.font_footnote())
                .child(super::sidebar::project_tile(
                    project,
                    model,
                    m.scaled(18.0),
                    None,
                ))
                .child(project.name.clone())
        }))
        .when(hidden > 0, |row| {
            row.child(
                div()
                    .text_size(m.font_footnote())
                    .text_color(theme.fg_muted)
                    .child(format!("+{hidden}")),
            )
        })
        .into_any_element()
}

fn trends(
    summary: &Summary,
    previous: &Summary,
    range: Range,
    theme: &Theme,
    m: Metrics,
) -> AnyElement {
    let against = match range {
        Range::Today => tr!("than yesterday"),
        Range::Week => tr!("than last week"),
        Range::Month => tr!("than the 30 days before"),
    };
    let mut items: Vec<(bool, String)> = Vec::new();
    if previous.tokens > 0 && summary.tokens > 0 {
        let change = percent_change(summary.tokens, previous.tokens);
        if change != 0 {
            let more = change > 0;
            let word = if more {
                tr!("more tokens")
            } else {
                tr!("fewer tokens")
            };
            items.push((more, format!("{}% {word} {against}", change.abs())));
        }
    }
    if summary.commits != previous.commits && (summary.commits > 0 || previous.commits > 0) {
        let more = summary.commits > previous.commits;
        let difference = summary.commits.abs_diff(previous.commits);
        let word = match (more, difference == 1) {
            (true, true) => tr!("more commit"),
            (true, false) => tr!("more commits"),
            (false, true) => tr!("fewer commit"),
            (false, false) => tr!("fewer commits"),
        };
        items.push((more, format!("{difference} {word} {against}")));
    }
    div()
        .flex()
        .flex_wrap()
        .gap(m.spacing7())
        .text_size(m.font_body())
        .text_color(theme.fg_muted)
        .children(items.into_iter().map(|(up, text)| {
            div()
                .flex()
                .gap(m.spacing2())
                .child(
                    div()
                        .text_color(if up { theme.accent } else { theme.fg_dim })
                        .child(if up { "↗" } else { "↘" }),
                )
                .child(text)
        }))
        .into_any_element()
}

fn ramp(base: Hsla, empty: Hsla) -> [Hsla; 5] {
    let mut colors = [empty; 5];
    for (slot, alpha) in colors.iter_mut().skip(1).zip(LEVEL_ALPHA) {
        *slot = base.opacity(alpha);
    }
    colors
}

fn agent_color(theme: &Theme) -> Hsla {
    let mut color = theme.accent;
    color.h = (color.h + AGENT_HUE_SHIFT).rem_euclid(1.0);
    color
}

fn section(
    title: SharedString,
    meta: String,
    colors: [Hsla; 5],
    theme: &Theme,
    m: Metrics,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(m.spacing6())
        .child(
            div()
                .text_size(m.font_headline())
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(m.spacing6())
                .text_size(m.font_caption())
                .text_color(theme.fg_muted)
                .child(meta)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(tr!("Less"))
                        .children(
                            colors
                                .iter()
                                .map(|color| div().size(m.scaled(9.0)).rounded(px(2.0)).bg(*color)),
                        )
                        .child(tr!("More")),
                ),
        )
}

fn heatmap(cells: &[Cell], colors: [Hsla; 5], today_ring: Hsla, m: Metrics) -> AnyElement {
    let pitch = CELL + GAP;
    let mut columns: Vec<Vec<&Cell>> = Vec::new();
    for cell in cells {
        let starts_week = cell.date.weekday() == jiff::civil::Weekday::Sunday;
        if starts_week || columns.is_empty() {
            columns.push(Vec::new());
        }
        if let Some(column) = columns.last_mut() {
            column.push(cell);
        }
    }
    let mut months: Vec<(usize, String)> = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        if let Some(first) = column.first()
            && first.date.day() <= 7
        {
            months.push((index, first.date.strftime("%b").to_string()));
        }
    }
    let count = columns.len().max(1);
    let width = m.scaled(pitch * f32::from(u16::try_from(count).unwrap_or(1)) - GAP);
    let last = cells.last().map(|cell| cell.date);
    div()
        .w_full()
        .flex()
        .justify_end()
        .overflow_hidden()
        .child(
            div()
                .flex_none()
                .w(width)
                .flex()
                .flex_col()
                .gap(m.spacing3())
                .child(
                    div()
                        .flex()
                        .gap(m.scaled(GAP))
                        .children(columns.into_iter().map(|column| {
                            div().flex().flex_col().gap(m.scaled(GAP)).children(
                                column.into_iter().map(|cell| {
                                    let today = Some(cell.date) == last;
                                    div()
                                        .size(m.scaled(CELL))
                                        .rounded(m.scaled(2.5))
                                        .bg(colors[usize::from(cell.level.min(4))])
                                        .when(today, |cell| {
                                            cell.border_1().border_color(today_ring)
                                        })
                                }),
                            )
                        })),
                )
                .child(
                    div().relative().h(m.scaled(12.0)).children(
                        months
                            .into_iter()
                            .filter(|(index, _)| *index + 2 < count)
                            .map(|(index, label)| {
                                div()
                                    .absolute()
                                    .left(m.scaled(
                                        pitch * f32::from(u16::try_from(index).unwrap_or(0)),
                                    ))
                                    .text_size(m.font_caption())
                                    .child(label)
                            }),
                    ),
                ),
        )
        .into_any_element()
}

fn stats(
    items: Vec<(String, SharedString, Option<Hsla>)>,
    theme: &Theme,
    m: Metrics,
) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap(m.spacing8())
        .children(items.into_iter().map(|(value, label, color)| {
            div()
                .min_w(m.scaled(150.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(m.font_title_large())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color.unwrap_or(theme.fg))
                        .child(value),
                )
                .child(
                    div()
                        .text_size(m.font_footnote())
                        .text_color(theme.fg_muted)
                        .child(label),
                )
        }))
        .into_any_element()
}

fn commits(
    digest: &Digest,
    summary: &Summary,
    range: Range,
    theme: &Theme,
    m: Metrics,
) -> AnyElement {
    let colors = ramp(theme.accent, theme.fg_alpha(0.07));
    let yearly = digest.yearly_commits;
    let repos_label = match range {
        Range::Today => tr!("Repos today"),
        Range::Week => tr!("Repos this week"),
        Range::Month => tr!("Repos this month"),
    };
    let streak = digest.streak;
    let repos = summary.repositories.len();
    div()
        .flex()
        .flex_col()
        .gap(m.spacing7())
        .child(section(
            tr!("Commits"),
            tr!("%@ in the last year", &grouped(yearly)).to_string(),
            colors,
            theme,
            m,
        ))
        .child(if yearly == 0 {
            empty(&tr!("No commits by you in your projects in the last year. Pane reads each project's Git history on this Mac."), theme, m)
        } else {
            heatmap(&digest.commit_cells, colors, theme.fg_muted, m)
        })
        .child(stats(
            vec![
                (
                    count(u64::from(streak), &tr!("day"), &tr!("days")),
                    tr!("Streak"),
                    None,
                ),
                (grouped(u64::try_from(repos).unwrap_or(0)), repos_label, None),
                lines("+", summary.added, tr!("Lines added"), theme.accent),
                lines("−", summary.removed, tr!("Lines removed"), theme.danger),
            ],
            theme,
            m,
        ))
        .into_any_element()
}

fn lines(
    sign: &str,
    value: u64,
    label: SharedString,
    color: Hsla,
) -> (String, SharedString, Option<Hsla>) {
    if value == 0 {
        return ("0".to_owned(), label, None);
    }
    (format!("{sign}{}", grouped(value)), label, Some(color))
}

fn agents(digest: &Digest, summary: &Summary, theme: &Theme, m: Metrics) -> AnyElement {
    let color = agent_color(theme);
    let colors = ramp(color, theme.fg_alpha(0.07));
    let yearly = digest.yearly_tokens;
    div()
        .flex()
        .flex_col()
        .gap(m.spacing7())
        .child(section(
            tr!("Agent activity"),
            tr!("%@ tokens in the last year", &compact(yearly)).to_string(),
            colors,
            theme,
            m,
        ))
        .child(if yearly == 0 {
            empty(&tr!("No agent sessions found. Pane reads Claude Code and Codex logs on this Mac. Run an agent in any pane to start filling this in."), theme, m)
        } else {
            heatmap(&digest.token_cells, colors, theme.fg_muted, m)
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(m.spacing8())
                .child(div().flex_1().min_w(m.scaled(300.0)).child(stats(
                    vec![
                        (grouped(u64::from(summary.sessions)), tr!("Sessions"), None),
                        (duration(summary.working_seconds), tr!("Agents working"), None),
                    ],
                    theme,
                    m,
                )))
                .when(!summary.models.is_empty(), |row| {
                    row.child(models(summary, color, theme, m))
                }),
        )
        .into_any_element()
}

fn models(summary: &Summary, color: Hsla, theme: &Theme, m: Metrics) -> AnyElement {
    let total: u64 = summary.models.iter().map(|(_, tokens)| tokens).sum();
    let shown: Vec<(String, u64, Hsla)> = summary
        .models
        .iter()
        .filter(|(_, tokens)| percent(*tokens, total) > 0)
        .take(4)
        .enumerate()
        .map(|(index, (model, tokens))| {
            let alpha = LEVEL_ALPHA[3 - index.min(3)];
            (model_name(model), *tokens, color.opacity(alpha))
        })
        .collect();
    div()
        .flex_1()
        .min_w(m.scaled(260.0))
        .flex()
        .flex_col()
        .gap(m.spacing4())
        .child(
            div()
                .flex()
                .h(m.scaled(6.0))
                .w_full()
                .rounded_full()
                .overflow_hidden()
                .bg(theme.fg_alpha(0.07))
                .children(shown.iter().map(|(_, tokens, color)| {
                    div()
                        .h_full()
                        .flex_basis(gpui::relative(share(*tokens, total)))
                        .bg(*color)
                })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(m.spacing6())
                .gap_y(m.spacing2())
                .text_size(m.font_footnote())
                .children(shown.iter().map(|(name, tokens, color)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(m.spacing2())
                        .child(div().size(m.scaled(7.0)).rounded_full().bg(*color))
                        .child(name.clone())
                        .child(
                            div()
                                .text_color(theme.fg_dim)
                                .child(format!("{}%", percent(*tokens, total))),
                        )
                })),
        )
        .into_any_element()
}

fn recent(model: &AppModel, cx: &Context<AppModel>) -> AnyElement {
    let theme = &model.theme;
    let m = model.metrics;
    div()
        .flex()
        .flex_col()
        .gap(m.spacing4())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(m.font_headline())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tr!("Pick up where you left off")),
                )
                .child(
                    div()
                        .text_size(m.font_caption())
                        .text_color(theme.fg_muted)
                        .child(tr!("Agent sessions on this Mac")),
                ),
        )
        .children(
            model
                .insights
                .recent
                .iter()
                .enumerate()
                .map(|(index, recent)| {
                    let elapsed = ago(recent.session.modified).as_secs();
                    let active = elapsed < ACTIVE_WITHIN_SECONDS;
                    let project = model
                        .state
                        .project(recent.project)
                        .map(|project| project.name.clone())
                        .unwrap_or_default();
                    let detail = format!(
                        "{project} · {} · {}",
                        recent.session.provider.name(),
                        relative(elapsed)
                    );
                    div()
                        .id(SharedString::from(format!("overview-recent-{index}")))
                        .role(Role::Button)
                        .aria_label(recent.session.title.clone())
                        .flex()
                        .items_center()
                        .gap(m.spacing5())
                        .px(m.spacing4())
                        .py(m.spacing4())
                        .mx(-m.spacing4())
                        .rounded(m.radius_md())
                        .text_size(m.font_body())
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.hover))
                        .button_interaction(cx.listener(move |model, _: &ClickEvent, _, cx| {
                            model.resume_recent(index, cx);
                        }))
                        .child(
                            div()
                                .flex_none()
                                .px(m.spacing3())
                                .py(px(1.0))
                                .rounded_full()
                                .text_size(m.font_caption())
                                .bg(if active {
                                    theme.accent_soft
                                } else {
                                    theme.hover
                                })
                                .text_color(if active { theme.accent } else { theme.fg_muted })
                                .child(if active { tr!("active") } else { tr!("resume") }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .child(recent.session.title.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(m.font_footnote())
                                .text_color(theme.fg_muted)
                                .child(detail),
                        )
                }),
        )
        .into_any_element()
}

fn empty(text: &str, theme: &Theme, m: Metrics) -> AnyElement {
    div()
        .py(m.spacing7())
        .text_size(m.font_body())
        .text_color(theme.fg_muted)
        .child(text.to_owned())
        .into_any_element()
}

fn skeleton(theme: &Theme, m: Metrics) -> AnyElement {
    let block = |width: f32, height: f32| {
        div()
            .w(gpui::relative(width))
            .h(m.scaled(height))
            .rounded(m.radius_md())
            .bg(theme.fg_alpha(0.06))
    };
    div()
        .flex()
        .flex_col()
        .gap(m.spacing6())
        .child(block(0.7, 40.0))
        .child(block(0.5, 40.0))
        .child(block(0.3, 14.0))
        .child(div().h(m.scaled(24.0)))
        .child(block(1.0, 110.0))
        .child(block(1.0, 110.0))
        .into_any_element()
}

fn greeting() -> SharedString {
    let hour = TimeZone::system().to_datetime(Timestamp::now()).hour();
    match hour {
        5..=11 => tr!("Good morning"),
        12..=16 => tr!("Good afternoon"),
        17..=21 => tr!("Good evening"),
        _ => tr!("Late night"),
    }
}

fn first_name() -> String {
    let name = std::env::var("USER").unwrap_or_default();
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn count(value: u64, one: &str, many: &str) -> String {
    format!("{} {}", grouped(value), if value == 1 { one } else { many })
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn compact(value: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];
    for (size, unit) in UNITS {
        if value >= size {
            let whole = value / size;
            let tenth = value % size * 10 / size;
            return if whole >= 100 || tenth == 0 {
                format!("{whole}{unit}")
            } else {
                format!("{whole}.{tenth}{unit}")
            };
        }
    }
    value.to_string()
}

fn duration(seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}

fn relative(seconds: u64) -> String {
    match seconds {
        0..60 => tr!("now").to_string(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

fn percent(part: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    (part * 100 + total / 2) / total
}

fn share(part: u64, total: u64) -> f32 {
    f32::from(u16::try_from(percent(part, total).min(100)).unwrap_or(0)) / 100.0
}

fn percent_change(now: u64, before: u64) -> i64 {
    let now = i128::from(now);
    let before = i128::from(before);
    i64::try_from((now - before) * 100 / before).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_numbers_stay_short_and_readable() {
        assert_eq!(compact(498_312_000), "498M");
        assert_eq!(compact(25_412_000_000), "25.4B");
        assert_eq!(compact(1_000), "1K");
        assert_eq!(compact(999), "999");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(512), "512");
    }

    #[test]
    fn changes_compare_against_the_earlier_period() {
        assert_eq!(percent_change(150, 100), 50);
        assert_eq!(percent_change(50, 100), -50);
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(0, 0), 0);
    }
}
