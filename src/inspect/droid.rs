//! Factory Droid: `${FACTORY_HOME_OVERRIDE:-$HOME}/.factory/sessions/<slug>/<id>.jsonl`, where
//! the slug is the physical cwd with `/` replaced by `-`.
//!
//! The TUI creates the file at start-up. Its first line (`session_start`, carrying `cwd` and
//! `title`) is rewritten in place when the title changes, which shifts every later offset, so
//! the file is re-parsed whole whenever it changes. Turns end with `agent_turn_outcome`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{
    claim_first, clean_text_summary, file_created_ms, file_mtime_ms, home_dir, json_str,
    list_dir, read_first_line, todo_progress, Claim, ExternalInsight, LaunchContext, Tracker,
};
use crate::osc7501::{BlockedKind, OscState};

const SCAN_INTERVAL: Duration = Duration::from_millis(1000);
const MAX_PARSE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Default)]
pub struct DroidTracker {
    session: Option<Session>,
    claim: Option<Claim>,
    last_scan: Option<Instant>,
    seen: Option<(u64, u64)>,
}

struct Session {
    path: PathBuf,
    created: u64,
    /// Lines that predate this launch (a resumed session); only the title is read from them.
    history_lines: Option<usize>,
}

impl Tracker for DroidTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            self.scan(ctx);
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Ok(meta) = std::fs::metadata(&session.path) else {
            return;
        };
        let stamp = (meta.len(), file_mtime_ms(&session.path).unwrap_or(0));
        if self.seen == Some(stamp) || meta.len() > MAX_PARSE_BYTES {
            return;
        }
        self.seen = Some(stamp);
        let Ok(raw) = std::fs::read_to_string(&session.path) else {
            return;
        };
        let lines: Vec<Value> = raw
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        *insight = parse(&lines, session.history_lines.unwrap_or(0));
    }
}

impl DroidTracker {
    fn scan(&mut self, ctx: &LaunchContext) {
        let Some(root) = std::env::var_os("FACTORY_HOME_OVERRIDE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(home_dir)
            .map(|h| h.join(".factory/sessions"))
        else {
            return;
        };
        let wanted = resume_id(ctx);
        let current = self.session.as_ref().map(|s| s.created);
        let mut candidates: Vec<(u64, PathBuf, bool)> = Vec::new();
        for dir in ctx.cwd_variants.iter().map(|cwd| root.join(slug(cwd))) {
            for path in list_dir(&dir) {
                let Some(stem) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(".jsonl"))
                else {
                    continue;
                };
                if self.session.as_ref().is_some_and(|s| s.path == path)
                    || wanted.as_deref().is_some_and(|w| w != stem)
                {
                    continue;
                }
                let created = file_created_ms(&path).unwrap_or(0);
                let updated = file_mtime_ms(&path).unwrap_or(0);
                if wanted.is_none() && !ctx.session_time_ok(created, updated) {
                    continue;
                }
                // A newer session replaces ours only after input typed here (`/new`).
                if current.is_some_and(|c| created <= c || !ctx.created_by_our_submit(created)) {
                    continue;
                }
                let header = read_first_line(&path, 64 * 1024)
                    .and_then(|l| serde_json::from_str::<Value>(&l).ok());
                if !header
                    .as_ref()
                    .and_then(|h| json_str(h, &["cwd"]))
                    .is_some_and(|c| ctx.matches_cwd(c))
                {
                    continue;
                }
                candidates.push((created, path, created >= ctx.min_created_ms()));
            }
        }
        candidates.sort_by_key(|(created, _, _)| *created);
        if let Some(((created, path, fresh), claim)) =
            claim_first("droid", candidates, |(_, p, _)| p.to_string_lossy().into_owned())
        {
            let history_lines = (!fresh).then(|| {
                std::fs::read_to_string(&path)
                    .map(|raw| raw.lines().count())
                    .unwrap_or(0)
            });
            self.claim = Some(claim);
            self.seen = None;
            self.session = Some(Session {
                path,
                created,
                history_lines,
            });
        }
    }
}

