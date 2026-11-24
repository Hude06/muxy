use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use muxy_protocol::{CatalogPage, ProjectDescriptor, SessionId};
use serde::{Deserialize, Serialize};

use crate::{AppState, ProjectId, ProjectKind, ServerId};

pub const SOURCE_FILE: &str = "desktop-state.json";
pub const SERVER_SOCKET: &str = "server.sock";
pub const SNAPSHOT_FILE: &str = "pane-muxy-sync.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MuxyProject {
    pub id: ProjectId,
    pub directory: PathBuf,
    name: String,
    color: String,
    icon: Option<String>,
}

impl MuxyProject {
    pub fn from_catalog(page: &CatalogPage) -> Vec<Self> {
        page.projects
            .iter()
            .filter(|project| {
                !project.home
                    && project.parent_id.is_none()
                    && project.kind != Some(ProjectKind::Worktree)
            })
            .filter_map(|project| {
                Some(Self {
                    id: project.id,
                    directory: project.directory.to_path_buf().ok()?,
                    name: project.name.clone(),
                    color: project.color.clone(),
                    icon: project.icon.clone(),
                })
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Push {
    Create(ProjectDescriptor),
    Delete(ProjectId),
}

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
    server_id: ServerId,
    #[serde(with = "crate::catalog::directory")]
    directory: PathBuf,
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
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    projects: BTreeSet<PathBuf>,
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
    pub pushes: Vec<Push>,
    pub changed: bool,
}

impl AppState {
    pub fn local_project_directories(&self) -> BTreeSet<PathBuf> {
        self.synced_projects().into_keys().collect()
    }

    pub fn sync_with_muxy(
        &mut self,
        muxy: Option<&[MuxyProject]>,
        source: Option<&Source>,
        previous: Option<&Snapshot>,
    ) -> Synced {
        let mut synced = Synced::default();
        if let Some(previous) = previous {
            synced.snapshot.projects.clone_from(&previous.projects);
        }
        if let Some(muxy) = muxy {
            self.sync_projects(muxy, previous, &mut synced);
        }
        if let Some(source) = source {
            self.sync_workspaces(source, previous, &mut synced);
        } else if let Some(previous) = previous {
            synced.snapshot.workspaces.clone_from(&previous.workspaces);
        }
        synced
    }

    fn synced_projects(&self) -> BTreeMap<PathBuf, ProjectId> {
        self.projects()
            .iter()
            .filter(|project| {
                project.server_id.is_local()
                    && !project.home
                    && project.parent_id.is_none()
                    && project.kind != Some(ProjectKind::Worktree)
            })
            .map(|project| (project.directory.clone(), project.id))
            .collect()
    }

    fn sync_projects(
        &mut self,
        muxy: &[MuxyProject],
        previous: Option<&Snapshot>,
        synced: &mut Synced,
    ) {
        let known = |directory: &Path| {
            previous.is_some_and(|previous| previous.projects.contains(directory))
        };
        let here = self.synced_projects();
        let there: BTreeMap<&Path, &MuxyProject> = muxy
            .iter()
            .map(|project| (project.directory.as_path(), project))
            .collect();
        let mut kept = BTreeSet::new();
        for project in muxy {
            if here.contains_key(&project.directory) {
                kept.insert(project.directory.clone());
                continue;
            }
            if known(&project.directory) {
                synced.pushes.push(Push::Delete(project.id));
                continue;
            }
            let created =
                self.create_project(ServerId::local(), project.directory.clone(), |descriptor| {
                    descriptor.name.clone_from(&project.name);
                    descriptor.color.clone_from(&project.color);
                    descriptor.icon.clone_from(&project.icon);
                });
            if created.is_ok() {
                synced.changed = true;
                kept.insert(project.directory.clone());
            }
        }
        for (directory, id) in &here {
            if there.contains_key(directory.as_path()) {
                continue;
            }
            if !known(directory) {
                if let Some(descriptor) = self.descriptor_for(*id, &there) {
                    synced.pushes.push(Push::Create(descriptor));
                    kept.insert(directory.clone());
                }
                continue;
            }
            if self
                .project(*id)
                .is_some_and(|project| !project.tabs.is_empty())
            {
                kept.insert(directory.clone());
                continue;
            }
            if let Ok(sessions) = self.remove_project(*id) {
                synced.changed = true;
                synced.discarded.extend(
                    sessions
                        .into_iter()
                        .map(|session| (ServerId::local(), session)),
                );
            }
        }
        synced.snapshot.projects = kept;
    }

    fn descriptor_for(
        &self,
        id: ProjectId,
        there: &BTreeMap<&Path, &MuxyProject>,
    ) -> Option<ProjectDescriptor> {
        let project = self.project(id)?;
        let path = crate::catalog::checked_server_path(&project.directory).ok()?;
        let mut descriptor =
            ProjectDescriptor::new(path, there.values().map(|project| project.color.as_str()));
        descriptor.name.clone_from(&project.name);
        project.color.as_str().clone_into(&mut descriptor.color);
        descriptor.icon.clone_from(&project.icon);
        descriptor.logo.clone_from(&project.logo);
        Some(descriptor)
    }

    fn sync_workspaces(
        &mut self,
        source: &Source,
        previous: Option<&Snapshot>,
        synced: &mut Synced,
    ) {
        let here = self.synced_projects();
        let ids: BTreeMap<ProjectId, ProjectId> = source
            .projects
            .iter()
            .filter(|project| project.server_id.is_local())
            .filter_map(|project| Some((project.id, *here.get(&project.directory)?)))
            .collect();
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
            for (source_id, id_here) in &ids {
                let member = workspace.projects.contains(source_id);
                let already = self
                    .workspace(id)
                    .is_some_and(|candidate| candidate.projects.contains(id_here));
                if member != already && self.set_workspace_member(id, *id_here, member).is_ok() {
                    synced.changed = true;
                }
            }
        }
        synced.snapshot.workspaces = source
            .workspaces
            .iter()
            .map(|workspace| workspace.name.clone())
            .collect();
        let Some(previous) = previous else {
            return;
        };
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
}

fn same_name(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Folders {
        _root: tempfile::TempDir,
        paths: Vec<PathBuf>,
    }

    fn folders(names: &[&str]) -> Folders {
        let root = tempfile::tempdir().unwrap();
        let paths = names
            .iter()
            .map(|name| {
                let path = root.path().join(name);
                fs::create_dir(&path).unwrap();
                path
            })
            .collect();
        Folders { _root: root, paths }
    }

    fn muxy(paths: &[&PathBuf]) -> Vec<MuxyProject> {
        paths
            .iter()
            .map(|path| MuxyProject {
                id: ProjectId::new(),
                directory: (*path).clone(),
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                color: "#f5a623".to_owned(),
                icon: None,
            })
            .collect()
    }

    fn names(state: &AppState) -> Vec<String> {
        state
            .projects()
            .iter()
            .filter(|project| !project.home)
            .map(|project| project.name.clone())
            .collect()
    }

    fn created(synced: &Synced) -> Vec<PathBuf> {
        synced
            .pushes
            .iter()
            .filter_map(|push| match push {
                Push::Create(descriptor) => descriptor.directory.to_path_buf().ok(),
                Push::Delete(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_first_sync_joins_both_sides_and_deletes_nothing() {
        let folders = folders(&["alpha", "own"]);
        let [alpha, own] = [&folders.paths[0], &folders.paths[1]];
        let mut state = AppState::bootstrap().unwrap();
        state.add_project(ServerId::local(), own.clone()).unwrap();

        let first = state.sync_with_muxy(Some(&muxy(&[alpha])), None, None);
        assert_eq!(names(&state), ["own", "alpha"]);
        assert_eq!(created(&first), std::slice::from_ref(own));
        assert_eq!(first.snapshot.projects.len(), 2);
    }

    #[test]
    fn deleting_on_either_side_deletes_on_the_other() {
        let folders = folders(&["alpha", "beta"]);
        let [alpha, beta] = [&folders.paths[0], &folders.paths[1]];
        let mut state = AppState::bootstrap().unwrap();
        let both = muxy(&[alpha, beta]);
        let first = state.sync_with_muxy(Some(&both), None, None);
        assert!(first.pushes.is_empty());

        let gone_in_muxy = state.sync_with_muxy(Some(&both[..1]), None, Some(&first.snapshot));
        assert_eq!(names(&state), ["alpha"]);
        assert!(gone_in_muxy.pushes.is_empty());

        let alpha_here = state
            .projects()
            .iter()
            .find(|project| project.name == "alpha")
            .unwrap()
            .id;
        state.remove_project(alpha_here).unwrap();
        let gone_here = state.sync_with_muxy(Some(&both[..1]), None, Some(&gone_in_muxy.snapshot));
        assert_eq!(gone_here.pushes, [Push::Delete(both[0].id)]);
        assert!(names(&state).is_empty());
    }

    #[test]
    fn a_project_deleted_and_added_again_is_added_on_the_other_side() {
        let folders = folders(&["alpha"]);
        let alpha = &folders.paths[0];
        let mut state = AppState::bootstrap().unwrap();
        let projects = muxy(&[alpha]);
        let first = state.sync_with_muxy(Some(&projects), None, None);
        let gone = state.sync_with_muxy(Some(&[]), None, Some(&first.snapshot));
        assert!(names(&state).is_empty());
        assert!(gone.snapshot.projects.is_empty());

        state.add_project(ServerId::local(), alpha.clone()).unwrap();
        let back = state.sync_with_muxy(Some(&[]), None, Some(&gone.snapshot));
        assert_eq!(names(&state), ["alpha"]);
        assert_eq!(created(&back), std::slice::from_ref(alpha));
    }

    #[test]
    fn a_project_with_open_terminals_stays_when_muxy_drops_it() {
        let folders = folders(&["alpha"]);
        let alpha = &folders.paths[0];
        let mut state = AppState::bootstrap().unwrap();
        let first = state.sync_with_muxy(Some(&muxy(&[alpha])), None, None);
        let project = state
            .projects()
            .iter()
            .find(|project| project.name == "alpha")
            .unwrap()
            .id;
        state.open_terminal_tab(project).unwrap();

        let dropped = state.sync_with_muxy(Some(&[]), None, Some(&first.snapshot));
        assert!(dropped.discarded.is_empty());
        assert!(dropped.pushes.is_empty());
        assert_eq!(names(&state), ["alpha"]);
    }

    #[test]
    fn workspaces_follow_muxy_by_name_ignoring_case() {
        let folders = folders(&["alpha"]);
        let alpha = &folders.paths[0];
        let mut state = AppState::bootstrap().unwrap();
        let existing = state.create_workspace("CLIENTS").unwrap();
        let projects = muxy(&[alpha]);
        let source = |workspaces: serde_json::Value| -> Source {
            serde_json::from_value(serde_json::json!({
                "projects": [{
                    "id": projects[0].id,
                    "server_id": ServerId::local(),
                    "directory": alpha.as_os_str().as_encoded_bytes(),
                }],
                "workspaces": workspaces,
            }))
            .unwrap()
        };
        let with = source(serde_json::json!([{"name": "Clients", "projects": [projects[0].id]}]));
        let first = state.sync_with_muxy(Some(&projects), Some(&with), None);
        assert_eq!(state.workspaces().len(), 1);
        assert_eq!(state.workspaces()[0].id, existing);
        assert_eq!(state.workspaces()[0].projects.len(), 1);

        state.sync_with_muxy(
            Some(&projects),
            Some(&source(serde_json::json!([]))),
            Some(&first.snapshot),
        );
        assert!(state.workspaces().is_empty());
    }
}
