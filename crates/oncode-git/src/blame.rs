//! Blame: parser for `git blame --incremental`.

use std::collections::HashMap;

use crate::error::Result;
use crate::model::{BlameRange, Timestamp};
use crate::repo::Repo;
use crate::runner::{check_path, Cmd};

#[derive(Default, Clone)]
struct CommitInfo {
    author: String,
    author_email: String,
    time: Timestamp,
    summary: String,
}

/// Parses `--incremental` output into line ranges, sorted from the first line.
pub fn parse_blame(text: &str) -> Vec<BlameRange> {
    let mut commits: HashMap<String, CommitInfo> = HashMap::new();
    let mut ranges = Vec::new();
    let mut current: Option<(String, u32, u32)> = None;

    for line in text.lines() {
        if let Some((oid, start, count)) = &current {
            if let Some(rest) = line.strip_prefix("filename ") {
                let _ = rest;
                let info = commits.get(oid).cloned().unwrap_or_default();
                ranges.push(BlameRange {
                    uncommitted: oid.chars().all(|c| c == '0'),
                    oid: oid.clone(),
                    start_line: *start,
                    line_count: *count,
                    author: info.author,
                    author_email: info.author_email,
                    time: info.time,
                    summary: info.summary,
                });
                current = None;
                continue;
            }
            let info = commits.entry(oid.clone()).or_default();
            if let Some(v) = line.strip_prefix("author ") {
                info.author = v.to_string();
            } else if let Some(v) = line.strip_prefix("author-mail ") {
                info.author_email = v.trim_matches(|c| c == '<' || c == '>').to_string();
            } else if let Some(v) = line.strip_prefix("author-time ") {
                info.time = v.parse::<i64>().unwrap_or(0) as Timestamp;
            } else if let Some(v) = line.strip_prefix("summary ") {
                info.summary = v.to_string();
            }
            continue;
        }
        // Header line: <oid> <original line> <result line> <line count>
        let mut parts = line.split(' ');
        if let (Some(oid), Some(_orig), Some(start), Some(count)) = (parts.next(), parts.next(), parts.next(), parts.next()) {
            if oid.len() >= 40 && oid.chars().all(|c| c.is_ascii_hexdigit()) {
                if let (Ok(start), Ok(count)) = (start.parse(), count.parse()) {
                    current = Some((oid.to_string(), start, count));
                }
            }
        }
    }
    ranges.sort_by_key(|r| r.start_line);
    ranges
}

impl Repo {
    /// Blames a file. `contents` is the unsaved buffer content, if any.
    pub async fn blame(&self, path: &str, contents: Option<Vec<u8>>) -> Result<Vec<BlameRange>> {
        let path = check_path(path)?;
        let mut cmd = Cmd::new(["blame", "--incremental"]);
        if let Some(contents) = contents {
            cmd = cmd.args(["--contents", "-"]).stdin(contents);
        }
        let out = self.run(cmd.paths([path])).await?;
        Ok(parse_blame(&out.text()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_incremental_output() {
        let a = "a".repeat(40);
        let zero = "0".repeat(40);
        let text = format!(
            "{a} 1 1 2\nauthor Ana\nauthor-mail <ana@x.id>\nauthor-time 1700000000\nauthor-tz +0700\ncommitter Ana\nsummary First commit\nfilename f.txt\n\
             {zero} 3 3 1\nauthor Not Committed Yet\nauthor-mail <not.committed.yet>\nauthor-time 1700000500\nsummary Version of f.txt from f.txt\nfilename f.txt\n\
             {a} 4 5 3\nfilename f.txt\n"
        );
        let ranges = parse_blame(&text);
        assert_eq!(ranges.len(), 3);
        assert_eq!((ranges[0].start_line, ranges[0].line_count), (1, 2));
        assert_eq!(ranges[0].author, "Ana");
        assert_eq!(ranges[0].author_email, "ana@x.id");
        assert_eq!(ranges[0].summary, "First commit");
        assert!(ranges[1].uncommitted);
        // The second entry for the same commit reuses the metadata already known.
        assert_eq!((ranges[2].start_line, ranges[2].line_count), (5, 3));
        assert_eq!(ranges[2].author, "Ana");
        assert_eq!(ranges[2].time, 1_700_000_000.0);
    }
}