fn resume_id(ctx: &LaunchContext) -> Option<String> {
    let id = ctx
        .arg_value(&["--resume", "-r", "--fork"])
        .or_else(|| {
            let mut args = ctx.args.iter().skip(1).filter(|a| !a.starts_with('-'));
            (args.next().map(String::as_str) == Some("resume")).then(|| args.next().cloned()).flatten()
        })?;
    (!id.starts_with('-') && id.len() >= 8).then_some(id)
}

/// `/private/tmp/x` → `-private-tmp-x`.
fn slug(cwd: &str) -> String {
    format!("-{}", cwd.trim_start_matches('/').replace('/', "-"))
}

fn parse(lines: &[Value], skip: usize) -> ExternalInsight {
    let mut insight = ExternalInsight {
        title: lines
            .first()
            .filter(|h| json_str(h, &["type"]) == Some("session_start"))
            .and_then(|h| json_str(h, &["title"]))
            .filter(|t| *t != "New Session")
            .and_then(clean_text_summary),
        ..ExternalInsight::default()
    };
    let mut reply = None;
    let mut error = None;
    let mut question: Option<String> = None;
    for line in lines.iter().skip(skip.max(1)) {
        match json_str(line, &["type"]).unwrap_or("") {
            "message" => {
                let id = json_str(line, &["id"]).unwrap_or("");
                if id.starts_with("context-") || id.starts_with("__internal__") {
                    continue;
                }
                let Some(msg) = line.get("message") else {
                    continue;
                };
                let blocks = msg.get("content").and_then(Value::as_array);
                match json_str(msg, &["role"]) {
                    Some("user") => {
                        for block in blocks.into_iter().flatten() {
                            match json_str(block, &["type"]) {
                                Some("tool_result") => {
                                    if question.take().is_some()
                                        && insight.state == Some(OscState::Blocked)
                                    {
                                        insight.state = Some(OscState::Working);
                                        insight.blocked_kind = None;
                                        insight.blocked_msg = None;
                                    }
                                }
                                Some("text") => {
                                    let text = json_str(block, &["text"]).unwrap_or("");
                                    if text.starts_with("BYOK Error") || text.contains("\n\nUpstream error:") {
                                        error = text.lines().next().and_then(clean_text_summary);
                                    } else if msg.get("userMessageSource").is_some() {
                                        reply = None;
                                        error = None;
                                        question = None;
                                        insight.begin_turn();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some("assistant") => {
                        for block in blocks.into_iter().flatten() {
                            match json_str(block, &["type"]) {
                                Some("text") => {
                                    reply = json_str(block, &["text"]).and_then(clean_text_summary);
                                }
                                Some("tool_use") => {
                                    let input = block.get("input").unwrap_or(&Value::Null);
                                    if json_str(block, &["name"]) == Some("AskUser") {
                                        question = json_str(input, &["questionnaire"])
                                            .map(questionnaire_subject)
                                            .or(Some(String::new()));
                                        insight.state = Some(OscState::Blocked);
                                        insight.blocked_kind = Some(BlockedKind::Question);
                                        insight.blocked_msg =
                                            question.clone().filter(|q| !q.is_empty());
                                    } else if let Some(caption) = json_str(input, &["command"])
                                        .map(|c| format!("$ {c}"))
                                        .or_else(|| json_str(input, &["summary"]).map(str::to_owned))
                                        .as_deref()
                                        .and_then(clean_text_summary)
                                    {
                                        insight.working_msg = Some(caption);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            "todo_state" => {
                if let Some(todos) = line
                    .get("todos")
                    .and_then(|t| t.get("todos"))
                    .and_then(Value::as_array)
                {
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
            "agent_turn_outcome" => {
                question = None;
                match json_str(line, &["reason"]).unwrap_or("") {
                    "completed" => insight.finish_turn(reply.take()),
                    "cancelled" | "canceled" | "permission_rejected" => insight.abort_turn(),
                    _ => insight.fail_turn(error.take()),
                }
            }
            _ => {}
        }
    }
    insight
}

/// `1. [question] Which color?\n[topic] …` → `Which color?`.
fn questionnaire_subject(raw: &str) -> String {
    raw.lines()
        .find_map(|l| l.split_once("[question]").map(|(_, q)| q.trim().to_owned()))
        .unwrap_or_default()
}
