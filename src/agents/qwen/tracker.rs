//! Qwen Code: `$QWEN_HOME` (default `~/.qwen`).
//!
//! A running TUI registers itself in `sessions/<pid>.json` (`sessionId`, `cwd`), replaced on
//! `/clear` and removed on exit. The transcript is
//! `<chats root>/projects/<cwd with non-alphanumerics as '-'>/chats/<sessionId>.jsonl`, created
//! with the first prompt; resumed sessions keep appending to it. Turn boundaries come from the
//! `request_lifecycle` telemetry records of the main agent (memory extractors write their own,
//! tagged with `subagentId`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::engine::store::{
    clean_text_summary, env_dir_or_home, home_dir, json_str, list_dir, parse_iso_ms,
    todo_progress, ExternalInsight, JsonlTail, LaunchContext, Tracker,
};
use crate::osc7501::{BlockedKind, OscState};

const SCAN_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub struct QwenTracker {
    session_id: Option<String>,
    last_scan: Option<Instant>,
    tail: JsonlTail,
    reply: Option<String>,
    /// Tool calls of the current turn without a result yet, by call id.
    pending: BTreeMap<String, Option<String>>,
    api_error: Option<String>,
}

impl Tracker for QwenTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            self.scan(ctx, insight);
        }
        for line in self.tail.read_lines() {
            self.apply(&line, ctx, insight);
        }
    }
}

impl QwenTracker {
    fn scan(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        let Some(global) = env_dir_or_home("QWEN_HOME", ".qwen") else {
            return;
        };
        let Some((session_id, cwd)) = registered_session(&global, ctx) else {
            return;
        };
        if self.session_id.as_deref() == Some(session_id.as_str()) {
            return;
        }
        *insight = ExternalInsight::default();
        self.reply = None;
        self.pending.clear();
        self.api_error = None;
        let path = chats_root(&global, &cwd)
            .join("projects")
            .join(project_slug(&cwd))
            .join("chats")
            .join(format!("{session_id}.jsonl"));
        self.tail = JsonlTail::default();
        self.tail.open(path);
        self.session_id = Some(session_id);
    }

