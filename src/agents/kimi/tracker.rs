//! Kimi Code (2.x, wire protocol 1.5): `$KIMI_CODE_HOME` (default `~/.kimi-code`).
//!
//! Sessions live in `sessions/wd_<basename>_<sha256(cwd)[..12]>/session_<uuid>/` with
//! `state.json` (cwd, createdAt, title) and the append-only event log `agents/main/wire.jsonl`.
//! A session is only created when the first prompt is submitted.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::engine::store::{
    claim_first, clean_text_summary, content_text, env_dir_or_home, json_str, list_dir,
    todo_progress, Claim, ExternalInsight, JsonlTail, LaunchContext, Tracker,
};
use crate::osc7501::{BlockedKind, OscState};

const SCAN_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub struct KimiTracker {
    session: Option<(PathBuf, u64)>,
    claim: Option<Claim>,
    last_scan: Option<Instant>,
    wire: JsonlTail,
    pending: Vec<(String, BlockedKind, Option<String>)>,
    reply: Option<String>,
}

impl Tracker for KimiTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            self.scan(ctx, insight);
        }
        let Some((dir, _)) = self.session.as_ref() else {
            return;
        };
        if let Some(title) = read_json(&dir.join("state.json"))
            .as_ref()
            .and_then(|s| json_str(s, &["title"]))
            .and_then(clean_text_summary)
        {
            insight.title = Some(title);
        }
        for line in self.wire.read_lines() {
            self.apply(&line, insight);
        }
    }
}

impl KimiTracker {
    fn scan(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        let Some(root) = env_dir_or_home("KIMI_CODE_HOME", ".kimi-code").map(|r| r.join("sessions"))
        else {
            return;
        };
        let wanted = ctx
            .arg_value(&["-r", "--resume", "-S", "--session"])
            .filter(|v| !v.starts_with('-'));
        let current_created = self.session.as_ref().map(|(_, c)| *c);
        let mut candidates: Vec<(u64, PathBuf, bool)> = Vec::new();
        for workspace in workspace_dirs(&root, ctx) {
            for dir in list_dir(&workspace) {
                let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if self.session.as_ref().is_some_and(|(p, _)| *p == dir) {
                    continue;
                }
                if wanted
                    .as_deref()
                    .is_some_and(|w| name != w && name != format!("session_{w}"))
                {
                    continue;
                }
                let Some(state) = read_json(&dir.join("state.json")) else {
                    continue;
                };
                let created = state.get("createdAt").and_then(Value::as_u64).unwrap_or(0);
                let updated = state.get("updatedAt").and_then(Value::as_u64).unwrap_or(0);
                if !json_str(&state, &["cwd"]).is_some_and(|c| ctx.matches_cwd(c)) {
                    continue;
                }
                if wanted.is_none() && !ctx.session_time_ok(created, updated) {
                    continue;
                }
                if current_created.is_some_and(|c| created <= c) {
                    continue;
                }
                // Sessions are created by the first prompt, so a new one is ours only if it
                // followed input typed here.
                let fresh_session = wanted.is_none() && (!ctx.resume || current_created.is_some());
                if fresh_session && !ctx.created_by_our_submit(created) {
                    continue;
                }
                candidates.push((created, dir, created >= ctx.min_created_ms()));
            }
        }
        candidates.sort_by_key(|(created, _, _)| *created);
        if let Some(((created, dir, fresh), claim)) =
            claim_first("kimi", candidates, |(_, d, _)| d.to_string_lossy().into_owned())
        {
            *insight = ExternalInsight::default();
            self.pending.clear();
            self.reply = None;
            self.wire = JsonlTail::default();
            let wire = dir.join("agents/main/wire.jsonl");
            if fresh {
                self.wire.open(wire);
            } else {
                self.wire.open_at_end(wire);
            }
            self.claim = Some(claim);
            self.session = Some((dir, created));
        }
    }

