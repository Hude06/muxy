mod agents;
mod git;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use jiff::ToSpan;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

pub use agents::Cache;

use crate::agent_sessions::Roots;

pub const WEEKS: usize = 53;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDay {
    pub tokens: u64,
    pub sessions: u32,
    pub working_seconds: u64,
    pub models: BTreeMap<String, u64>,
    pub directories: BTreeSet<PathBuf>,
}

impl AgentDay {
    fn merge(&mut self, other: &Self) {
        self.tokens += other.tokens;
        self.sessions += other.sessions;
        self.working_seconds += other.working_seconds;
        for (model, tokens) in &other.models {
            *self.models.entry(model.clone()).or_default() += tokens;
        }
        self.directories.extend(other.directories.iter().cloned());
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitDay {
    pub commits: u32,
    pub added: u64,
    pub removed: u64,
    pub repositories: BTreeSet<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Range {
    #[default]
    Today,
    Week,
    Month,
}

impl Range {
    pub const ALL: [Self; 3] = [Self::Today, Self::Week, Self::Month];

    const fn index(self) -> usize {
        match self {
            Self::Today => 0,
            Self::Week => 1,
            Self::Month => 2,
        }
    }

    pub const fn days(self) -> i64 {
        match self {
            Self::Today => 1,
            Self::Week => 7,
            Self::Month => 30,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub tokens: u64,
    pub commits: u32,
    pub added: u64,
    pub removed: u64,
    pub sessions: u32,
    pub working_seconds: u64,
    pub models: Vec<(String, u64)>,
    pub directories: BTreeSet<PathBuf>,
    pub repositories: BTreeSet<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub date: Date,
    pub value: u64,
    pub level: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Period {
    pub current: Summary,
    pub previous: Summary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Digest {
    pub commit_cells: Vec<Cell>,
    pub token_cells: Vec<Cell>,
    pub streak: u32,
    pub yearly_commits: u64,
    pub yearly_tokens: u64,
    periods: [Period; 3],
}

impl Digest {
    pub fn period(&self, range: Range) -> &Period {
        &self.periods[range.index()]
    }
}

impl From<&Report> for Digest {
    fn from(report: &Report) -> Self {
        Self {
            commit_cells: report.commit_cells(),
            token_cells: report.token_cells(),
            streak: report.streak(),
            yearly_commits: report.yearly_commits(),
            yearly_tokens: report.yearly_tokens(),
            periods: Range::ALL.map(|range| Period {
                current: report.summary(range),
                previous: report.previous(range),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub today: Date,
    pub git: BTreeMap<Date, GitDay>,
    pub agents: BTreeMap<Date, AgentDay>,
}

impl Report {
    pub fn collect(
        cache: &mut Cache,
        roots: &Roots,
        repositories: &[PathBuf],
        zone: &TimeZone,
        today: Date,
    ) -> Self {
        let since = first_day(today);
        Self {
            today,
            git: git::scan(repositories, zone, since),
            agents: cache.scan(roots, zone, since),
        }
    }

    pub fn summary(&self, range: Range) -> Summary {
        self.window(self.today, range)
    }

    pub fn previous(&self, range: Range) -> Summary {
        self.window(self.today.saturating_sub(range.days().days()), range)
    }

    fn window(&self, last: Date, range: Range) -> Summary {
        let first = last.saturating_sub((range.days() - 1).days());
        let mut summary = Summary::default();
        let mut agents = AgentDay::default();
        for day in self.git.range(first..=last).map(|(_, day)| day) {
            summary.commits += day.commits;
            summary.added += day.added;
            summary.removed += day.removed;
            summary.directories.extend(day.repositories.iter().cloned());
            summary
                .repositories
                .extend(day.repositories.iter().cloned());
        }
        for day in self.agents.range(first..=last).map(|(_, day)| day) {
            agents.merge(day);
        }
        summary.tokens = agents.tokens;
        summary.sessions = agents.sessions;
        summary.working_seconds = agents.working_seconds;
        summary.directories.extend(agents.directories);
        summary.models = agents.models.into_iter().collect();
        summary
            .models
            .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        summary
    }

    pub fn streak(&self) -> u32 {
        let committed = |date: Date| self.git.get(&date).is_some_and(|day| day.commits > 0);
        let mut date = if committed(self.today) {
            self.today
        } else {
            self.today.yesterday().unwrap_or(self.today)
        };
        let mut streak = 0;
        while committed(date) {
            streak += 1;
            let Ok(previous) = date.yesterday() else {
                break;
            };
            date = previous;
        }
        streak
    }

    pub fn commit_cells(&self) -> Vec<Cell> {
        cells(self.today, |date| {
            self.git.get(&date).map_or(0, |day| u64::from(day.commits))
        })
    }

    pub fn token_cells(&self) -> Vec<Cell> {
        cells(self.today, |date| {
            self.agents.get(&date).map_or(0, |day| day.tokens)
        })
    }

    pub fn yearly_commits(&self) -> u64 {
        self.git.values().map(|day| u64::from(day.commits)).sum()
    }

    pub fn yearly_tokens(&self) -> u64 {
        self.agents.values().map(|day| day.tokens).sum()
    }
}

pub fn first_day(today: Date) -> Date {
    let offset = i64::from(today.weekday().to_sunday_zero_offset());
    today.saturating_sub((7 * (i64::try_from(WEEKS).unwrap_or(53) - 1) + offset).days())
}

fn cells(today: Date, value: impl Fn(Date) -> u64) -> Vec<Cell> {
    let first = first_day(today);
    let dates: Vec<Date> = first
        .series(1.day())
        .take_while(|date| *date <= today)
        .collect();
    let values: Vec<u64> = dates.iter().map(|date| value(*date)).collect();
    let thresholds = thresholds(&values);
    dates
        .into_iter()
        .zip(values)
        .map(|(date, value)| Cell {
            date,
            value,
            level: level(value, &thresholds),
        })
        .collect()
}

fn thresholds(values: &[u64]) -> [u64; 3] {
    let mut active: Vec<u64> = values.iter().copied().filter(|value| *value > 0).collect();
    if active.is_empty() {
        return [0; 3];
    }
    active.sort_unstable();
    let at = |fraction: usize| active[(active.len() - 1) * fraction / 4];
    [at(1), at(2), at(3)]
}

fn level(value: u64, thresholds: &[u64; 3]) -> u8 {
    if value == 0 {
        return 0;
    }
    1 + u8::try_from(thresholds.iter().filter(|limit| value > **limit).count()).unwrap_or(3)
}

pub fn model_name(model: &str) -> String {
    let lower = model.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("claude-") {
        let mut parts = rest.split('-');
        let family = parts.next().unwrap_or_default();
        let version: Vec<&str> = parts
            .take_while(|part| part.len() <= 2 && part.chars().all(|c| c.is_ascii_digit()))
            .collect();
        let mut name = capitalized(family);
        if !version.is_empty() {
            name.push(' ');
            name.push_str(&version.join("."));
        }
        return name;
    }
    if let Some(rest) = lower.strip_prefix("gpt-") {
        return format!("GPT-{rest}");
    }
    model.to_owned()
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn report(commits: &[(Date, u32)]) -> Report {
        Report {
            today: date(2026, 11, 22),
            git: commits
                .iter()
                .map(|(day, commits)| {
                    (
                        *day,
                        GitDay {
                            commits: *commits,
                            ..GitDay::default()
                        },
                    )
                })
                .collect(),
            agents: BTreeMap::new(),
        }
    }

    #[test]
    fn a_streak_survives_a_morning_without_commits_but_not_a_skipped_day() {
        let kept = report(&[
            (date(2026, 11, 19), 1),
            (date(2026, 11, 20), 2),
            (date(2026, 11, 21), 1),
        ]);
        assert_eq!(kept.streak(), 3);
        let broken = report(&[(date(2026, 11, 19), 1), (date(2026, 11, 21), 1)]);
        assert_eq!(broken.streak(), 1);
    }

    #[test]
    fn ranges_count_only_their_own_days_and_compare_to_the_window_before() {
        let report = report(&[
            (date(2026, 11, 22), 4),
            (date(2026, 11, 16), 2),
            (date(2026, 11, 15), 9),
        ]);
        assert_eq!(report.summary(Range::Today).commits, 4);
        assert_eq!(report.summary(Range::Week).commits, 6);
        assert_eq!(report.previous(Range::Week).commits, 9);
    }

    #[test]
    fn the_calendar_ends_today_and_starts_on_a_sunday_a_year_back() {
        let cells = report(&[]).commit_cells();
        assert_eq!(cells.last().map(|cell| cell.date), Some(date(2026, 11, 22)));
        assert_eq!(cells[0].date.weekday(), jiff::civil::Weekday::Sunday);
        assert!(cells.len() > 52 * 7 && cells.len() <= WEEKS * 7);
    }

    #[test]
    fn busier_days_never_get_a_lighter_shade() {
        let values = [0, 1, 2, 3, 5, 8, 13, 21, 34];
        let thresholds = thresholds(&values);
        let levels: Vec<u8> = values.iter().map(|v| level(*v, &thresholds)).collect();
        assert_eq!(levels[0], 0);
        assert!(levels.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(levels.last(), Some(&4));
    }

    #[test]
    fn model_ids_read_as_product_names() {
        assert_eq!(model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(model_name("gpt-5.4"), "GPT-5.4");
    }
}
