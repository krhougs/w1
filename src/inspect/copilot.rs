//! GitHub Copilot CLI: `$COPILOT_HOME` (default `~/.copilot`).
//!
//! Each session is `session-state/<id>/` with `workspace.yaml` (`name`), an append-only
//! `events.jsonl` (created with the first prompt) and `inuse.<pid>.lock` while a process holds
//! it. A resume first opens a placeholder session and then moves to the old one, so the lock is
//! re-scanned continuously.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{
    clean_text_summary, env_dir_or_home, json_str, list_dir, parse_iso_ms, row_str, sqlite_rows,
    todo_progress, ExternalInsight, JsonlTail, LaunchContext, Tracker,
};
use crate::osc7501::{BlockedKind, OscState};

const SCAN_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub struct CopilotTracker {
    dir: Option<PathBuf>,
    last_scan: Option<Instant>,
    events: JsonlTail,
    reply: Option<String>,
    final_reply: bool,
    question: Option<String>,
    permissions: usize,
}

impl Tracker for CopilotTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            self.scan(ctx, insight);
            if let Some(dir) = self.dir.as_deref() {
                if let Some(name) = workspace_name(dir) {
                    insight.title = Some(name);
                }
                if let Some(pct) = todos(dir) {
                    insight.progress = Some(pct);
                }
            }
        }
        for line in self.events.read_lines() {
            self.apply(&line, ctx, insight);
        }
    }
}

impl CopilotTracker {
    fn scan(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        let Some(child) = ctx.child_pid else {
            return;
        };
        let Some(root) = env_dir_or_home("COPILOT_HOME", ".copilot").map(|d| d.join("session-state"))
        else {
            return;
        };
        let held = list_dir(&root).into_iter().find(|dir| {
            list_dir(dir).iter().any(|f| {
                f.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("inuse."))
                    .and_then(|n| n.strip_suffix(".lock"))
                    .and_then(|pid| pid.parse::<i32>().ok())
                    .is_some_and(|pid| pid == child || crate::procs::is_descendant_of(pid, child))
            })
        });
        let Some(dir) = held else {
            return;
        };
        if self.dir.as_deref() == Some(dir.as_path()) {
            return;
        }
        *insight = ExternalInsight::default();
        self.reply = None;
        self.final_reply = false;
        self.question = None;
        self.permissions = 0;
        self.events = JsonlTail::default();
        self.events.open(dir.join("events.jsonl"));
        self.dir = Some(dir);
    }

    fn apply(&mut self, line: &Value, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        // Earlier turns of a resumed session are history.
        let live = json_str(line, &["timestamp"])
            .and_then(parse_iso_ms)
            .is_none_or(|ts| ts >= ctx.min_created_ms());
        if !live {
            return;
        }
        let data = line.get("data").unwrap_or(&Value::Null);
        match json_str(line, &["type"]).unwrap_or("") {
            "user.message" => {
                self.reply = None;
                self.final_reply = false;
                self.question = None;
                self.permissions = 0;
                insight.begin_turn();
            }
            "assistant.message" => {
                let requests = data
                    .get("toolRequests")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                self.final_reply = requests == 0;
                if let Some(text) = json_str(data, &["content"]).and_then(clean_text_summary) {
                    if requests == 0 {
                        self.reply = Some(text);
                    } else {
                        insight.working_msg = Some(text);
                    }
                }
            }
            "tool.execution_start" => {
                let args = data.get("arguments").unwrap_or(&Value::Null);
                if json_str(data, &["toolName"]) == Some("ask_user") {
                    self.question = json_str(args, &["message"])
                        .or_else(|| json_str(args, &["question"]))
                        .and_then(clean_text_summary)
                        .or(Some(String::new()));
                    self.sync_blocked(insight);
                } else if let Some(caption) = json_str(args, &["command"])
                    .map(|c| format!("$ {c}"))
                    .or_else(|| json_str(data, &["toolTitle"]).map(str::to_owned))
                    .as_deref()
                    .and_then(clean_text_summary)
                {
                    insight.working_msg = Some(caption);
                }
            }
            "tool.execution_complete" => {
                if self.question.take().is_some() {
                    self.sync_blocked(insight);
                }
            }
            "permission.requested" => {
                self.permissions += 1;
                let request = data.get("permissionRequest").unwrap_or(&Value::Null);
                insight.blocked_msg = json_str(request, &["fullCommandText"])
                    .map(|c| format!("$ {c}"))
                    .or_else(|| json_str(request, &["intention"]).map(str::to_owned))
                    .as_deref()
                    .and_then(clean_text_summary);
                self.sync_blocked(insight);
            }
            "permission.completed" => {
                self.permissions = self.permissions.saturating_sub(1);
                self.sync_blocked(insight);
            }
            "assistant.turn_end" => {
                if self.final_reply {
                    insight.finish_turn(self.reply.take());
                }
            }
            "session.error" => {
                insight.fail_turn(json_str(data, &["message"]).and_then(clean_text_summary));
            }
            "abort" => insight.abort_turn(),
            _ => {}
        }
    }

    fn sync_blocked(&self, insight: &mut ExternalInsight) {
        if let Some(question) = self.question.as_ref() {
            insight.state = Some(OscState::Blocked);
            insight.blocked_kind = Some(BlockedKind::Question);
            insight.blocked_msg = (!question.is_empty()).then(|| question.clone());
        } else if self.permissions > 0 {
            insight.state = Some(OscState::Blocked);
            insight.blocked_kind = Some(BlockedKind::Permission);
        } else if insight.state == Some(OscState::Blocked) {
            insight.state = Some(OscState::Working);
            insight.blocked_kind = None;
            insight.blocked_msg = None;
        }
    }
}

/// `name:` from `workspace.yaml` (a flat map of scalars).
fn workspace_name(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("workspace.yaml")).ok()?;
    let value = raw.lines().find_map(|l| l.strip_prefix("name:"))?.trim();
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value);
    clean_text_summary(unquoted)
}

fn todos(dir: &Path) -> Option<u8> {
    let db = dir.join("session.db");
    if !db.is_file() {
        return None;
    }
    let rows = sqlite_rows(&db, "SELECT status FROM todos;")?;
    todo_progress(rows.iter().map(|r| (row_str(r, "status").unwrap_or(""), ""))).map(|(pct, _)| pct)
}
