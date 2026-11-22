use crate::{AppError, AppState, PaneContent, PaneId, Tab, TabId};

impl AppState {
    pub fn overview_tab(&self) -> Option<TabId> {
        self.home()
            .tabs
            .iter()
            .find(|tab| {
                tab.panes
                    .iter()
                    .any(|pane| pane.content == PaneContent::Overview)
            })
            .map(|tab| tab.id)
    }

    pub fn is_overview_pane(&self, pane: PaneId) -> bool {
        self.projects
            .iter()
            .flat_map(|project| &project.tabs)
            .flat_map(|tab| &tab.panes)
            .any(|candidate| candidate.id == pane && candidate.content == PaneContent::Overview)
    }

    pub fn show_overview_tab(&mut self, title: &str) -> Result<TabId, AppError> {
        let home = self.home().id;
        let tab = if let Some(tab) = self.overview_tab() {
            tab
        } else {
            let tab = Tab::with_content(PaneContent::Overview, title);
            let id = tab.id;
            self.add_tab(home, tab, None)?;
            let project = self.project_mut(home)?;
            if let Some(index) = project.tabs.iter().position(|tab| tab.id == id) {
                let tab = project.tabs.remove(index);
                project.tabs.insert(0, tab);
            }
            id
        };
        if self.tab_mut(tab)?.window.is_none() {
            self.select_project(home)?;
            self.select_tab(home, tab)?;
        }
        if let Some(pane) = self
            .tab_mut(tab)?
            .panes
            .iter()
            .find(|pane| pane.content == PaneContent::Overview)
            .map(|pane| pane.id)
        {
            self.focus_pane(pane)?;
        }
        Ok(tab)
    }
}
