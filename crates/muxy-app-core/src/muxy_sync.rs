use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use muxy_protocol::SessionId;
use serde::{Deserialize, Serialize};

use crate::{AppState, ProjectId, ProjectKind, ServerId};

pub const SOURCE_FILE: &str = "desktop-state.json";
pub const SNAPSHOT_FILE: &str = "muxy-sync.json";

#[derive(Debug, Deserialize)]
pub struct Source {
    #[serde(default)]
    projects: Vec<SourceProject>,
    #[serde(default)]
    workspaces: Vec<SourceWorkspace>,
}

#[derive(Debug, Deserialize)]
struct SourceProject {
    id: ProjectId,
    #[serde(default)]
    home: bool,
    name: String,
    #[serde(default)]
    icon: Option<String>,
    color: String,
    server_id: ServerId,
    #[serde(with = "crate::catalog::directory")]
    directory: PathBuf,
    #[serde(default)]
    kind: Option<ProjectKind>,
    #[serde(default)]
    parent_id: Option<ProjectId>,
}

#[derive(Debug, Deserialize)]
struct SourceWorkspace {
    name: String,
    #[serde(default)]
    projects: BTreeSet<ProjectId>,
}

impl Source {
    pub fn load(path: &Path) -> Option<Self> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }

    fn synced(&self) -> impl Iterator<Item = &SourceProject> {
        self.projects.iter().filter(|project| {
            !project.home
                && project.parent_id.is_none()
                && project.kind != Some(ProjectKind::Worktree)
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    projects: BTreeSet<(ServerId, PathBuf)>,
    workspaces: BTreeSet<String>,
}

impl Snapshot {
    pub fn load(path: &Path) -> Option<Self> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }
}

#[derive(Debug, Default)]
pub struct Synced {
    pub snapshot: Snapshot,
    pub discarded: Vec<(ServerId, SessionId)>,
    pub changed: bool,
}

impl AppState {
    pub fn sync_from_muxy(&mut self, source: &Source, previous: Option<&Snapshot>) -> Synced {
        let mut synced = Synced {
            snapshot: Snapshot {
                projects: source
                    .synced()
                    .map(|project| (project.server_id, project.directory.clone()))
                    .collect(),
                workspaces: source
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.name.clone())
                    .collect(),
            },
            ..Synced::default()
        };
        let ids = self.add_synced_projects(source, &mut synced);
        if let Some(previous) = previous {
            self.remove_unsynced_projects(previous, &mut synced);
        }
        self.sync_workspaces(source, &ids, &mut synced);
        if let Some(previous) = previous {
            for name in previous.workspaces.difference(&synced.snapshot.workspaces) {
                let found = self
                    .workspaces()
                    .iter()
                    .find(|workspace| same_name(&workspace.name, name))
                    .map(|workspace| workspace.id);
                if let Some(id) = found
                    && self.delete_workspace(id).is_ok()
                {
                    synced.changed = true;
                }
            }
        }
        synced
    }

    fn add_synced_projects(
        &mut self,
        source: &Source,
        synced: &mut Synced,
    ) -> BTreeMap<ProjectId, ProjectId> {
        let mut ids = BTreeMap::new();
        for project in source.synced() {
            if self.server_home(project.server_id).is_none() {
                continue;
            }
            let id = if let Some(id) = self.top_level(project.server_id, &project.directory) {
                Some(id)
            } else {
                let created = self.create_project(
                    project.server_id,
                    project.directory.clone(),
                    |descriptor| {
                        descriptor.name.clone_from(&project.name);
                        descriptor.color.clone_from(&project.color);
                        descriptor.icon.clone_from(&project.icon);
                    },
                );
                synced.changed |= created.is_ok();
                created.ok()
            };
            if let Some(id) = id {
                ids.insert(project.id, id);
            }
        }
        ids
    }

    fn remove_unsynced_projects(&mut self, previous: &Snapshot, synced: &mut Synced) {
        for (server, directory) in previous.projects.difference(&synced.snapshot.projects) {
            let Some(id) = self.top_level(*server, directory) else {
                continue;
            };
            if self
                .project(id)
                .is_some_and(|project| !project.tabs.is_empty())
            {
                continue;
            }
            if let Ok(sessions) = self.remove_project(id) {
                synced.changed = true;
                synced
                    .discarded
                    .extend(sessions.into_iter().map(|session| (*server, session)));
            }
        }
    }

    fn sync_workspaces(
        &mut self,
        source: &Source,
        ids: &BTreeMap<ProjectId, ProjectId>,
        synced: &mut Synced,
    ) {
        for workspace in &source.workspaces {
            let existing = self
                .workspaces()
                .iter()
                .find(|candidate| same_name(&candidate.name, &workspace.name))
                .map(|candidate| candidate.id);
            let id = match existing {
                Some(id) => id,
                None => match self.create_workspace(&workspace.name) {
                    Ok(id) => {
                        synced.changed = true;
                        id
                    }
                    Err(_) => continue,
                },
            };
            for (source_id, id_here) in ids {
                let member = workspace.projects.contains(source_id);
                let already = self
                    .workspace(id)
                    .is_some_and(|candidate| candidate.projects.contains(id_here));
                if member != already && self.set_workspace_member(id, *id_here, member).is_ok() {
                    synced.changed = true;
                }
            }
        }
    }

    fn top_level(&self, server: ServerId, directory: &Path) -> Option<ProjectId> {
        self.projects()
            .iter()
            .find(|project| {
                project.server_id == server
                    && !project.home
                    && project.parent_id.is_none()
                    && project.directory == directory
            })
            .map(|project| project.id)
    }
}

