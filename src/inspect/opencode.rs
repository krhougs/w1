//! OpenCode (and Kilo, which shares its store): `$XDG_DATA_HOME/opencode/opencode.db`.
//!
//! A root `session` row (physical cwd in `directory`) is created when the first prompt is
//! submitted. The latest message of the session tells the turn state; the `part` table carries
//! replies, tool calls and pending questions; `todo` the plan.

use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::{
    claim_first, clean_text_summary, env_dir_or_home, row_str, row_u64, short_id,
    sql_escape_literal, sqlite_rows, todo_progress, Claim, ExternalInsight, LaunchContext, Tracker,
};
use crate::osc7501::{BlockedKind, OscRecord, OscState};

const DB_POLL: Duration = Duration::from_millis(1000);

pub struct OpenCodeTracker {
    stores: &'static [&'static str],
    session: Option<(String, u64)>,
    claim: Option<Claim>,
    last_poll: Option<Instant>,
}

impl OpenCodeTracker {
    pub fn opencode() -> Self {
        Self::with_stores(&["opencode/opencode.db"])
    }

    pub fn kilo() -> Self {
        Self::with_stores(&["kilo/kilo.db", "kilo/opencode.db"])
    }

    fn with_stores(stores: &'static [&'static str]) -> Self {
        Self {
            stores,
            session: None,
            claim: None,
            last_poll: None,
        }
    }
}

impl Tracker for OpenCodeTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_poll.is_some_and(|t| t.elapsed() < DB_POLL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        let Some(db) = env_dir_or_home("XDG_DATA_HOME", ".local/share").and_then(|d| {
            self.stores.iter().map(|s| d.join(s)).find(|p| p.is_file())
        }) else {
            return;
        };
        self.locate(ctx, &db);
        let Some((sid, _)) = self.session.clone() else {
            return;
        };
        let sid = sql_escape_literal(&sid);
        let min = ctx.min_created_ms();

        if let Some(row) = one(&db, &format!("SELECT title FROM session WHERE id = '{sid}';")) {
            insight.title = row_str(&row, "title")
                .filter(|t| !t.starts_with("New session - "))
                .and_then(clean_text_summary)
                .or_else(|| first_prompt(&db, &sid));
        }

        let latest = one(
            &db,
            &format!(
                "SELECT json_extract(data, '$.role') AS role, \
                   json_extract(data, '$.time.created') AS created, \
                   json_extract(data, '$.time.completed') AS completed, \
                   json_extract(data, '$.finish') AS finish, \
                   json_extract(data, '$.error.name') AS error_name, \
                   json_extract(data, '$.error.data.message') AS error_message \
                 FROM message WHERE session_id = '{sid}' \
                 ORDER BY json_extract(data, '$.time.created') DESC LIMIT 1;"
            ),
        );
        let mut state = latest.as_ref().and_then(|row| message_state(row, min));
        // Declining a permission ends the turn right after the step that asked for it.
        let after_tool_step = latest.as_ref().is_some_and(|r| {
            row_str(r, "finish") == Some("tool-calls") && row_u64(r, "completed").is_some()
        });
        if state == Some(OscState::Working) && after_tool_step && last_tool_rejected(&db, &sid) {
            state = Some(OscState::Idle);
        }
        insight.state = state;
        match state {
            Some(OscState::Done) => insight.done_msg = last_reply(&db, &sid),
            Some(OscState::Error) => {
                insight.error_msg = latest
                    .as_ref()
                    .and_then(|r| row_str(r, "error_message"))
                    .and_then(clean_text_summary);
            }
            _ => {}
        }

        insight.blocked_kind = None;
        insight.blocked_msg = None;
        insight.working_msg = None;
        let active_tool = (state == Some(OscState::Working)).then(|| one(
            &db,
            &format!(
                "SELECT json_extract(data, '$.tool') AS tool, \
                   json_extract(data, '$.state.title') AS title, \
                   json_extract(data, '$.state.input.command') AS command, \
                   json_extract(data, '$.state.input.questions[0].question') AS question \
                 FROM part WHERE session_id = '{sid}' AND json_extract(data, '$.type') = 'tool' \
                   AND json_extract(data, '$.state.status') IN ('pending', 'running') \
                 ORDER BY id DESC LIMIT 1;"
            ),
        ));
        if let Some(row) = active_tool.flatten() {
            if row_str(&row, "tool") == Some("question") {
                insight.state = Some(OscState::Blocked);
                insight.blocked_kind = Some(BlockedKind::Question);
                insight.blocked_msg = row_str(&row, "question").and_then(clean_text_summary);
            } else if let Some(caption) = row_str(&row, "command")
                .map(|c| format!("$ {c}"))
                .or_else(|| row_str(&row, "title").map(str::to_owned))
            {
                insight.working_msg = clean_text_summary(&caption);
            }
        }

        if let Some(rows) = sqlite_rows(
            &db,
            &format!("SELECT status, content FROM todo WHERE session_id = '{sid}' ORDER BY position ASC;"),
        ) {
            insight.progress = None;
            let items = rows
                .iter()
                .map(|r| (row_str(r, "status").unwrap_or(""), row_str(r, "content").unwrap_or("")));
            if let Some((pct, active)) = todo_progress(items) {
                insight.progress = Some(pct);
                if active.is_some() && state == Some(OscState::Working) {
                    insight.working_msg = active;
                }
            }
        }

        insight.children = children(&db, &sid, min);
    }
}

