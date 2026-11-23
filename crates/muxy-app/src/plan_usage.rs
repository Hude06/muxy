use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use serde::Deserialize;

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const CREDENTIALS_FILE: &str = ".credentials.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Account {
    pub(crate) name: String,
    pub(crate) directory: PathBuf,
    default: bool,
}

impl Account {
    pub(crate) fn command(&self) -> String {
        if self.default {
            return "claude".to_owned();
        }
        format!(
            "CLAUDE_CONFIG_DIR={} claude",
            crate::extensions::authoring::shell_quote(&self.directory.to_string_lossy())
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Limit {
    pub(crate) used: f32,
    pub(crate) resets_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Status {
    SignedOut,
    Expired,
    Failed(String),
    Ready {
        session: Option<Limit>,
        week: Option<Limit>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reading {
    pub(crate) account: Account,
    pub(crate) plan: Option<String>,
    pub(crate) status: Status,
}

impl Reading {
    pub(crate) fn session_used(&self) -> Option<f32> {
        match &self.status {
            Status::Ready { session, .. } => session.map(|limit| limit.used),
            _ => None,
        }
    }
}

pub(crate) fn accounts(home: &Path) -> Vec<Account> {
    let mut numbered: Vec<(u32, PathBuf)> = fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let number = entry
                .file_name()
                .to_str()?
                .strip_prefix(".claude-")?
                .parse::<u32>()
                .ok()?;
            (number >= 2 && entry.path().is_dir()).then(|| (number, entry.path()))
        })
        .collect();
    numbered.sort();
    let default = home.join(".claude");
    let mut accounts = Vec::new();
    if default.is_dir() {
        accounts.push(Account {
            name: "claude1".to_owned(),
            directory: default,
            default: true,
        });
    }
    accounts.extend(numbered.into_iter().map(|(number, directory)| Account {
        name: format!("claude{number}"),
        directory,
        default: false,
    }));
    accounts
}

pub(crate) fn read(account: &Account) -> Reading {
    let now = Timestamp::now().as_millisecond();
    let mut found: Vec<Credential> = fs::read_to_string(account.directory.join(CREDENTIALS_FILE))
        .ok()
        .and_then(|text| credential(&text))
        .into_iter()
        .collect();
    found.extend(keychain(account).and_then(|text| credential(&text)));
    let Some(credential) = freshest(found) else {
        return Reading {
            account: account.clone(),
            plan: None,
            status: Status::SignedOut,
        };
    };
    let status = if credential.expires_at.is_some_and(|at| at <= now) {
        Status::Expired
    } else {
        fetch(&credential.token)
    };
    Reading {
        account: account.clone(),
        plan: credential.plan.as_deref().map(plan_name),
        status,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Credential {
    token: String,
    expires_at: Option<i64>,
    plan: Option<String>,
}

#[derive(Deserialize)]
struct Stored {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<Oauth>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Oauth {
    access_token: Option<String>,
    expires_at: Option<i64>,
    subscription_type: Option<String>,
}

fn credential(text: &str) -> Option<Credential> {
    let oauth = serde_json::from_str::<Stored>(text).ok()?.oauth?;
    let token = oauth.access_token.filter(|token| !token.is_empty())?;
    Some(Credential {
        token,
        expires_at: oauth.expires_at,
        plan: oauth.subscription_type.filter(|plan| !plan.is_empty()),
    })
}

fn freshest(found: Vec<Credential>) -> Option<Credential> {
    found
        .into_iter()
        .max_by_key(|credential| credential.expires_at.unwrap_or(i64::MAX))
}

fn plan_name(plan: &str) -> String {
    let mut chars = plan.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(target_os = "macos")]
fn keychain(account: &Account) -> Option<String> {
    let service = if account.default {
        KEYCHAIN_SERVICE.to_owned()
    } else {
        let digest = ring::digest::digest(
            &ring::digest::SHA256,
            account.directory.to_string_lossy().as_bytes(),
        );
        let hex = digest
            .as_ref()
            .iter()
            .take(4)
            .fold(String::new(), |mut hex, byte| {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        format!("{KEYCHAIN_SERVICE}-{hex}")
    };
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", &service, "-w"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(not(target_os = "macos"))]
fn keychain(_account: &Account) -> Option<String> {
    None
}

#[derive(Deserialize)]
struct Response {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

fn fetch(token: &str) -> Status {
    let client = crate::proxy::configure(
        reqwest::blocking::Client::builder()
            .user_agent(concat!("Pane/", env!("CARGO_PKG_VERSION")))
            .https_only(true)
            .timeout(Duration::from_secs(15)),
    )
    .and_then(reqwest::blocking::ClientBuilder::build);
    let response = client.and_then(|client| {
        client
            .get(ENDPOINT)
            .bearer_auth(token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
    });
    match response {
        Ok(response) if matches!(response.status().as_u16(), 401 | 403) => Status::Expired,
        Ok(response) if !response.status().is_success() => {
            Status::Failed(format!("HTTP {}", response.status().as_u16()))
        }
        Ok(response) => response.text().map_or_else(
            |error| Status::Failed(error.to_string()),
            |text| usage(&text),
        ),
        Err(error) => Status::Failed(error.to_string()),
    }
}

fn usage(text: &str) -> Status {
    let Ok(response) = serde_json::from_str::<Response>(text) else {
        return Status::Failed("Unexpected answer from Anthropic".to_owned());
    };
    let limit = |window: Option<Window>| {
        let window = window?;
        #[allow(clippy::cast_possible_truncation, reason = "A percentage fits an f32")]
        let used = window.utilization?.clamp(0.0, 100.0) as f32;
        Some(Limit {
            used,
            resets_at: window.resets_at.and_then(|at| at.parse().ok()),
        })
    };
    Status::Ready {
        session: limit(response.five_hour),
        week: limit(response.seven_day),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_numbered_claude_folder_is_an_account_named_like_its_command() {
        let home = tempfile::tempdir().unwrap();
        for folder in [".claude", ".claude-2", ".claude-10", ".claude-code-router"] {
            fs::create_dir(home.path().join(folder)).unwrap();
        }
        fs::write(home.path().join(".claude-3"), "").unwrap();
        let names: Vec<String> = accounts(home.path())
            .into_iter()
            .map(|account| account.name)
            .collect();
        assert_eq!(names, ["claude1", "claude2", "claude10"]);
    }

    #[test]
    fn a_stale_credentials_file_loses_to_the_newer_login() {
        let stale =
            r#"{"claudeAiOauth":{"accessToken":"old","expiresAt":1000,"subscriptionType":"max"}}"#;
        let fresh =
            r#"{"claudeAiOauth":{"accessToken":"new","expiresAt":9000,"subscriptionType":"max"}}"#;
        let found = [stale, fresh]
            .iter()
            .filter_map(|text| credential(text))
            .collect();
        assert_eq!(
            freshest(found).map(|credential| credential.token),
            Some("new".to_owned())
        );
        assert_eq!(credential(r#"{"claudeAiOauth":{"accessToken":""}}"#), None);
    }

    #[test]
    fn usage_reads_both_windows_and_tolerates_missing_ones() {
        let status = usage(
            r#"{"five_hour":{"utilization":23.5,"resets_at":"2026-11-23T21:00:00Z"},"seven_day":{"utilization":140},"seven_day_opus":null}"#,
        );
        let Status::Ready { session, week } = status else {
            panic!("expected usage, got {status:?}");
        };
        assert_eq!(session.map(|limit| limit.used), Some(23.5));
        assert!(session.and_then(|limit| limit.resets_at).is_some());
        assert_eq!(week.map(|limit| limit.used), Some(100.0));
        assert!(matches!(
            usage(r#"{"five_hour":null}"#),
            Status::Ready {
                session: None,
                week: None
            }
        ));
    }
}