fn same_name(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(projects: &[(ProjectId, &Path)], workspaces: &[(&str, &[ProjectId])]) -> Source {
        let local = serde_json::to_value(ServerId::local()).unwrap();
        let projects: Vec<serde_json::Value> = projects
            .iter()
            .map(|(id, directory)| {
                serde_json::json!({
                    "id": id,
                    "name": directory.file_name().unwrap().to_string_lossy(),
                    "color": "#f5a623",
                    "server_id": local,
                    "directory": directory.as_os_str().as_encoded_bytes(),
                })
            })
            .collect();
        let workspaces: Vec<serde_json::Value> = workspaces
            .iter()
            .map(|(name, members)| serde_json::json!({"name": name, "projects": members}))
            .collect();
        serde_json::from_value(serde_json::json!({"projects": projects, "workspaces": workspaces}))
            .unwrap()
    }

    fn names(state: &AppState) -> Vec<String> {
        state
            .projects()
            .iter()
            .filter(|project| !project.home)
            .map(|project| project.name.clone())
            .collect()
    }

    #[test]
    fn projects_added_and_removed_in_muxy_follow_without_touching_pane_only_ones() {
        let folders = tempfile::tempdir().unwrap();
        let [alpha, beta, own] = ["alpha", "beta", "own"].map(|name| {
            let path = folders.path().join(name);
            fs::create_dir(&path).unwrap();
            path
        });
        let mut state = AppState::bootstrap().unwrap();
        state.add_project(ServerId::local(), own).unwrap();
        let a = ProjectId::new();
        let b = ProjectId::new();

        let first = state.sync_from_muxy(&source(&[(a, &alpha), (b, &beta)], &[]), None);
        assert!(first.changed);
        assert_eq!(names(&state), ["own", "alpha", "beta"]);

        let again = state.sync_from_muxy(
            &source(&[(a, &alpha), (b, &beta)], &[]),
            Some(&first.snapshot),
        );
        assert!(!again.changed);

        let removed = state.sync_from_muxy(&source(&[(a, &alpha)], &[]), Some(&again.snapshot));
        assert!(removed.changed);
        assert_eq!(names(&state), ["own", "alpha"]);
    }

    #[test]
    fn a_project_with_open_terminals_stays_when_muxy_drops_it() {
        let folders = tempfile::tempdir().unwrap();
        let alpha = folders.path().join("alpha");
        fs::create_dir(&alpha).unwrap();
        let mut state = AppState::bootstrap().unwrap();
        let a = ProjectId::new();
        let first = state.sync_from_muxy(&source(&[(a, &alpha)], &[]), None);
        let project = state
            .projects()
            .iter()
            .find(|project| project.name == "alpha")
            .unwrap()
            .id;
        state.open_terminal_tab(project).unwrap();

        let dropped = state.sync_from_muxy(&source(&[], &[]), Some(&first.snapshot));
        assert!(dropped.discarded.is_empty());
        assert_eq!(names(&state), ["alpha"]);
    }

    #[test]
    fn a_first_sync_never_removes_and_workspaces_follow_by_name() {
        let folders = tempfile::tempdir().unwrap();
        let alpha = folders.path().join("alpha");
        fs::create_dir(&alpha).unwrap();
        let mut state = AppState::bootstrap().unwrap();
        let a = ProjectId::new();

        let existing = state.create_workspace("CLIENTS").unwrap();
        let first = state.sync_from_muxy(&source(&[(a, &alpha)], &[("Clients", &[a])]), None);
        assert_eq!(state.workspaces().len(), 1);
        assert_eq!(state.workspaces()[0].id, existing);
        let workspace = state
            .workspaces()
            .iter()
            .find(|workspace| workspace.name == "CLIENTS")
            .unwrap();
        assert_eq!(workspace.projects.len(), 1);

        state.sync_from_muxy(&source(&[(a, &alpha)], &[]), Some(&first.snapshot));
        assert!(
            state
                .workspaces()
                .iter()
                .all(|workspace| workspace.name != "Clients")
        );
        assert_eq!(names(&state), ["alpha"]);
    }
}
