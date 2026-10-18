//! OpenAI Codex CLI: `$CODEX_HOME/state_5.sqlite` (`threads`) locates the session,
//! `sessions/YYYY/MM/DD/rollout-*.jsonl` carries the turn events.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::engine::store::{
    claim_first, clean_text_summary, content_text, env_dir_or_home, json_str, parse_iso_ms,
    row_str, row_u64, short_id, sql_escape_literal, sqlite_rows, todo_progress, Claim,
    ExternalInsight, JsonlTail, LaunchContext, Tracker,
};
use crate::osc7501::{BlockedKind, OscRecord, OscState};

const DB_POLL: Duration = Duration::from_millis(1200);
const MAX_CHILDREN: usize = 8;

#[derive(Default)]
pub struct CodexTracker {
    thread: Option<ThreadRef>,
    claim: Option<Claim>,
    tail: JsonlTail,
    last_db_poll: Option<Instant>,
    db_title: Option<String>,
    children: BTreeMap<String, ChildThread>,
}

#[derive(Clone)]
struct ThreadRef {
    id: String,
    rollout: PathBuf,
    created_ms: u64,
}

struct ChildThread {
    label: String,
    tail: JsonlTail,
    insight: ExternalInsight,
}

impl Tracker for CodexTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        let Some(home) = env_dir_or_home("CODEX_HOME", ".codex") else {
            return;
        };
        let db = home.join("state_5.sqlite");
        let due = self
            .last_db_poll
            .is_none_or(|t| t.elapsed() >= DB_POLL);
        if due && db.is_file() {
            self.last_db_poll = Some(Instant::now());
            self.refresh_thread(ctx, &db);
            self.refresh_children(&db);
        }

        let Some(thread) = self.thread.clone() else {
            return;
        };
        self.tail.open(thread.rollout.clone());
        for line in self.tail.read_lines() {
            apply_rollout_line(&line, ctx, insight);
        }
        if let Some(title) = self.db_title.clone() {
            insight.title = Some(title);
        }

        insight.children = self
            .children
            .iter_mut()
            .map(|(id, child)| {
                for line in child.tail.read_lines() {
                    apply_rollout_line(&line, ctx, &mut child.insight);
                }
                let state = match child.insight.state {
                    Some(OscState::Done) => OscState::Done,
                    Some(OscState::Error) => OscState::Error,
                    _ => OscState::Working,
                };
                let msg = match state {
                    OscState::Done => child.insight.done_msg.clone(),
                    OscState::Error => child.insight.error_msg.clone(),
                    _ => child.insight.working_msg.clone(),
                };
                OscRecord::new(state)
                    .with_id(format!("sub-{id}"))
                    .with_title(Some(child.label.clone()))
                    .with_msg(msg)
            })
            .collect();
    }
}

impl CodexTracker {
    fn refresh_thread(&mut self, ctx: &LaunchContext, db: &std::path::Path) {
        let min = ctx.min_created_ms();
        let time_filter = if ctx.resume {
            format!("(created_at_ms >= {min} OR updated_at_ms >= {min})")
        } else {
            format!("created_at_ms >= {min}")
        };
        let after_current = self
            .thread
            .as_ref()
            .map(|t| format!(" AND created_at_ms > {}", t.created_ms))
            .unwrap_or_default();
        let sql = format!(
            "SELECT id, rollout_path, created_at_ms, recency_at_ms, first_user_message \
             FROM threads WHERE archived = 0 AND cwd IN ({cwds}) AND {time_filter} \
             AND source IN ('cli', 'vscode') \
             AND COALESCE(originator, '') NOT IN ('codex_exec', 'codex_work_desktop'){after_current} \
             ORDER BY created_at_ms ASC LIMIT 8;",
            cwds = ctx.sql_cwd_list(),
        );
        if let Some(rows) = sqlite_rows(db, &sql) {
            // The thread row appears with the first prompt and `recency_at_ms` is that prompt's
            // time, which tells this instance's thread from another one in the same directory.
            let check_owner = !ctx.resume || self.thread.is_some();
            let candidates = rows
                .into_iter()
                .filter(|row| {
                    !check_owner || {
                        let prompted_here = row_u64(row, "recency_at_ms")
                            .filter(|r| *r > 0)
                            .is_none_or(|r| ctx.prompted_here(r));
                        let prompt_in_argv = row_str(row, "first_user_message")
                            .map(str::trim)
                            .filter(|m| !m.is_empty())
                            .is_some_and(|m| ctx.args.iter().skip(1).any(|a| a.trim() == m));
                        prompted_here || prompt_in_argv
                    }
                })
                .filter_map(|row| {
                    Some(ThreadRef {
                        id: row_str(&row, "id")?.to_owned(),
                        rollout: PathBuf::from(row_str(&row, "rollout_path")?),
                        created_ms: row_u64(&row, "created_at_ms").unwrap_or(0),
                    })
                });
            if let Some((thread, claim)) = claim_first("codex", candidates, |t| t.id.clone()) {
                self.claim = Some(claim);
                self.thread = Some(thread);
                self.children.clear();
                self.db_title = None;
            }
        }

        let Some(thread) = self.thread.as_ref() else {
            return;
        };
        let sql = format!(
            "SELECT COALESCE(name, '') AS name, title FROM threads WHERE id = '{}';",
            sql_escape_literal(&thread.id)
        );
        if let Some(row) = sqlite_rows(db, &sql).and_then(|rows| rows.into_iter().next()) {
            self.db_title = row_str(&row, "name")
                .or_else(|| row_str(&row, "title"))
                .and_then(clean_text_summary);
        }
    }

