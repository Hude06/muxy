use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use muxy_protocol::AgentProvider;
use serde::{Deserialize, Serialize};

use super::AgentDay;
use crate::agent_sessions::Roots;

const VERSION: u32 = 1;
const MAX_DEPTH: usize = 5;
const IDLE_GAP_SECONDS: i64 = 5 * 60;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    version: u32,
    files: HashMap<PathBuf, Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    length: u64,
    modified: i64,
    days: BTreeMap<Date, AgentDay>,
}

impl Cache {
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|cache| cache.version == VERSION)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }

    pub(super) fn scan(
        &mut self,
        roots: &Roots,
        zone: &TimeZone,
        since: Date,
    ) -> BTreeMap<Date, AgentDay> {
        let start = zone
            .to_timestamp(since.to_datetime(jiff::civil::Time::midnight()))
            .unwrap_or(Timestamp::UNIX_EPOCH);
        self.version = VERSION;
        let mut seen = HashSet::new();
        for provider in [AgentProvider::Claude, AgentProvider::Codex] {
            let Some(root) = roots.root(provider) else {
                continue;
            };
            let mut files = Vec::new();
            walk(root, 0, &mut files);
            for path in files {
                let Some((length, modified)) = stamp(&path) else {
                    continue;
                };
                if modified < start.as_second() {
                    continue;
                }
                seen.insert(path.clone());
                if self
                    .files
                    .get(&path)
                    .is_some_and(|entry| entry.length == length && entry.modified == modified)
                {
                    continue;
                }
                let days = read(provider, &path, zone).unwrap_or_default();
                self.files.insert(
                    path,
                    Entry {
                        length,
                        modified,
                        days,
                    },
                );
            }
        }
        self.files.retain(|path, _| seen.contains(path));
        let mut totals: BTreeMap<Date, AgentDay> = BTreeMap::new();
        for entry in self.files.values() {
            for (date, day) in entry.days.range(since..) {
                totals.entry(*date).or_default().merge(day);
            }
        }
        totals
    }
}

fn walk(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(&path, depth + 1, files);
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            files.push(path);
        }
    }
}

fn stamp(path: &Path) -> Option<(u64, i64)> {
    let metadata = fs::metadata(path).ok()?;
    let modified = Timestamp::try_from(metadata.modified().ok()?).ok()?;
    Some((metadata.len(), modified.as_second()))
}

struct Event {
    at: Timestamp,
    tokens: u64,
    model: Option<String>,
    directory: Option<PathBuf>,
}

fn read(
    provider: AgentProvider,
    path: &Path,
    zone: &TimeZone,
) -> io::Result<BTreeMap<Date, AgentDay>> {
    let reader = BufReader::new(fs::File::open(path)?);
    let events = match provider {
        AgentProvider::Claude => claude(reader),
        _ => codex(reader),
    };
    let subagent = path
        .components()
        .any(|part| part.as_os_str() == "subagents");
    Ok(days(&events, zone, !subagent))
}

fn days(events: &[Event], zone: &TimeZone, counts_as_session: bool) -> BTreeMap<Date, AgentDay> {
    let mut days: BTreeMap<Date, AgentDay> = BTreeMap::new();
    let mut previous: Option<Timestamp> = None;
    for event in events {
        let date = zone.to_datetime(event.at).date();
        let day = days.entry(date).or_default();
        day.tokens += event.tokens;
        if let Some(model) = &event.model {
            *day.models.entry(model.clone()).or_default() += event.tokens;
        }
        if let Some(directory) = &event.directory {
            day.directories.insert(directory.clone());
        }
        if let Some(before) = previous {
            let gap = event.at.as_second() - before.as_second();
            if (0..=IDLE_GAP_SECONDS).contains(&gap) {
                day.working_seconds += u64::try_from(gap).unwrap_or(0);
            }
        }
        previous = Some(event.at);
    }
    if counts_as_session {
        for day in days.values_mut() {
            day.sessions = 1;
        }
    }
    days
}

#[derive(Deserialize)]
struct ClaudeLine {
    timestamp: Option<Timestamp>,
    #[serde(rename = "requestId")]
    request: Option<String>,
    cwd: Option<PathBuf>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
#[allow(
    clippy::struct_field_names,
    reason = "Field names mirror the Claude Code log format"
)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

fn claude(reader: impl BufRead) -> Vec<Event> {
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<ClaudeLine>(&line) else {
            continue;
        };
        let (Some(at), Some(message)) = (parsed.timestamp, parsed.message) else {
            continue;
        };
        let Some(usage) = message.usage else {
            continue;
        };
        if let Some(id) = &message.id
            && !seen.insert((id.clone(), parsed.request.clone()))
        {
            continue;
        }
        let model = message
            .model
            .filter(|model| !model.starts_with('<') && !model.is_empty());
        events.push(Event {
            at,
            tokens: usage.input_tokens
                + usage.output_tokens
                + usage.cache_creation_input_tokens
                + usage.cache_read_input_tokens,
            model,
            directory: parsed.cwd,
        });
    }
    events
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<Timestamp>,
    #[serde(rename = "type")]
    kind: String,
    payload: Option<serde_json::Value>,
}

