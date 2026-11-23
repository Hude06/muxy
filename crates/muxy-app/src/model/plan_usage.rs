use std::time::{Duration, Instant};

use gpui::{Context, Task};
use muxy_ui::tr;

use super::AppModel;
use crate::plan_usage::{Reading, accounts, read};

const EVERY: Duration = Duration::from_secs(300);

#[derive(Default)]
pub(crate) struct PlanUsage {
    pub(crate) readings: Vec<Reading>,
    pub(crate) checked: Option<Instant>,
    task: Option<Task<()>>,
    timer: Option<Task<()>>,
}

impl PlanUsage {
    pub(crate) fn loading(&self) -> bool {
        self.task.is_some()
    }

    pub(crate) fn roomiest(&self) -> Option<usize> {
        let ready: Vec<(usize, f32)> = self
            .readings
            .iter()
            .enumerate()
            .filter_map(|(index, reading)| Some((index, reading.session_used()?)))
            .collect();
        if ready.len() < 2 {
            return None;
        }
        ready
            .into_iter()
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(index, _)| index)
    }
}

impl AppModel {
    pub(crate) fn start_plan_usage(&mut self, cx: &mut Context<Self>) {
        if self.plan_usage.timer.is_some() {
            return;
        }
        self.refresh_plan_usage(cx);
        self.plan_usage.timer = Some(cx.spawn(async move |model, cx| {
            loop {
                cx.background_executor().timer(EVERY).await;
                if model.update(cx, AppModel::refresh_plan_usage).is_err() {
                    break;
                }
            }
        }));
    }

    pub(crate) fn refresh_plan_usage(&mut self, cx: &mut Context<Self>) {
        if self.plan_usage.task.is_some() {
            return;
        }
        let Ok(home) = muxy_core::paths::home_dir() else {
            return;
        };
        let scan = cx
            .background_executor()
            .spawn(async move { accounts(&home).iter().map(read).collect::<Vec<_>>() });
        self.plan_usage.task = Some(cx.spawn(async move |model, cx| {
            let readings = scan.await;
            let _ = model.update(cx, |model, cx| {
                model.plan_usage.task = None;
                model.plan_usage.readings = readings;
                model.plan_usage.checked = Some(Instant::now());
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn start_plan_account(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(account) = self
            .plan_usage
            .readings
            .get(index)
            .map(|reading| reading.account.clone())
        else {
            return;
        };
        let command = account.command();
        let project = self.state.current_project().id;
        let opened = self.open_terminal(
            super::tabs::OpenTerminal {
                project,
                title: Some(&account.name),
                command: Some(&command),
                directory: None,
            },
            cx,
        );
        if let Err(error) = opened {
            self.fail(
                tr!("Could not start %@: %@", &account.name, &error).to_string(),
                cx,
            );
        }
    }
}
