//! Crush: one SQLite store per project, `<data dir>/crush.db`.
//!
//! The data dir is `-D/--data-dir`, else the entry for this directory in the global
//! `projects.json` (refreshed on every start), else the nearest `.crush` upwards. A root session
//! (`parent_session_id IS NULL`) is created by the first prompt; times are in seconds. Message
//! rows are updated in place while streaming; the last `finish` part tells how a turn ended.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::engine::store::{
    claim_first, clean_text_summary, env_dir_or_home, json_str, parse_iso_ms, row_str, row_u64,
    sql_escape_literal, sqlite_rows, todo_progress, Claim, ExternalInsight, LaunchContext, Tracker,
};
use crate::osc7501::OscState;

const DB_POLL: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub struct CrushTracker {
    db: Option<PathBuf>,
    session: Option<(String, u64)>,
    claim: Option<Claim>,
    last_poll: Option<Instant>,
}

impl Tracker for CrushTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_poll.is_some_and(|t| t.elapsed() < DB_POLL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        if self.db.is_none() {
            self.db = data_dir(ctx).map(|d| d.join("crush.db")).filter(|p| p.is_file());
        }
        let Some(db) = self.db.clone() else {
            return;
        };
        self.locate(ctx, &db);
        let Some((sid, _)) = self.session.clone() else {
            return;
        };
        let sid = sql_escape_literal(&sid);

        if let Some(row) = one(&db, &format!("SELECT title, todos FROM sessions WHERE id = '{sid}';")) {
            insight.title = row_str(&row, "title")
                .filter(|t| !matches!(*t, "Untitled Session" | "New Session"))
                .and_then(clean_text_summary);
            insight.progress = None;
            insight.working_msg = None;
            let todos: Vec<Value> = row_str(&row, "todos")
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or_default();
            if let Some((pct, _)) = todo_progress(todos.iter().map(|t| {
                (json_str(t, &["status"]).unwrap_or(""), json_str(t, &["content"]).unwrap_or(""))
            })) {
                insight.progress = Some(pct);
                insight.working_msg = todos
                    .iter()
                    .find(|t| json_str(t, &["status"]) == Some("in_progress"))
                    .and_then(|t| json_str(t, &["active_form"]).or_else(|| json_str(t, &["content"])))
                    .and_then(clean_text_summary);
            }
        }

        let Some(latest) = one(
            &db,
            &format!(
                "SELECT role, parts, created_at FROM messages WHERE session_id = '{sid}' \
                 ORDER BY created_at DESC, rowid DESC LIMIT 1;"
            ),
        ) else {
            return;
        };
        if row_u64(&latest, "created_at").is_some_and(|c| c * 1000 + 1000 < ctx.min_created_ms()) {
            return;
        }
        let parts: Vec<Value> = row_str(&latest, "parts")
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or_default();
        let finish = parts
            .iter()
            .rev()
            .find(|p| json_str(p, &["type"]) == Some("finish"))
            .and_then(|p| p.get("data"));
        match (row_str(&latest, "role"), finish.and_then(|f| json_str(f, &["reason"]))) {
            (Some("assistant"), Some("end_turn" | "stop")) => {
                let text = parts
                    .iter()
                    .filter(|p| json_str(p, &["type"]) == Some("text"))
                    .filter_map(|p| json_str(p, &["data", "text"]))
                    .collect::<Vec<_>>()
                    .join("\n");
                insight.finish_turn(clean_text_summary(&text));
            }
            (Some("assistant"), Some("canceled" | "cancelled")) => insight.abort_turn(),
            (Some("assistant"), Some("error")) => {
                let msg = finish.and_then(|f| {
                    let head = json_str(f, &["message"]).unwrap_or("");
                    let details = json_str(f, &["details"]).unwrap_or("");
                    let joined = match (head.is_empty(), details.is_empty()) {
                        (false, false) => format!("{head}: {details}"),
                        (false, true) => head.to_owned(),
                        _ => details.to_owned(),
                    };
                    clean_text_summary(&joined)
                });
                insight.fail_turn(msg);
            }
            _ => {
                if insight.state != Some(OscState::Working) {
                    insight.state = Some(OscState::Working);
                    insight.done_msg = None;
                    insight.error_msg = None;
                }
            }
        }
    }
}