    fn apply(&mut self, line: &Value, insight: &mut ExternalInsight) {
        match json_str(line, &["type"]).unwrap_or("") {
            "agent.turn.started" => {
                self.pending.clear();
                self.reply = None;
                insight.begin_turn();
            }
            "interaction.request" => {
                let id = json_str(line, &["id"]).unwrap_or("").to_owned();
                let request = line.get("request").unwrap_or(&Value::Null);
                let (kind, msg) = match json_str(line, &["kind"]) {
                    Some("question") => (
                        BlockedKind::Question,
                        request
                            .get("questions")
                            .and_then(Value::as_array)
                            .and_then(|q| q.first())
                            .and_then(|q| json_str(q, &["question"]))
                            .and_then(clean_text_summary),
                    ),
                    _ => (
                        BlockedKind::Permission,
                        json_str(request, &["action"]).and_then(clean_text_summary),
                    ),
                };
                self.pending.push((id, kind, msg));
                self.sync_blocked(insight);
            }
            "interaction.resolved" => {
                let id = json_str(line, &["id"]).unwrap_or("");
                self.pending.retain(|(p, _, _)| p != id);
                self.sync_blocked(insight);
            }
            "context.append_loop_event" => {
                let Some(event) = line.get("event") else {
                    return;
                };
                if json_str(event, &["type"]) == Some("tool.call") {
                    let caption = json_str(event, &["display", "command"])
                        .map(|c| format!("$ {c}"))
                        .or_else(|| json_str(event, &["name"]).map(str::to_owned));
                    if let Some(caption) = caption.as_deref().and_then(clean_text_summary) {
                        insight.working_msg = Some(caption);
                    }
                }
            }
            "tools.update_store" if json_str(line, &["key"]) == Some("todo") => {
                if let Some(items) = line.get("value").and_then(Value::as_array) {
                    let todos = items.iter().map(|t| {
                        (
                            json_str(t, &["status"]).unwrap_or(""),
                            json_str(t, &["title"]).unwrap_or(""),
                        )
                    });
                    if let Some((pct, active)) = todo_progress(todos) {
                        insight.progress = Some(pct);
                        if active.is_some() {
                            insight.working_msg = active;
                        }
                    }
                }
            }
            "agent.message.appended" => {
                let Some(msg) = line.get("message").and_then(|m| m.get("message")) else {
                    return;
                };
                let no_tools = msg
                    .get("toolCalls")
                    .and_then(Value::as_array)
                    .is_none_or(|calls| calls.is_empty());
                if json_str(msg, &["role"]) == Some("assistant") && no_tools {
                    if let Some(text) = msg.get("content").and_then(|c| content_text(c, &["text"])) {
                        self.reply = clean_text_summary(&text);
                    }
                }
            }
            "turn.ended" => {
                self.pending.clear();
                match json_str(line, &["reason"]).unwrap_or("") {
                    "completed" => insight.finish_turn(self.reply.take()),
                    "cancelled" => insight.abort_turn(),
                    _ => {
                        let err = json_str(line, &["error", "message"]).and_then(clean_text_summary);
                        insight.fail_turn(err);
                    }
                }
            }
            _ => {}
        }
    }

    fn sync_blocked(&self, insight: &mut ExternalInsight) {
        match self.pending.last() {
            Some((_, kind, msg)) => {
                insight.state = Some(OscState::Blocked);
                insight.blocked_kind = Some(*kind);
                insight.blocked_msg = msg.clone();
            }
            None if insight.state == Some(OscState::Blocked) => {
                insight.state = Some(OscState::Working);
                insight.blocked_kind = None;
                insight.blocked_msg = None;
            }
            None => {}
        }
    }
}

/// `wd_<basename>_<sha256(cwd)[..12]>` for each spelling of the cwd (matched on the hash suffix,
/// since the basename part is sanitised by kimi).
fn workspace_dirs(root: &Path, ctx: &LaunchContext) -> Vec<PathBuf> {
    let suffixes: Vec<String> = ctx
        .cwd_variants
        .iter()
        .map(|cwd| {
            let digest = Sha256::digest(cwd.as_bytes());
            let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
            format!("_{hex}")
        })
        .collect();
    list_dir(root)
        .into_iter()
        .filter(|dir| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("wd_") && suffixes.iter().any(|s| n.ends_with(s.as_str())))
        })
        .collect()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
