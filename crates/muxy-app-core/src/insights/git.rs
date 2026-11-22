use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use super::GitDay;

const RECORD: char = '\u{1e}';

pub(super) fn scan(
    repositories: &[PathBuf],
    zone: &TimeZone,
    since: Date,
) -> BTreeMap<Date, GitDay> {
    let roots: Vec<PathBuf> = parallel(repositories, |directory| {
        git(directory, &["rev-parse", "--show-toplevel"]).map(|root| PathBuf::from(root.trim()))
    })
    .into_iter()
    .flatten()
    .collect::<std::collections::BTreeSet<_>>()
    .into_iter()
    .collect();
    let logs = parallel(&roots, |root| log(root, since));
    let mut days: BTreeMap<Date, GitDay> = BTreeMap::new();
    let mut commits = HashSet::new();
    for (root, log) in roots.iter().zip(logs) {
        for commit in log.as_deref().map(parse).unwrap_or_default() {
            if !commits.insert(commit.hash.clone()) {
                continue;
            }
            let date = zone.to_datetime(commit.at).date();
            let day = days.entry(date).or_default();
            day.commits += 1;
            day.added += commit.added;
            day.removed += commit.removed;
            day.repositories.insert(root.clone());
        }
    }
    days
}

fn parallel<T: Send>(paths: &[PathBuf], work: impl Fn(&Path) -> T + Sync) -> Vec<T> {
    let workers = std::thread::available_parallelism().map_or(4, usize::from);
    let chunk = paths.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|chunk| {
                let work = &work;
                scope.spawn(move || chunk.iter().map(|path| work(path)).collect::<Vec<_>>())
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

fn log(root: &Path, since: Date) -> Option<String> {
    let since = format!("--since={since}T00:00:00");
    let email = git(root, &["config", "user.email"]).map(|email| email.trim().to_owned());
    let author = email
        .filter(|email| !email.is_empty())
        .map(|email| format!("--author=<{email}>"));
    let mut arguments = vec![
        "log",
        "--all",
        "--no-merges",
        "--fixed-strings",
        since.as_str(),
        "--pretty=format:%x1e%H %ct",
        "--shortstat",
    ];
    if let Some(author) = &author {
        arguments.push(author.as_str());
    }
    git(root, &arguments)
}

fn git(directory: &Path, arguments: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Debug, PartialEq, Eq)]
struct Commit {
    hash: String,
    at: Timestamp,
    added: u64,
    removed: u64,
}

fn parse(log: &str) -> Vec<Commit> {
    log.split(RECORD)
        .filter_map(|record| {
            let mut lines = record.lines().filter(|line| !line.trim().is_empty());
            let (hash, seconds) = lines.next()?.split_once(' ')?;
            let at = Timestamp::from_second(seconds.trim().parse().ok()?).ok()?;
            let (added, removed) = lines.next().map_or((0, 0), shortstat);
            Some(Commit {
                hash: hash.to_owned(),
                at,
                added,
                removed,
            })
        })
        .collect()
}

fn shortstat(line: &str) -> (u64, u64) {
    let mut added = 0;
    let mut removed = 0;
    for part in line.split(',') {
        let part = part.trim();
        let Some((count, label)) = part.split_once(' ') else {
            continue;
        };
        let Ok(count) = count.parse::<u64>() else {
            continue;
        };
        if label.starts_with("insertion") {
            added = count;
        } else if label.starts_with("deletion") {
            removed = count;
        }
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_keep_their_line_counts_even_when_one_side_is_missing() {
        let log = "\u{1e}aaa 1795392000\n\n 3 files changed, 120 insertions(+), 7 deletions(-)\n\
                   \u{1e}bbb 1795390000\n\n 1 file changed, 4 deletions(-)\n\
                   \u{1e}ccc 1795380000\n";
        let commits = parse(log);
        assert_eq!(commits.len(), 3);
        assert_eq!((commits[0].added, commits[0].removed), (120, 7));
        assert_eq!((commits[1].added, commits[1].removed), (0, 4));
        assert_eq!((commits[2].added, commits[2].removed), (0, 0));
    }
}
