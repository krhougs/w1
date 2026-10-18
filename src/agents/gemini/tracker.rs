//! Gemini CLI (0.4x): `$GEMINI_CLI_HOME/.gemini` (default `~/.gemini`).
//!
//! Each project (realpath of the cwd) maps to `tmp/<slug>/` through `projects.json` or a
//! `.project_root` file; sessions are `tmp/<slug>/chats/session-*.jsonl` whose first line carries
//! `sessionId`, `projectHash` (sha256 of the root) and `startTime`. The store has no turn-end
//! record, so completion comes from the screen; the store supplies title, reply and errors.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::engine::store::{
    claim_first, clean_text_summary, content_text, file_mtime_ms, home_dir, json_str, list_dir,
    parse_iso_ms, read_first_line, todo_progress, Claim, ExternalInsight, JsonlTail, LaunchContext,
    Tracker,
};

const SCAN_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub struct GeminiTracker {
    session: Option<(String, u64)>,
    claim: Option<Claim>,
    last_scan: Option<Instant>,
    tail: JsonlTail,
    topic_title: Option<String>,
    first_prompt: Option<String>,
}

struct Candidate {
    path: PathBuf,
    session_id: String,
    start: u64,
    fresh: bool,
}

impl Tracker for GeminiTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            self.scan(ctx, insight);
        }
        if self.session.is_none() {
            return;
        }
        for line in self.tail.read_lines() {
            self.apply(&line, insight);
        }
        insight.title = self.topic_title.clone().or_else(|| self.first_prompt.clone());
    }
}

impl GeminiTracker {
    fn scan(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        let Some(home) = gemini_home() else {
            return;
        };
        let current_start = self.session.as_ref().map(|(_, s)| *s);
        let mut candidates = Vec::new();
        for (root, chats) in project_chat_dirs(&home, ctx) {
            let hash = hex_sha256(&root);
            for path in list_dir(&chats) {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with("session-") || !name.ends_with(".jsonl") {
                    continue;
                }
                let Some(header) = read_first_line(&path, 16 * 1024)
                    .and_then(|l| serde_json::from_str::<Value>(&l).ok())
                else {
                    continue;
                };
                if json_str(&header, &["kind"]).is_some_and(|k| k != "main")
                    || json_str(&header, &["projectHash"]).is_some_and(|h| h != hash)
                {
                    continue;
                }
                let Some(session_id) = json_str(&header, &["sessionId"]).map(str::to_owned) else {
                    continue;
                };
                let start = json_str(&header, &["startTime"]).and_then(parse_iso_ms).unwrap_or(0);
                let updated = file_mtime_ms(&path).unwrap_or(0);
                let fresh = start >= ctx.min_created_ms();
                // A resume writes a two-line stub and keeps appending to the original file.
                if fresh && ctx.resume {
                    continue;
                }
                if !ctx.session_time_ok(start, updated)
                    || current_start.is_some_and(|c| start <= c || !ctx.created_by_our_submit(start))
                {
                    continue;
                }
                candidates.push(Candidate {
                    path,
                    session_id,
                    start,
                    fresh,
                });
            }
        }
        candidates.sort_by_key(|c| c.start);
        if let Some((found, claim)) = claim_first("gemini", candidates, |c| c.session_id.clone()) {
            *insight = ExternalInsight::default();
            self.topic_title = None;
            self.first_prompt = None;
            self.tail = JsonlTail::default();
            if found.fresh {
                self.tail.open(found.path);
            } else {
                self.tail.open_at_end(found.path);
            }
            self.claim = Some(claim);
            self.session = Some((found.session_id, found.start));
        }
    }

    fn apply(&mut self, line: &Value, insight: &mut ExternalInsight) {
        if let Some(set) = line.get("$set") {
            if let Some(summary) = json_str(set, &["summary"]).and_then(clean_text_summary) {
                self.topic_title.get_or_insert(summary);
            }
            return;
        }
        match json_str(line, &["type"]).unwrap_or("") {
            "user" => {
                let Some(text) = line.get("content").and_then(|c| content_text(c, &[])) else {
                    return;
                };
                if text.trim_start().starts_with("<session_context>") {
                    return;
                }
                if self.first_prompt.is_none() {
                    self.first_prompt = clean_text_summary(&text);
                }
                insight.begin_turn();
            }
            "gemini" => {
                if let Some(subject) = line
                    .get("thoughts")
                    .and_then(Value::as_array)
                    .and_then(|t| t.last())
                    .and_then(|t| json_str(t, &["subject"]))
                    .and_then(clean_text_summary)
                {
                    insight.working_msg = Some(subject);
                }
                for call in line.get("toolCalls").and_then(Value::as_array).into_iter().flatten() {
                    match json_str(call, &["name"]).unwrap_or("") {
                        "update_topic" => {
                            if let Some(t) = json_str(call, &["args", "title"]).and_then(clean_text_summary) {
                                self.topic_title = Some(t);
                            }
                        }
                        "write_todos" => {
                            if let Some(todos) = call.get("args").and_then(|a| a.get("todos")).and_then(Value::as_array) {
                                if let Some((pct, active)) = todo_progress(todos.iter().map(|t| {
                                    (
                                        json_str(t, &["status"]).unwrap_or(""),
                                        json_str(t, &["description"]).unwrap_or(""),
                                    )
                                })) {
                                    insight.progress = Some(pct);
                                    if active.is_some() {
                                        insight.working_msg = active;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let has_calls = line
                    .get("toolCalls")
                    .and_then(Value::as_array)
                    .is_some_and(|calls| !calls.is_empty());
                if let Some(reply) = line
                    .get("content")
                    .and_then(|c| content_text(c, &[]))
                    .as_deref()
                    .and_then(clean_text_summary)
                {
                    insight.done_msg = Some(reply);
                    // The final answer; its thoughts describe a finished turn.
                    if !has_calls {
                        insight.working_msg = None;
                    }
                }
            }
            "info" => {
                if json_str(line, &["content"]).is_some_and(|c| c.contains("Request cancelled")) {
                    insight.abort_turn();
                }
            }
            "error" => {
                let msg = json_str(line, &["content"]).and_then(clean_text_summary);
                insight.fail_turn(msg);
            }
            _ => {}
        }
    }
}

fn gemini_home() -> Option<PathBuf> {
    std::env::var_os("GEMINI_CLI_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(home_dir)
        .map(|h| h.join(".gemini"))
}

fn hex_sha256(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// `(project root, chats dir)` for every recorded spelling of the cwd.
fn project_chat_dirs(home: &Path, ctx: &LaunchContext) -> Vec<(String, PathBuf)> {
    let tmp = home.join("tmp");
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let projects = std::fs::read(home.join("projects.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
    for root in &ctx.cwd_variants {
        if let Some(slug) = projects
            .as_ref()
            .and_then(|p| p.get("projects"))
            .and_then(|p| p.get(root.as_str()))
            .and_then(Value::as_str)
        {
            out.push((root.clone(), tmp.join(slug).join("chats")));
        }
    }
    if out.is_empty() {
        for dir in list_dir(&tmp) {
            let Ok(recorded) = std::fs::read_to_string(dir.join(".project_root")) else {
                continue;
            };
            let recorded = recorded.trim();
            if ctx.cwd_variants.iter().any(|v| v == recorded) {
                out.push((recorded.to_owned(), dir.join("chats")));
            }
        }
    }
    out
}