impl OpenCodeTracker {
    fn locate(&mut self, ctx: &LaunchContext, db: &std::path::Path) {
        let mut dirs = ctx.sql_cwd_list();
        // `opencode <dir>` works on that directory instead of the cwd.
        if let Some(dir) = ctx
            .args
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .and_then(|a| std::fs::canonicalize(a).ok())
            .filter(|p| p.is_dir())
        {
            dirs.push_str(&format!(",'{}'", sql_escape_literal(&dir.to_string_lossy())));
        }
        let min = ctx.min_created_ms();
        let after = self
            .session
            .as_ref()
            .map(|(_, c)| format!(" AND time_created > {c}"))
            .unwrap_or_default();
        let sql = if let Some(id) = ctx.arg_value(&["--session", "-s"]).filter(|_| self.session.is_none()) {
            format!(
                "SELECT id, time_created FROM session WHERE id = '{}';",
                sql_escape_literal(&id)
            )
        } else {
            let time = if ctx.resume && self.session.is_none() {
                format!("(time_created >= {min} OR time_updated >= {min})")
            } else {
                format!("time_created >= {min}")
            };
            format!(
                "SELECT id, time_created FROM session WHERE parent_id IS NULL \
                 AND directory IN ({dirs}) AND {time}{after} ORDER BY time_created ASC LIMIT 8;"
            )
        };
        let Some(rows) = sqlite_rows(db, &sql) else {
            return;
        };
        // Sessions are created by the first prompt, so a new one is ours only if it followed
        // input typed here.
        let by_id = ctx.arg_value(&["--session", "-s"]).is_some() && self.session.is_none();
        let fresh_only = !by_id && (!ctx.resume || self.session.is_some());
        let candidates = rows
            .into_iter()
            .filter_map(|r| {
                Some((row_str(&r, "id")?.to_owned(), row_u64(&r, "time_created").unwrap_or(0)))
            })
            .filter(|(_, created)| !fresh_only || ctx.created_by_our_submit(*created));
        if let Some(((id, created), claim)) = claim_first("opencode", candidates, |(id, _)| id.clone()) {
            self.claim = Some(claim);
            self.session = Some((id, created));
        }
    }
}

fn one(db: &std::path::Path, sql: &str) -> Option<Map<String, Value>> {
    sqlite_rows(db, sql)?.into_iter().next()
}