    fn refresh_children(&mut self, db: &std::path::Path) {
        let Some(thread) = self.thread.as_ref() else {
            return;
        };
        let pattern = format!("%\"parent_thread_id\":\"{}\"%", sql_escape_literal(&thread.id));
        let sql = format!(
            "SELECT id, rollout_path, COALESCE(agent_nickname, '') AS nickname, \
             COALESCE(agent_role, '') AS role FROM threads \
             WHERE source LIKE '{pattern}' ORDER BY created_at_ms DESC LIMIT {MAX_CHILDREN};"
        );
        let Some(rows) = sqlite_rows(db, &sql) else {
            return;
        };
        for row in rows {
            let (Some(id), Some(rollout)) = (row_str(&row, "id"), row_str(&row, "rollout_path")) else {
                continue;
            };
            let key = short_id(id, 24);
            if key.is_empty() || self.children.contains_key(&key) {
                continue;
            }
            let label = match (row_str(&row, "nickname"), row_str(&row, "role")) {
                (Some(n), Some(r)) => format!("{n} ({r})"),
                (Some(n), None) => n.to_owned(),
                (None, Some(r)) => r.to_owned(),
                (None, None) => "subagent".to_owned(),
            };
            let mut tail = JsonlTail::default();
            tail.open(PathBuf::from(rollout));
            self.children.insert(
                key,
                ChildThread {
                    label,
                    tail,
                    insight: ExternalInsight::default(),
                },
            );
        }
    }
}

fn apply_rollout_line(line: &Value, ctx: &LaunchContext, insight: &mut ExternalInsight) {
    let Some(kind) = line.get("type").and_then(Value::as_str) else {
        return;
    };
    let Some(payload) = line.get("payload") else {
        return;
    };
    // History replayed from a resumed session must not drive the live state.
    let live = line
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_iso_ms)
        .is_none_or(|ts| ts >= ctx.min_created_ms());

    match kind {
        "event_msg" => match payload.get("type").and_then(Value::as_str).unwrap_or("") {
            "task_started" if live => insight.begin_turn(),
            "task_complete" if live => {
                let msg = payload
                    .get("last_agent_message")
                    .and_then(Value::as_str)
                    .and_then(clean_text_summary);
                insight.finish_turn(msg);
            }
            "turn_aborted" if live => insight.abort_turn(),
            "error" | "stream_error" if live => {
                let msg = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .and_then(clean_text_summary);
                if payload.get("type").and_then(Value::as_str) == Some("error") {
                    insight.fail_turn(msg);
                } else {
                    insight.working_msg = msg;
                }
            }
            "item_completed" => {
                let Some(item) = payload.get("item") else {
                    return;
                };
                match item.get("type").and_then(Value::as_str).unwrap_or("") {
                    "AgentMessage" => {
                        let text = item.get("content").and_then(|c| content_text(c, &[]));
                        let summary = text.as_deref().and_then(clean_text_summary);
                        match item.get("phase").and_then(Value::as_str) {
                            Some("final_answer") => {
                                if summary.is_some() {
                                    insight.done_msg = summary;
                                }
                            }
                            _ if live && summary.is_some() => insight.working_msg = summary,
                            _ => {}
                        }
                    }
                    "CommandExecution" if live => {
                        let cmd = item
                            .get("command")
                            .and_then(Value::as_array)
                            .and_then(|a| a.last())
                            .and_then(Value::as_str)
                            .and_then(clean_text_summary);
                        if let Some(cmd) = cmd {
                            insight.working_msg = Some(format!("$ {cmd}"));
                        }
                    }
                    _ => {}
                }
            }
            "thread_goal_updated" => {
                if let Some(obj) = payload.get("objective").and_then(Value::as_str) {
                    if insight.title.is_none() {
                        insight.title = clean_text_summary(obj);
                    }
                }
            }
            _ => {}
        },
        "response_item" => {
            if payload.get("type").and_then(Value::as_str) != Some("function_call") || !live {
                return;
            }
            let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
            let args: Value = payload
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or(Value::Null);
            match name {
                "update_plan" => {
                    if let Some(plan) = args.get("plan").and_then(Value::as_array) {
                        let items = plan.iter().map(|step| {
                            (
                                json_str(step, &["status"]).unwrap_or(""),
                                json_str(step, &["step"]).unwrap_or(""),
                            )
                        });
                        if let Some((pct, active)) = todo_progress(items) {
                            insight.progress = Some(pct);
                            if active.is_some() {
                                insight.working_msg = active;
                            }
                        }
                    }
                }
                "request_user_input" => {
                    let question = args
                        .get("questions")
                        .and_then(Value::as_array)
                        .and_then(|q| q.first())
                        .and_then(|q| q.get("title").or_else(|| q.get("question")))
                        .and_then(Value::as_str)
                        .and_then(clean_text_summary);
                    insight.state = Some(OscState::Blocked);
                    insight.blocked_kind = Some(BlockedKind::Question);
                    insight.blocked_msg = question;
                }
                _ => {}
            }
        }
        "response_item_output" | "function_call_output" => {}
        _ => {}
    }
    if kind == "response_item"
        && payload.get("type").and_then(Value::as_str) == Some("function_call_output")
        && insight.state == Some(OscState::Blocked)
        && live
    {
        insight.state = Some(OscState::Working);
        insight.blocked_kind = None;
        insight.blocked_msg = None;
    }
}