    fn apply(&mut self, line: &Value, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if json_str(line, &["subtype"]) == Some("custom_title") {
            if let Some(title) =
                json_str(line, &["systemPayload", "customTitle"]).and_then(clean_text_summary)
            {
                insight.title = Some(title);
            }
            return;
        }
        // Earlier turns of a resumed session only contribute the title.
        let live = json_str(line, &["timestamp"])
            .and_then(parse_iso_ms)
            .is_none_or(|ts| ts >= ctx.min_created_ms());
        if !live || is_background(line) {
            return;
        }
        match json_str(line, &["type"]).unwrap_or("") {
            "user" if json_str(line, &["provenance"]) == Some("real_user") => {
                self.reply = None;
                self.pending.clear();
                self.api_error = None;
                insight.begin_turn();
            }
            "assistant" => self.apply_assistant(line, insight),
            "tool_result" => {
                let Some(result) = line.get("toolCallResult") else {
                    return;
                };
                if let Some(id) = json_str(result, &["callId"]) {
                    self.pending.remove(id);
                }
                if insight.state == Some(OscState::Blocked) && !self.question_pending() {
                    insight.state = Some(OscState::Working);
                    insight.blocked_kind = None;
                    insight.blocked_msg = None;
                }
                // A declined tool call ends the turn.
                if json_str(result, &["status"]) == Some("cancelled") {
                    self.pending.clear();
                    insight.abort_turn();
                }
            }
            "system" if json_str(line, &["subtype"]) == Some("ui_telemetry") => {
                let Some(event) = line.get("systemPayload").and_then(|p| p.get("uiEvent")) else {
                    return;
                };
                match json_str(event, &["event.name"]).unwrap_or("") {
                    "qwen-code.api_error" => {
                        self.api_error = json_str(event, &["error_message"]).and_then(clean_text_summary);
                    }
                    "request_lifecycle" if json_str(event, &["phase"]) == Some("ended") => {
                        // A successful request is followed by its assistant record, which tells
                        // whether the turn goes on with tool calls.
                        match json_str(event, &["outcome"]).unwrap_or("") {
                            "cancelled" | "interrupted" => {
                                self.pending.clear();
                                insight.abort_turn();
                            }
                            "error" => {
                                self.pending.clear();
                                insight.fail_turn(self.api_error.take());
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn apply_assistant(&mut self, line: &Value, insight: &mut ExternalInsight) {
        let parts = line
            .get("message")
            .and_then(|m| m.get("parts"))
            .and_then(Value::as_array);
        let mut text = String::new();
        let mut calls = 0usize;
        for part in parts.into_iter().flatten() {
            if let Some(call) = part.get("functionCall") {
                calls += 1;
                let id = json_str(call, &["id"]).unwrap_or("").to_owned();
                let name = json_str(call, &["name"]).unwrap_or("");
                let args = call.get("args").unwrap_or(&Value::Null);
                match name {
                    "ask_user_question" => {
                        let question = args
                            .get("questions")
                            .and_then(Value::as_array)
                            .and_then(|q| q.first())
                            .and_then(|q| json_str(q, &["question"]))
                            .and_then(clean_text_summary);
                        self.pending.insert(id, Some(question.clone().unwrap_or_default()));
                        insight.state = Some(OscState::Blocked);
                        insight.blocked_kind = Some(BlockedKind::Question);
                        insight.blocked_msg = question;
                    }
                    "todo_write" => {
                        self.pending.insert(id, None);
                        if let Some(todos) = args.get("todos").and_then(Value::as_array) {
                            if let Some((pct, active)) = todo_progress(todos.iter().map(|t| {
                                (
                                    json_str(t, &["status"]).unwrap_or(""),
                                    json_str(t, &["content"]).unwrap_or(""),
                                )
                            })) {
                                insight.progress = Some(pct);
                                if active.is_some() {
                                    insight.working_msg = active;
                                }
                            }
                        }
                    }
                    _ => {
                        self.pending.insert(id, None);
                        let caption = json_str(args, &["command"])
                            .map(|c| format!("$ {c}"))
                            .or_else(|| json_str(args, &["description"]).map(str::to_owned))
                            .or_else(|| (!name.is_empty()).then(|| name.to_owned()));
                        if let Some(caption) = caption.as_deref().and_then(clean_text_summary) {
                            insight.working_msg = Some(caption);
                        }
                    }
                }
            } else if part.get("thought").and_then(Value::as_bool) != Some(true) {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                }
            }
        }
        if let Some(reply) = clean_text_summary(&text) {
            self.reply = Some(reply);
        }
        if calls == 0 && self.pending.is_empty() {
            insight.finish_turn(self.reply.take());
        }
    }

    fn question_pending(&self) -> bool {
        self.pending.values().any(Option::is_some)
    }
}

fn is_background(line: &Value) -> bool {
    let event = line.get("systemPayload").and_then(|p| p.get("uiEvent"));
    if event.and_then(|e| e.get("subagentId")).is_some_and(|v| !v.is_null()) {
        return true;
    }
    [
        json_str(line, &["promptId"]),
        event.and_then(|e| json_str(e, &["promptId"])),
        event.and_then(|e| json_str(e, &["prompt_id"])),
    ]
    .into_iter()
    .flatten()
    .any(|id| id.contains("#managed-"))
}

/// The session registered by a qwen process descending from the child `w1` spawned.
fn registered_session(global: &Path, ctx: &LaunchContext) -> Option<(String, String)> {
    let child = ctx.child_pid?;
    list_dir(&global.join("sessions")).into_iter().find_map(|path| {
        let pid: i32 = path.file_stem()?.to_str()?.parse().ok()?;
        if pid != child && !crate::procs::is_descendant_of(pid, child) {
            return None;
        }
        let entry: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        let cwd = json_str(&entry, &["cwd"])?;
        if !ctx.matches_cwd(cwd) {
            return None;
        }
        Some((json_str(&entry, &["sessionId"])?.to_owned(), cwd.to_owned()))
    })
}

/// `QWEN_RUNTIME_DIR`, then `advanced.runtimeOutputDir` (workspace, then user settings), then
/// the global directory.
fn chats_root(global: &Path, cwd: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os("QWEN_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let configured = [Path::new(cwd).join(".qwen/settings.json"), global.join("settings.json")]
        .iter()
        .find_map(|settings| {
            let raw = std::fs::read(settings).ok()?;
            let value: Value = serde_json::from_slice(&raw).ok()?;
            json_str(&value, &["advanced", "runtimeOutputDir"])
                .filter(|d| !d.trim().is_empty())
                .map(str::to_owned)
        });
    match configured {
        Some(dir) => match dir.strip_prefix("~/") {
            Some(rest) => home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(&dir)),
            None => PathBuf::from(dir),
        },
        None => global.to_path_buf(),
    }
}

fn project_slug(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
