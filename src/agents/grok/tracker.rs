//! Grok Build: `$GROK_HOME` (default `~/.grok`).
//!
//! `active_sessions.json` maps running grok pids to session ids; the session directory
//! `sessions/<encoded cwd>/<id>/` holds `summary.json` (title), `events.jsonl` (turn state machine)
//! and `updates.jsonl` (ACP updates carrying the reply text).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::engine::store::{
    clean_text_summary, env_dir_or_home, json_str, list_dir, parse_iso_ms, ExternalInsight,
    JsonlTail, LaunchContext, Tracker,
};
use crate::osc7501::{BlockedKind, OscState};

const SCAN_INTERVAL: Duration = Duration::from_millis(1000);
const MAX_REPLY: usize = 4096;

#[derive(Default)]
pub struct GrokTracker {
    session: Option<(String, PathBuf)>,
    last_scan: Option<Instant>,
    events: JsonlTail,
    updates: JsonlTail,
    reply: String,
    turn_error: Option<String>,
}

impl Tracker for GrokTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        let Some(home) = env_dir_or_home("GROK_HOME", ".grok") else {
            return;
        };
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            if let Some((id, dir)) = active_session(&home, ctx) {
                if self.session.as_ref().is_none_or(|(cur, _)| *cur != id) {
                    *insight = ExternalInsight::default();
                    self.reply.clear();
                    self.events = JsonlTail::default();
                    self.updates = JsonlTail::default();
                    self.events.open(dir.join("events.jsonl"));
                    self.updates.open(dir.join("updates.jsonl"));
                    self.session = Some((id, dir));
                }
            }
        }
        let Some((_, dir)) = self.session.as_ref() else {
            return;
        };
        if let Ok(raw) = std::fs::read(dir.join("summary.json")) {
            if let Ok(summary) = serde_json::from_slice::<Value>(&raw) {
                if let Some(title) = json_str(&summary, &["generated_title"]).and_then(clean_text_summary) {
                    insight.title = Some(title);
                }
                if let Some(recap) = json_str(&summary, &["last_turn_summary"]).and_then(clean_text_summary) {
                    if insight.state == Some(OscState::Done) {
                        insight.done_msg = Some(recap);
                    }
                }
            }
        }
        for update in self.updates.read_lines() {
            self.apply_update(&update);
        }
        for event in self.events.read_lines() {
            self.apply_event(&event, ctx, insight);
        }
    }
}

impl GrokTracker {
    fn apply_update(&mut self, line: &Value) {
        let Some(update) = line.get("params").and_then(|p| p.get("update")) else {
            return;
        };
        match json_str(update, &["sessionUpdate"]).unwrap_or("") {
            "user_message_chunk" => {
                self.reply.clear();
                self.turn_error = None;
            }
            "agent_message_chunk" => {
                if let Some(text) = json_str(update, &["content", "text"]) {
                    if self.reply.len() < MAX_REPLY {
                        self.reply.push_str(text);
                    }
                }
            }
            "turn_completed" => {
                if json_str(update, &["stop_reason"]) == Some("error") {
                    self.turn_error = json_str(update, &["agent_result"]).and_then(clean_text_summary);
                }
            }
            "retry_state" => {
                if json_str(update, &["type"]) == Some("failed") {
                    self.turn_error = json_str(update, &["message"]).and_then(clean_text_summary);
                }
            }
            _ => {}
        }
    }

    fn apply_event(&mut self, event: &Value, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        let live = json_str(event, &["ts"])
            .and_then(parse_iso_ms)
            .is_none_or(|ts| ts >= ctx.min_created_ms());
        if !live {
            return;
        }
        match json_str(event, &["type"]).unwrap_or("") {
            "turn_started" => insight.begin_turn(),
            "phase_changed" => match json_str(event, &["phase"]).unwrap_or("") {
                "permission_prompt" => block(insight),
                "" => {}
                _ => resume_working(insight),
            },
            "permission_requested" => block(insight),
            "permission_resolved" => resume_working(insight),
            "turn_ended" => {
                let outcome = json_str(event, &["outcome"]).unwrap_or("");
                let cancelled = event.get("cancellation_category").is_some_and(|c| !c.is_null())
                    || matches!(outcome, "cancelled" | "canceled" | "interrupted" | "aborted");
                if outcome == "error" {
                    insight.fail_turn(self.turn_error.clone());
                } else if cancelled {
                    insight.abort_turn();
                } else {
                    insight.finish_turn(clean_text_summary(&self.reply));
                }
            }
            _ => {}
        }
    }
}

fn block(insight: &mut ExternalInsight) {
    insight.state = Some(OscState::Blocked);
    insight.blocked_kind = Some(BlockedKind::Permission);
}

fn resume_working(insight: &mut ExternalInsight) {
    if matches!(insight.state, Some(OscState::Blocked | OscState::Working)) {
        insight.state = Some(OscState::Working);
        insight.blocked_kind = None;
        insight.blocked_msg = None;
    }
}

/// The session registered by a grok process descending from the child `w1` spawned.
fn active_session(home: &Path, ctx: &LaunchContext) -> Option<(String, PathBuf)> {
    let child = ctx.child_pid?;
    let raw = std::fs::read(home.join("active_sessions.json")).ok()?;
    let entries: Vec<Value> = serde_json::from_slice(&raw).ok()?;
    let entry = entries.iter().rev().find(|e| {
        e.get("pid")
            .and_then(Value::as_i64)
            .and_then(|p| i32::try_from(p).ok())
            .is_some_and(|p| crate::procs::is_descendant_of(p, child))
    })?;
    let id = json_str(entry, &["session_id"])?.to_owned();
    let dir = list_dir(&home.join("sessions"))
        .into_iter()
        .map(|group| group.join(&id))
        .find(|d| d.is_dir())?;
    Some((id, dir))
}
