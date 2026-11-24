use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpui::{Context, Task};
use muxy_app_core::muxy_sync::{SNAPSHOT_FILE, SOURCE_FILE, Snapshot, Source};

use super::{AppModel, Quitting};

const EVERY: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct MuxySync {
    stamp: Option<SystemTime>,
    snapshot: Option<Snapshot>,
    task: Option<Task<()>>,
    timer: Option<Task<()>>,
}

struct Paths {
    source: PathBuf,
    snapshot: PathBuf,
}

fn paths() -> Option<Paths> {
    let upstream = muxy_core::dirs::upstream_muxy_dir().ok()?;
    let own = muxy_core::dirs::muxy_dir().ok()?;
    if upstream == own {
        return None;
    }
    Some(Paths {
        source: upstream.join(SOURCE_FILE),
        snapshot: own.join(SNAPSHOT_FILE),
    })
}

impl AppModel {
    pub(crate) fn start_muxy_sync(&mut self, cx: &mut Context<Self>) {
        if self.muxy_sync.timer.is_some() {
            return;
        }
        self.check_muxy_sync(cx);
        self.muxy_sync.timer = Some(cx.spawn(async move |model, cx| {
            loop {
                cx.background_executor().timer(EVERY).await;
                if model.update(cx, AppModel::check_muxy_sync).is_err() {
                    break;
                }
            }
        }));
    }

    fn check_muxy_sync(&mut self, cx: &mut Context<Self>) {
        if !self.settings.projects.sync_from_muxy
            || self.quitting != Quitting::Idle
            || self.muxy_sync.task.is_some()
        {
            return;
        }
        let Some(paths) = paths() else {
            return;
        };
        let Ok(modified) = std::fs::metadata(&paths.source).and_then(|meta| meta.modified()) else {
            return;
        };
        if self.muxy_sync.stamp == Some(modified) {
            return;
        }
        let cached = self.muxy_sync.snapshot.clone();
        let snapshot_path = paths.snapshot.clone();
        let read = cx.background_executor().spawn(async move {
            let source = Source::load(&paths.source);
            let snapshot = cached.or_else(|| Snapshot::load(&snapshot_path));
            (source, snapshot)
        });
        self.muxy_sync.task = Some(cx.spawn(async move |model, cx| {
            let (source, snapshot) = read.await;
            let _ = model.update(cx, |model, cx| {
                model.muxy_sync.task = None;
                model.muxy_sync.stamp = Some(modified);
                if let Some(source) = source {
                    model.apply_muxy_sync(&source, snapshot.as_ref(), &paths.snapshot, cx);
                }
            });
        }));
    }

    fn apply_muxy_sync(
        &mut self,
        source: &Source,
        previous: Option<&Snapshot>,
        snapshot_path: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        let mut trial = self.state.clone();
        let dry = trial.sync_from_muxy(source, previous);
        let snapshot = if dry.changed {
            let mut applied = None;
            self.edit_project(
                |state| {
                    let synced = state.sync_from_muxy(source, previous);
                    for (server, session) in &synced.discarded {
                        state.queue_discard(*server, *session);
                    }
                    applied = Some(synced);
                    Ok(())
                },
                cx,
            );
            let Some(synced) = applied else {
                return;
            };
            let mut servers: Vec<_> = synced.discarded.iter().map(|(server, _)| *server).collect();
            servers.dedup();
            for server in servers {
                self.discard_pending(server, cx);
            }
            synced.snapshot
        } else {
            dry.snapshot
        };
        if self.muxy_sync.snapshot.as_ref() != Some(&snapshot) {
            let _ = snapshot.save(snapshot_path);
            self.muxy_sync.snapshot = Some(snapshot);
        }
    }
}