fn message_state(row: &Map<String, Value>, min_ms: u64) -> Option<OscState> {
    if row_u64(row, "created").is_some_and(|c| c < min_ms) {
        return None;
    }
    if row_str(row, "role") == Some("user") {
        return Some(OscState::Working);
    }
    match row_str(row, "error_name") {
        Some("MessageAbortedError") => return Some(OscState::Idle),
        Some(_) => return Some(OscState::Error),
        None => {}
    }
    if row_u64(row, "completed").is_none() {
        return Some(OscState::Working);
    }
    match row_str(row, "finish") {
        Some("tool-calls") => Some(OscState::Working),
        _ => Some(OscState::Done),
    }
}

fn last_tool_rejected(db: &std::path::Path, sid: &str) -> bool {
    one(
        db,
        &format!(
            "SELECT json_extract(data, '$.state.status') AS status, \
               json_extract(data, '$.state.error') AS error \
             FROM part WHERE session_id = '{sid}' AND json_extract(data, '$.type') = 'tool' \
             ORDER BY id DESC LIMIT 1;"
        ),
    )
    .is_some_and(|row| {
        row_str(&row, "status") == Some("error")
            && row_str(&row, "error").is_some_and(|e| e.contains("rejected permission"))
    })
}

fn last_reply(db: &std::path::Path, sid: &str) -> Option<String> {
    let row = one(
        db,
        &format!(
            "SELECT json_extract(p.data, '$.text') AS text FROM part p JOIN message m ON m.id = p.message_id \
             WHERE p.session_id = '{sid}' AND json_extract(m.data, '$.role') = 'assistant' \
               AND json_extract(p.data, '$.type') = 'text' \
               AND json_extract(p.data, '$.time.end') IS NOT NULL \
             ORDER BY p.id DESC LIMIT 1;"
        ),
    )?;
    row_str(&row, "text").and_then(clean_text_summary)
}

fn first_prompt(db: &std::path::Path, sid: &str) -> Option<String> {
    let rows = sqlite_rows(
        db,
        &format!(
            "SELECT json_extract(p.data, '$.text') AS text FROM part p JOIN message m ON m.id = p.message_id \
             WHERE p.session_id = '{sid}' AND json_extract(m.data, '$.role') = 'user' \
               AND json_extract(p.data, '$.type') = 'text' \
               AND COALESCE(json_extract(p.data, '$.synthetic'), 0) = 0 \
             ORDER BY p.id ASC LIMIT 6;"
        ),
    )?;
    rows.iter()
        .filter_map(|r| row_str(r, "text"))
        .find(|t| {
            let t = t.trim_start();
            !t.starts_with('<') && !t.starts_with("[SYSTEM")
        })
        .and_then(clean_text_summary)
}

fn children(db: &std::path::Path, sid: &str, min_ms: u64) -> Vec<OscRecord> {
    let Some(rows) = sqlite_rows(
        db,
        &format!(
            "SELECT s.id AS id, s.title AS title, \
               (SELECT json_object('role', json_extract(m.data, '$.role'), \
                  'created', json_extract(m.data, '$.time.created'), \
                  'completed', json_extract(m.data, '$.time.completed'), \
                  'finish', json_extract(m.data, '$.finish'), \
                  'error_name', json_extract(m.data, '$.error.name')) \
                FROM message m WHERE m.session_id = s.id \
                ORDER BY json_extract(m.data, '$.time.created') DESC LIMIT 1) AS latest \
             FROM session s WHERE s.parent_id = '{sid}' AND s.time_created >= {min_ms} \
             ORDER BY s.time_created DESC LIMIT 8;"
        ),
    ) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|r| {
            let id = short_id(row_str(r, "id")?, 24);
            let latest: Map<String, Value> = row_str(r, "latest")
                .and_then(|l| serde_json::from_str(l).ok())
                .unwrap_or_default();
            let state = match message_state(&latest, 0) {
                Some(OscState::Error) => OscState::Error,
                Some(OscState::Done) | Some(OscState::Idle) => OscState::Done,
                _ => OscState::Working,
            };
            Some(
                OscRecord::new(state)
                    .with_id(format!("sub-{id}"))
                    .with_title(row_str(r, "title").and_then(clean_text_summary)),
            )
        })
        .collect()
}