impl CrushTracker {
    fn locate(&mut self, ctx: &LaunchContext, db: &Path) {
        let min_s = ctx.min_created_ms() / 1000;
        let prefix = ctx
            .arg_value(&["--session", "-s"])
            .filter(|_| self.session.is_none());
        let sql = if let Some(prefix) = prefix.as_deref() {
            format!(
                "SELECT id, created_at FROM sessions WHERE parent_session_id IS NULL \
                 AND id LIKE '{}%' LIMIT 1;",
                sql_escape_literal(prefix)
            )
        } else {
            let time = if ctx.resume && self.session.is_none() {
                format!("(created_at >= {min_s} OR updated_at >= {min_s})")
            } else {
                format!("created_at >= {min_s}")
            };
            let after = self
                .session
                .as_ref()
                .map(|(_, c)| format!(" AND created_at >= {c} AND id != '{}'", sql_escape_literal(&self.session_id())))
                .unwrap_or_default();
            format!(
                "SELECT id, created_at FROM sessions WHERE parent_session_id IS NULL \
                 AND {time}{after} ORDER BY created_at ASC, rowid ASC LIMIT 8;"
            )
        };
        let Some(rows) = sqlite_rows(db, &sql) else {
            return;
        };
        // Sessions are created by the first prompt; the store records neither cwd nor pid, so a
        // new one is ours only if it followed input typed here.
        let fresh_only = prefix.is_none() && (!ctx.resume || self.session.is_some());
        let candidates = rows
            .iter()
            .filter_map(|r| Some((row_str(r, "id")?.to_owned(), row_u64(r, "created_at").unwrap_or(0))))
            .filter(|(_, created)| !fresh_only || ctx.created_by_our_submit(created * 1000));
        if let Some(((id, created), claim)) = claim_first("crush", candidates, |(id, _)| id.clone()) {
            self.claim = Some(claim);
            self.session = Some((id, created));
        }
    }

    fn session_id(&self) -> String {
        self.session.as_ref().map(|(id, _)| id.clone()).unwrap_or_default()
    }
}

fn one(db: &Path, sql: &str) -> Option<Map<String, Value>> {
    sqlite_rows(db, sql)?.into_iter().next()
}

fn data_dir(ctx: &LaunchContext) -> Option<PathBuf> {
    let cwd = ctx.cwd_variants.first().map(PathBuf::from)?;
    if let Some(dir) = ctx.arg_value(&["--data-dir", "-D"]) {
        let dir = PathBuf::from(dir);
        return Some(if dir.is_absolute() { dir } else { cwd.join(dir) });
    }
    let registry = std::env::var_os("CRUSH_GLOBAL_DATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env_dir_or_home("XDG_DATA_HOME", ".local/share").map(|d| d.join("crush")))
        .map(|d| d.join("projects.json"));
    let registered = registry
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|v| {
            v.get("projects")?
                .as_array()?
                .iter()
                .filter(|p| json_str(p, &["path"]).is_some_and(|path| ctx.cwd_variants.iter().any(|c| c == path)))
                .max_by_key(|p| json_str(p, &["last_accessed"]).and_then(parse_iso_ms).unwrap_or(0))
                .and_then(|p| json_str(p, &["data_dir"]))
                .map(PathBuf::from)
        });
    if registered.is_some() {
        return registered;
    }
    cwd.ancestors()
        .map(|d| d.join(".crush"))
        .find(|d| d.is_dir())
        .or_else(|| Some(cwd.join(".crush")))
}
