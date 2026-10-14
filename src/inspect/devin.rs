//! Devin CLI: `$XDG_DATA_HOME/devin/cli/` (default `~/.local/share/devin/cli`).
//!
//! `session_locks/<id>.lock` holds the pid of the `devin acp` process serving the session, which
//! is a descendant of the process `w1` spawned, so the lock pins the session exactly (including
//! `-r`/`-c` and `/clear`). `sessions.db` rows only appear once the first prompt is submitted.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{
    clean_text_summary, env_dir_or_home, file_mtime_ms, list_dir, row_str, row_u64,
    sql_escape_literal, sqlite_rows, todo_progress, ExternalInsight, LaunchContext, Tracker,
};
use crate::osc7501::BlockedKind;

const DB_POLL: Duration = Duration::from_millis(1000);
const CHAIN_WALK: usize = 80;

#[derive(Default)]
pub struct DevinTracker {
    session: Option<String>,
    last_poll: Option<Instant>,
}

impl Tracker for DevinTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_poll.is_some_and(|t| t.elapsed() < DB_POLL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        let Some(root) = env_dir_or_home("XDG_DATA_HOME", ".local/share").map(|d| d.join("devin/cli"))
        else {
            return;
        };
        if let Some(id) = locked_session(&root, ctx) {
            if self.session.as_deref() != Some(id.as_str()) {
                *insight = ExternalInsight::default();
                self.session = Some(id);
            }
        }
        let Some(id) = self.session.as_deref() else {
            return;
        };
        let db = root.join("sessions.db");
        let sid = sql_escape_literal(id);
        let Some(row) = sqlite_rows(
            &db,
            &format!("SELECT title, main_chain_id FROM sessions WHERE id = '{sid}';"),
        )
        .and_then(|rows| rows.into_iter().next()) else {
            return;
        };
        if let Some(title) = row_str(&row, "title").filter(|t| !is_tool_call_title(t)) {
            insight.title = clean_text_summary(title);
        }
        let Some(head) = row_u64(&row, "main_chain_id") else {
            return;
        };
        let sql = format!(
            "WITH RECURSIVE chain(node_id, parent_node_id, chat_message, depth) AS ( \
               SELECT node_id, parent_node_id, chat_message, 0 FROM message_nodes \
                 WHERE session_id = '{sid}' AND node_id = {head} \
               UNION ALL \
               SELECT m.node_id, m.parent_node_id, m.chat_message, c.depth + 1 \
                 FROM message_nodes m JOIN chain c ON m.node_id = c.parent_node_id \
                 WHERE m.session_id = '{sid}' AND c.depth < {CHAIN_WALK}) \
             SELECT chat_message FROM chain ORDER BY depth ASC;"
        );
        let Some(rows) = sqlite_rows(&db, &sql) else {
            return;
        };
        let chain: Vec<Value> = rows
            .iter()
            .filter_map(|r| row_str(r, "chat_message"))
            .filter_map(|m| serde_json::from_str(m).ok())
            .collect();
        apply_chain(&chain, insight);
    }
}

fn locked_session(root: &Path, ctx: &LaunchContext) -> Option<String> {
    let child = ctx.child_pid?;
    let mut best: Option<(u64, String)> = None;
    for lock in list_dir(&root.join("session_locks")) {
        let Some(id) = lock
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".lock"))
        else {
            continue;
        };
        let Some(mtime) = file_mtime_ms(&lock).filter(|m| *m >= ctx.min_created_ms()) else {
            continue;
        };
        let pid = std::fs::read_to_string(&lock)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        if pid.is_some_and(|p| crate::procs::is_descendant_of(p, child))
            && best.as_ref().is_none_or(|(m, _)| mtime > *m)
        {
            best = Some((mtime, id.to_owned()));
        }
    }
    best.map(|(_, id)| id)
}

/// Devin sometimes stores the first tool call as the generated title.
fn is_tool_call_title(title: &str) -> bool {
    let t = title.trim_start();
    t.starts_with("functions.") && t.contains('{')
}

/// `chain` runs from the head node backwards.
fn apply_chain(chain: &[Value], insight: &mut ExternalInsight) {
    insight.done_msg = None;
    if let Some(head) = chain.first() {
        let role = head.get("role").and_then(Value::as_str).unwrap_or("");
        let finish = head
            .get("metadata")
            .and_then(|m| m.get("finish_reason"))
            .and_then(Value::as_str);
        if role == "assistant" && finish == Some("stop") {
            insight.done_msg = head
                .get("content")
                .and_then(|c| super::content_text(c, &[]))
                .as_deref()
                .and_then(clean_text_summary);
        }
    }

    insight.progress = None;
    insight.blocked_msg = None;
    insight.blocked_kind = None;
    for msg in chain {
        let Some(calls) = msg.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for call in calls.iter().rev() {
            let name = call
                .get("name")
                .or_else(|| call.get("function").and_then(|f| f.get("name")))
                .and_then(Value::as_str)
                .unwrap_or("");
            let args: Value = call
                .get("arguments")
                .or_else(|| call.get("function").and_then(|f| f.get("arguments")))
                .and_then(|a| match a {
                    Value::String(s) => serde_json::from_str(s).ok(),
                    other => Some(other.clone()),
                })
                .unwrap_or(Value::Null);
            if name == "todo_write" && insight.progress.is_none() {
                if let Some(todos) = args.get("todos").and_then(Value::as_array) {
                    let items = todos.iter().map(|t| {
                        (
                            t.get("status").and_then(Value::as_str).unwrap_or(""),
                            t.get("content").and_then(Value::as_str).unwrap_or(""),
                        )
                    });
                    if let Some((pct, active)) = todo_progress(items) {
                        insight.progress = Some(pct);
                        insight.working_msg = active.or(insight.working_msg.take());
                    }
                }
            }
            if name == "ask_user_question" && chain.first().is_some_and(|h| std::ptr::eq(msg, h)) {
                insight.blocked_kind = Some(BlockedKind::Question);
                insight.blocked_msg = args
                    .get("questions")
                    .and_then(Value::as_array)
                    .and_then(|q| q.first())
                    .and_then(|q| q.get("question"))
                    .and_then(Value::as_str)
                    .and_then(clean_text_summary);
            }
        }
    }
}
