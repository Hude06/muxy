use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use gpui::{Context, Task};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use muxy_app_core::agent_sessions::{Catalog, Roots, Session};
use muxy_app_core::insights::{Cache, Digest, Range, Report};
use muxy_app_core::{ProjectId, ProjectStatus};
use muxy_ui::tr;

use super::{AppModel, Quitting};

const STALE_AFTER: Duration = Duration::from_secs(120);
const RECENT_LIMIT: usize = 5;
const CACHE_FILE: &str = "insights-cache.json";

#[derive(Default)]
pub(crate) struct Insights {
    pub(crate) digest: Option<Digest>,
    pub(crate) recent: Vec<Recent>,
    pub(crate) range: Range,
    pub(crate) refreshed: Option<Instant>,
    catalog: Option<Catalog>,
    task: Option<Task<()>>,
    timer: Option<Task<()>>,
}

#[derive(Clone, Debug)]
pub(crate) struct Recent {
    pub(crate) session: Session,
    pub(crate) project: ProjectId,
}

impl Insights {
    pub(crate) fn loading(&self) -> bool {
        self.task.is_some()
    }
}

impl AppModel {
    pub(crate) fn launch(
        boot: crate::boot::Boot,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut model = Self::new(boot, window, cx);
        model.open_overview(cx);
        model.start_plan_usage(cx);
        model
    }

    pub(crate) fn open_overview(&mut self, cx: &mut Context<Self>) {
        if self.quitting != Quitting::Idle {
            return;
        }
        match self.transact(|state| state.show_overview_tab(&tr!("Overview")), cx) {
            Ok(_) => {}
            Err(error) => {
                self.fail(error.to_string(), cx);
                return;
            }
        }
        self.changed(cx);
        self.refresh_insights(false, cx);
        self.start_insights_timer(cx);
    }

    pub(crate) fn set_insights_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if self.insights.range != range {
            self.insights.range = range;
            cx.notify();
        }
    }

    fn start_insights_timer(&mut self, cx: &mut Context<Self>) {
        if self.insights.timer.is_some() {
            return;
        }
        self.insights.timer = Some(cx.spawn(async move |model, cx| {
            loop {
                cx.background_executor().timer(STALE_AFTER).await;
                let alive = model.update(cx, |model, cx| {
                    if model.state.overview_tab().is_some() {
                        model.refresh_insights(false, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    pub(crate) fn refresh_insights(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.insights.task.is_some() {
            return;
        }
        let fresh = self
            .insights
            .refreshed
            .is_some_and(|refreshed| refreshed.elapsed() < STALE_AFTER);
        if fresh && !force {
            return;
        }
        let Some(roots) = Roots::from_environment() else {
            return;
        };
        let projects: Vec<(ProjectId, PathBuf)> = self
            .state
            .projects()
            .iter()
            .filter(|project| {
                project.server_id.is_local() && project.status() == ProjectStatus::Available
            })
            .map(|project| {
                (
                    project.id,
                    muxy_core::files::canonicalize(&project.directory)
                        .unwrap_or_else(|_| project.directory.clone()),
                )
            })
            .collect();
        let mut catalog = self.insights.catalog.take().unwrap_or_default();
        let path = muxy_core::dirs::muxy_dir()
            .ok()
            .map(|directory| directory.join(CACHE_FILE));
        let scan = cx.background_executor().spawn(async move {
            let mut cache = path.as_deref().map(Cache::load).unwrap_or_default();
            let zone = TimeZone::system();
            let today = zone.to_datetime(Timestamp::now()).date();
            let directories: Vec<PathBuf> = projects.iter().map(|(_, path)| path.clone()).collect();
            let report = Report::collect(&mut cache, &roots, &directories, &zone, today);
            if let Some(path) = &path {
                let _ = cache.save(path);
            }
            drop(cache);
            let digest = Digest::from(&report);
            drop(report);
            let mut recent: Vec<Recent> = projects
                .iter()
                .flat_map(|(project, directory)| {
                    catalog
                        .scan(&roots, directory)
                        .sessions
                        .into_iter()
                        .filter(|session| !session.title.trim_start().starts_with('<'))
                        .map(|session| Recent {
                            session,
                            project: *project,
                        })
                })
                .collect();
            recent.sort_by_key(|recent| std::cmp::Reverse(recent.session.modified));
            recent.dedup_by(|left, right| left.session.path == right.session.path);
            recent.truncate(RECENT_LIMIT);
            (catalog, digest, recent)
        });
        self.insights.task = Some(cx.spawn(async move |model, cx| {
            let (catalog, digest, recent) = scan.await;
            let _ = model.update(cx, |model, cx| {
                model.insights.task = None;
                model.insights.catalog = Some(catalog);
                model.insights.digest = Some(digest);
                model.insights.recent = recent;
                model.insights.refreshed = Some(Instant::now());
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn resume_recent(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(recent) = self.insights.recent.get(index).cloned() else {
            return;
        };
        let Some(command) = recent.session.resume_command() else {
            return;
        };
        if !recent.session.path.is_file() {
            self.fail(
                tr!("This session is no longer available. Refresh the session list.").to_string(),
                cx,
            );
            return;
        }
        let project = if self.state.project(recent.project).is_some() {
            recent.project
        } else {
            self.state.home().id
        };
        let opened = self.open_terminal(
            super::tabs::OpenTerminal {
                project,
                title: Some(&recent.session.title),
                command: Some(&command),
                directory: Some(&recent.session.directory),
            },
            cx,
        );
        if let Err(error) = opened {
            self.fail(error, cx);
        }
    }
}

pub(crate) fn ago(modified: SystemTime) -> Duration {
    SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
}