fn codex(reader: impl BufRead) -> Vec<Event> {
    let mut events = Vec::new();
    let mut model = None;
    let mut directory = None;
    for line in reader.lines().map_while(Result::ok) {
        let interesting = line.contains("\"last_token_usage\"")
            || line.contains("\"turn_context\"")
            || line.contains("\"session_meta\"");
        if !interesting {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<CodexLine>(&line) else {
            continue;
        };
        let Some(payload) = parsed.payload else {
            continue;
        };
        match parsed.kind.as_str() {
            "session_meta" | "turn_context" => {
                if let Some(cwd) = payload.get("cwd").and_then(|cwd| cwd.as_str()) {
                    directory = Some(PathBuf::from(cwd));
                }
                if let Some(name) = payload.get("model").and_then(|name| name.as_str()) {
                    model = Some(name.to_owned());
                }
            }
            "event_msg" => {
                let tokens = payload
                    .pointer("/info/last_token_usage/total_tokens")
                    .and_then(serde_json::Value::as_u64);
                if let (Some(at), Some(tokens)) = (parsed.timestamp, tokens) {
                    events.push(Event {
                        at,
                        tokens,
                        model: model.clone(),
                        directory: directory.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn zone() -> TimeZone {
        TimeZone::get("America/Los_Angeles").unwrap_or(TimeZone::UTC)
    }

    const CLAUDE: &str = r#"{"type":"user","timestamp":"2026-11-22T06:00:00Z","message":{"content":"hi"}}
{"type":"assistant","timestamp":"2026-11-22T06:00:10Z","requestId":"r1","cwd":"/work/a","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000}}}
{"type":"assistant","timestamp":"2026-11-22T06:00:11Z","requestId":"r1","cwd":"/work/a","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000}}}
{"type":"assistant","timestamp":"2026-11-22T09:00:00Z","requestId":"r2","cwd":"/work/a","message":{"id":"m2","model":"claude-opus-5-5","usage":{"input_tokens":1,"output_tokens":1}}}
"#;

    #[test]
    fn repeated_claude_entries_for_one_message_count_once() {
        let events = claude(CLAUDE.as_bytes());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].tokens, 1115);
    }

    #[test]
    fn activity_lands_on_the_local_calendar_day() {
        let days = days(&claude(CLAUDE.as_bytes()), &zone(), true);
        assert_eq!(days[&date(2026, 11, 21)].tokens, 1115);
        assert_eq!(days[&date(2026, 11, 22)].tokens, 2);
        assert_eq!(days[&date(2026, 11, 22)].sessions, 1);
    }

    #[test]
    fn codex_usage_carries_the_model_and_folder_of_its_turn() {
        let log = r#"{"timestamp":"2026-06-09T17:06:44Z","type":"session_meta","payload":{"cwd":"/work/b"}}
{"timestamp":"2026-06-09T17:06:45Z","type":"turn_context","payload":{"model":"gpt-5.4","cwd":"/work/b"}}
{"timestamp":"2026-06-09T17:06:55Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":13248}}}}
{"timestamp":"2026-06-09T17:07:00Z","type":"event_msg","payload":{"type":"token_count","info":null}}
"#;
        let events = codex(log.as_bytes());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tokens, 13248);
        assert_eq!(events[0].model.as_deref(), Some("gpt-5.4"));
        assert_eq!(events[0].directory.as_deref(), Some(Path::new("/work/b")));
    }

    #[test]
    fn a_grown_log_is_read_again_and_a_deleted_one_is_forgotten() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".claude/projects/work");
        fs::create_dir_all(&folder).unwrap();
        let log = folder.join("session.jsonl");
        let first = CLAUDE.lines().take(2).collect::<Vec<_>>().join("\n") + "\n";
        fs::write(&log, first).unwrap();
        let roots = Roots::from_home(home.path());
        let mut cache = Cache::default();
        let since = date(2026, 1, 1);
        let total = |cache: &mut Cache| -> u64 {
            cache
                .scan(&roots, &zone(), since)
                .values()
                .map(|day| day.tokens)
                .sum()
        };
        assert_eq!(total(&mut cache), 1115);
        fs::write(&log, CLAUDE).unwrap();
        assert_eq!(total(&mut cache), 1117);
        fs::remove_file(&log).unwrap();
        assert_eq!(total(&mut cache), 0);
    }
}
