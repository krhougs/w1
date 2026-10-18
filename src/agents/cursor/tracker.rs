//! Cursor Agent CLI.
//!
//! Sessions live in `<config>/chats/<md5(cwd)>/<id>/meta.json` (config: `$CURSOR_CONFIG_DIR`,
//! `$XDG_CONFIG_HOME/cursor` or `~/.cursor`); transcripts in
//! `<data>/projects/<slug(cwd)>/agent-transcripts/<id>/<id>.jsonl` (data: `$CURSOR_DATA_DIR` or
//! `~/.cursor`). The transcript is rewritten from scratch at turn boundaries, so it is re-parsed
//! whole whenever it changes.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::engine::store::{
    claim_first, clean_text_summary, file_mtime_ms, home_dir, list_dir, todo_progress, Claim,
    ExternalInsight, LaunchContext, Tracker,
};

const SCAN_INTERVAL: Duration = Duration::from_millis(1500);
const TRANSCRIPT_MAX: u64 = 4 * 1024 * 1024;

#[derive(Default)]
pub struct CursorTracker {
    session: Option<Session>,
    claim: Option<Claim>,
    last_scan: Option<Instant>,
    transcript_mtime: Option<u64>,
    /// A turn was seen in progress (or the session is new), so a `turn_ended` line is ours.
    turn_observed: bool,
}

struct Session {
    id: String,
    meta: PathBuf,
    cwd: String,
    fresh: bool,
}

impl Tracker for CursorTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.session.is_none() && self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            self.locate(ctx);
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if let Some(meta) = read_json(&session.meta) {
            if let Some(title) = meta
                .get("title")
                .and_then(Value::as_str)
                .filter(|t| *t != "New Agent")
                .and_then(clean_text_summary)
            {
                insight.title = Some(title);
            }
        }
        let Some(data) = data_root() else {
            return;
        };
        let transcript = data
            .join("projects")
            .join(slug(&session.cwd))
            .join("agent-transcripts")
            .join(&session.id)
            .join(format!("{}.jsonl", session.id));
        let mtime = file_mtime_ms(&transcript);
        if mtime.is_none() || mtime == self.transcript_mtime {
            return;
        }
        self.transcript_mtime = mtime;
        if let Some(lines) = read_lines_tail(&transcript, TRANSCRIPT_MAX) {
            let accept_end = self.turn_observed || session.fresh;
            self.turn_observed = apply_transcript(&lines, accept_end, insight);
        }
    }
}

impl CursorTracker {
    fn locate(&mut self, ctx: &LaunchContext) {
        let Some(chats) = config_root().map(|c| c.join("chats")) else {
            return;
        };
        let wanted = ctx.arg_value(&["--resume"]).filter(|v| !v.starts_with('-'));
        let mut candidates: Vec<(u64, Session)> = Vec::new();
        for workspace in list_dir(&chats) {
            if wanted.is_none()
                && file_mtime_ms(&workspace).is_none_or(|m| m < ctx.min_created_ms())
            {
                continue;
            }
            for dir in list_dir(&workspace) {
                let Some(id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                    continue;
                };
                if wanted.as_deref().is_some_and(|w| w != id) {
                    continue;
                }
                let meta_path = dir.join("meta.json");
                let Some(meta) = read_json(&meta_path) else {
                    continue;
                };
                if meta.get("isSubagent").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let Some(cwd) = meta.get("cwd").and_then(Value::as_str) else {
                    continue;
                };
                let created = meta.get("createdAtMs").and_then(Value::as_u64).unwrap_or(0);
                let updated = meta.get("updatedAtMs").and_then(Value::as_u64).unwrap_or(0);
                if !ctx.matches_cwd(cwd) {
                    continue;
                }
                if wanted.is_none() && !ctx.session_time_ok(created, updated) {
                    continue;
                }
                candidates.push((
                    created,
                    Session {
                        id,
                        meta: meta_path,
                        cwd: cwd.to_owned(),
                        fresh: created >= ctx.min_created_ms(),
                    },
                ));
            }
        }
        candidates.sort_by_key(|(created, _)| *created);
        if let Some(((_, session), claim)) =
            claim_first("cursor", candidates, |(_, s)| s.id.clone())
        {
            self.claim = Some(claim);
            self.session = Some(session);
        }
    }
}

fn config_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CURSOR_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        let dir = PathBuf::from(xdg).join("cursor");
        if dir.is_dir() {
            return Some(dir);
        }
    }
    home_dir().map(|h| h.join(".cursor"))
}

fn data_root() -> Option<PathBuf> {
    std::env::var_os("CURSOR_DATA_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".cursor")))
}

/// `[^a-zA-Z0-9]` → `-`, runs collapsed, trimmed.
fn slug(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn read_lines_tail(path: &Path, max: u64) -> Option<Vec<Value>> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let mut lines = buf.split(|b| *b == b'\n');
    if start > 0 {
        lines.next();
    }
    Some(
        lines
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_slice(l).ok())
            .collect(),
    )
}

/// Returns whether a turn is still in progress.
fn apply_transcript(lines: &[Value], accept_end: bool, insight: &mut ExternalInsight) -> bool {
    let mut last_text = None;
    let mut progress = None;
    for line in lines {
        let blocks = line
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array);
        if line.get("role").and_then(Value::as_str) == Some("assistant") {
            for block in blocks.into_iter().flatten() {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            last_text = strip_thinking(text);
                        }
                    }
                    Some("tool_use")
                        if block.get("name").and_then(Value::as_str) == Some("TodoWrite") =>
                    {
                        if let Some(todos) = block
                            .get("input")
                            .and_then(|i| i.get("todos"))
                            .and_then(Value::as_array)
                        {
                            progress = todo_progress(todos.iter().map(|t| {
                                (
                                    t.get("status").and_then(Value::as_str).unwrap_or(""),
                                    t.get("content").and_then(Value::as_str).unwrap_or(""),
                                )
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some((pct, active)) = progress {
        insight.progress = Some(pct);
        if active.is_some() {
            insight.working_msg = active;
        }
    }
    let ended = lines
        .last()
        .filter(|l| l.get("type").and_then(Value::as_str) == Some("turn_ended"));
    let Some(status) = ended.and_then(|l| l.get("status")).and_then(Value::as_str) else {
        // A turn is being written; any earlier `turn_ended` no longer describes the present.
        insight.state = None;
        if last_text.is_some() {
            insight.working_msg = last_text;
        }
        return true;
    };
    if accept_end {
        match status {
            "success" => insight.finish_turn(last_text),
            "error" => {
                let err = ended
                    .and_then(|l| l.get("error"))
                    .and_then(Value::as_str)
                    .and_then(clean_text_summary);
                insight.fail_turn(err);
            }
            "aborted" => insight.abort_turn(),
            _ => {}
        }
    }
    false
}

/// Assistant text interleaves `**Thinking title**\n\n…` paragraphs before the actual reply.
fn strip_thinking(text: &str) -> Option<String> {
    let paragraphs: Vec<&str> = text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).collect();
    let reply = paragraphs
        .iter()
        .rev()
        .find(|p| !(p.starts_with("**") && p.ends_with("**")))
        .or(paragraphs.last())?;
    clean_text_summary(reply)
}
