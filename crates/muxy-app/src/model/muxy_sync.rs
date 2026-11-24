use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::{Context, Task};
use muxy_app_core::muxy_sync::{
    MuxyProject, Push, SERVER_SOCKET, SNAPSHOT_FILE, SOURCE_FILE, Snapshot, Source,
};
use muxy_client::Client;
use muxy_protocol::{OperationId, ProjectIntent, ProjectMutation};

use super::{AppModel, Quitting};

const EVERY: Duration = Duration::from_secs(3);
const CONNECT: Duration = Duration::from_secs(2);
const RECHECK: Duration = Duration::from_secs(30);

type Stamp = (Option<SystemTime>, BTreeSet<PathBuf>);

#[derive(Default)]
pub(crate) struct MuxySync {
    stamp: Option<Stamp>,
    checked: Option<Instant>,
    snapshot: Option<Snapshot>,
    task: Option<Task<()>>,
    timer: Option<Task<()>>,
}

#[derive(Clone)]
struct Paths {
    source: PathBuf,
    socket: PathBuf,
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
        socket: upstream.join(SERVER_SOCKET),
        snapshot: own.join(SNAPSHOT_FILE),
    })
}

fn read_muxy(socket: &Path) -> Option<Vec<MuxyProject>> {
    let client = Client::connect_with_timeout(socket, CONNECT).ok()?;
    let page = client.catalog().ok()?;
    Some(MuxyProject::from_catalog(&page))
}

fn send(socket: &Path, pushes: Vec<Push>) -> bool {
    let Ok(client) = Client::connect_with_timeout(socket, CONNECT) else {
        return false;
    };
    let mut done = true;
    for push in pushes {
        let mutation = match push {
            Push::Create(descriptor) => ProjectMutation::Create(descriptor),
            Push::Delete(id) => match client.all_project_sessions(id, |_| true) {
                Ok(sessions) if sessions.sessions.is_empty() => ProjectMutation::Delete(id),
                _ => {
                    done = false;
                    continue;
                }
            },
        };
        let intent = ProjectIntent {
            operation: OperationId::new(),
            mutation,
        };
        done &= client.mutate_project(intent).is_ok();
    }
    done
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

    fn muxy_stamp(&self, source: &Path) -> Stamp {
        (
            std::fs::metadata(source)
                .and_then(|meta| meta.modified())
                .ok(),
            self.state.local_project_directories(),
        )
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
        let stamp = self.muxy_stamp(&paths.source);
        let recent = self
            .muxy_sync
            .checked
            .is_some_and(|checked| checked.elapsed() < RECHECK);
        if recent && self.muxy_sync.stamp.as_ref() == Some(&stamp) {
            return;
        }
        self.muxy_sync.checked = Some(Instant::now());
        let cached = self.muxy_sync.snapshot.clone();
        let read_paths = paths.clone();
        let read = cx.background_executor().spawn(async move {
            let muxy = read_muxy(&read_paths.socket);
            let source = Source::load(&read_paths.source);
            let snapshot = cached.or_else(|| Snapshot::load(&read_paths.snapshot));
            (muxy, source, snapshot)
        });
        self.muxy_sync.task = Some(cx.spawn(async move |model, cx| {
            let (muxy, source, snapshot) = read.await;
            let pushes = model
                .update(cx, |model, cx| {
                    let reached = muxy.is_some();
                    let (next, pushes) = model.apply_muxy_sync(
                        muxy.as_deref(),
                        source.as_ref(),
                        snapshot.as_ref(),
                        cx,
                    );
                    if pushes.is_empty() {
                        model.keep_muxy_snapshot(next, &paths.snapshot);
                        model.muxy_sync.stamp = reached.then(|| model.muxy_stamp(&paths.source));
                        model.muxy_sync.task = None;
                        return None;
                    }
                    Some((next, pushes))
                })
                .ok()
                .flatten();
            let Some((next, pushes)) = pushes else {
                return;
            };
            let socket = paths.socket.clone();
            let sent = cx
                .background_executor()
                .spawn(async move { send(&socket, pushes) })
                .await;
            let _ = model.update(cx, |model, _| {
                model.muxy_sync.task = None;
                if sent {
                    model.keep_muxy_snapshot(next, &paths.snapshot);
                    model.muxy_sync.stamp = Some(model.muxy_stamp(&paths.source));
                } else {
                    model.muxy_sync.stamp = None;
                }
            });
        }));
    }

    fn keep_muxy_snapshot(&mut self, snapshot: Snapshot, path: &Path) {
        if self.muxy_sync.snapshot.as_ref() != Some(&snapshot) {
            let _ = snapshot.save(path);
            self.muxy_sync.snapshot = Some(snapshot);
        }
    }

    fn apply_muxy_sync(
        &mut self,
        muxy: Option<&[MuxyProject]>,
        source: Option<&Source>,
        previous: Option<&Snapshot>,
        cx: &mut Context<Self>,
    ) -> (Snapshot, Vec<Push>) {
        let mut trial = self.state.clone();
        let dry = trial.sync_with_muxy(muxy, source, previous);
        if !dry.changed {
            return (dry.snapshot, dry.pushes);
        }
        let mut applied = None;
        self.edit_project(
            |state| {
                let synced = state.sync_with_muxy(muxy, source, previous);
                for (server, session) in &synced.discarded {
                    state.queue_discard(*server, *session);
                }
                applied = Some(synced);
                Ok(())
            },
            cx,
        );
        let Some(synced) = applied else {
            return (previous.cloned().unwrap_or_default(), Vec::new());
        };
        let mut servers: Vec<_> = synced.discarded.iter().map(|(server, _)| *server).collect();
        servers.dedup();
        for server in servers {
            self.discard_pending(server, cx);
        }
        (synced.snapshot, synced.pushes)
    }
}
