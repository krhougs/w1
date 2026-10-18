//! Antigravity CLI (`agy`): `~/.gemini/antigravity-cli/`.
//!
//! Each agy process writes `log/cli-*.log` whose first line carries its pid and which names the
//! conversation it creates or resumes. `conversation_summaries.db` holds title and run status;
//! `conversations/<id>.db` holds the protobuf-encoded steps.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::engine::store::{
    claim_first, clean_text_summary, file_created_ms, file_mtime_ms, home_dir, list_dir,
    read_first_line, row_str, sql_escape_literal, sqlite_rows, Claim, ExternalInsight,
    LaunchContext, Tracker,
};
use crate::osc7501::OscState;

const DB_POLL: Duration = Duration::from_millis(1000);
const LOG_TAIL_BYTES: u64 = 512 * 1024;
const STEP_PLANNER_RESPONSE: u64 = 15;

#[derive(Default)]
pub struct AgyTracker {
    log: Option<PathBuf>,
    conversation: Option<String>,
    claim: Option<Claim>,
    running: bool,
    last_poll: Option<Instant>,
}

impl Tracker for AgyTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_poll.is_some_and(|t| t.elapsed() < DB_POLL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        let Some(root) = home_dir().map(|h| h.join(".gemini/antigravity-cli")) else {
            return;
        };
        let located = self.locate_from_log(&root, ctx).or_else(|| {
            if self.log.is_some() {
                None
            } else {
                self.locate_from_db(&root, ctx)
            }
        });
        if let Some(id) = located {
            if self.conversation.as_deref() != Some(id.as_str()) {
                *insight = ExternalInsight::default();
                self.running = false;
                self.conversation = Some(id);
            }
        }
        let Some(id) = self.conversation.clone() else {
            return;
        };
        let sql = format!(
            "SELECT title, status FROM conversation_summaries WHERE conversation_id = '{}';",
            sql_escape_literal(&id)
        );
        let Some(row) = sqlite_rows(&root.join("conversation_summaries.db"), &sql)
            .and_then(|rows| rows.into_iter().next())
        else {
            return;
        };
        if let Some(title) = row_str(&row, "title").and_then(clean_text_summary) {
            insight.title = Some(title);
        }
        let running = row_str(&row, "status").is_some_and(|s| s.ends_with("_RUNNING"));
        if running && !self.running {
            insight.begin_turn();
        } else if !running && self.running {
            let reply = last_reply(&root.join("conversations").join(format!("{id}.db")));
            insight.finish_turn(reply);
        }
        if !running && insight.state == Some(OscState::Working) {
            insight.state = None;
        }
        self.running = running;
    }
}

impl AgyTracker {
    /// The conversation named by the log of an agy process descending from our child.
    fn locate_from_log(&mut self, root: &Path, ctx: &LaunchContext) -> Option<String> {
        if self.log.is_none() {
            let child = ctx.child_pid?;
            self.log = list_dir(&root.join("log")).into_iter().find(|path| {
                file_created_ms(path).is_some_and(|c| c >= ctx.min_created_ms())
                    && read_first_line(path, 4096)
                        .and_then(|l| {
                            l.rsplit_once("with pid ")
                                .and_then(|(_, pid)| pid.trim().parse::<i32>().ok())
                        })
                        .is_some_and(|pid| crate::procs::is_descendant_of(pid, child))
            });
        }
        let log = self.log.as_deref()?;
        let text = read_tail(log, LOG_TAIL_BYTES)?;
        const MARKERS: [&str; 4] = [
            "Created conversation ",
            "Continuing last-used conversation (from cache): ",
            "Resuming conversation ",
            "Sending user message to conversation ",
        ];
        text.lines().rev().find_map(|line| {
            MARKERS.iter().find_map(|m| {
                let (_, rest) = line.split_once(m)?;
                let id: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_hexdigit() || *c == '-')
                    .collect();
                (id.len() >= 32).then_some(id)
            })
        })
    }

    fn locate_from_db(&mut self, root: &Path, ctx: &LaunchContext) -> Option<String> {
        if self.conversation.is_some() {
            return None;
        }
        let rows = sqlite_rows(
            &root.join("conversation_summaries.db"),
            "SELECT conversation_id, workspace_uris FROM conversation_summaries \
             WHERE app_data_dir = 'antigravity-cli' ORDER BY last_modified_time DESC LIMIT 32;",
        )?;
        let conversations = root.join("conversations");
        let mut candidates: Vec<(u64, String)> = rows
            .iter()
            .filter_map(|row| {
                let id = row_str(row, "conversation_id")?;
                let uris: Vec<String> = serde_json::from_str(row_str(row, "workspace_uris")?).ok()?;
                if !uris.iter().any(|u| ctx.matches_cwd(&percent_decode(u))) {
                    return None;
                }
                let db = conversations.join(format!("{id}.db"));
                let created = file_created_ms(&db).unwrap_or(0);
                let updated = file_mtime_ms(&db).unwrap_or(0);
                ctx.session_time_ok(created, updated)
                    .then(|| (created, id.to_owned()))
            })
            .collect();
        candidates.sort();
        let ((_, id), claim) = claim_first("agy", candidates, |(_, id)| id.clone())?;
        self.claim = Some(claim);
        Some(id)
    }
}

fn read_tail(path: &Path, max: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Text of the last planner response (step type 15, field 20.1 of the step payload).
fn last_reply(db: &Path) -> Option<String> {
    let sql = format!(
        "SELECT hex(step_payload) AS payload FROM steps WHERE step_type = {STEP_PLANNER_RESPONSE} \
         ORDER BY idx DESC LIMIT 4;"
    );
    for row in sqlite_rows(db, &sql)? {
        let Some(bytes) = row_str(&row, "payload").and_then(hex_decode) else {
            continue;
        };
        let text = proto_field(&bytes, 20)
            .and_then(|inner| proto_field(inner, 1))
            .and_then(|t| std::str::from_utf8(t).ok())
            .and_then(clean_text_summary);
        if text.is_some() {
            return text;
        }
    }
    None
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    let bytes = hex.as_bytes();
    bytes
        .chunks(2)
        .map(|pair| {
            let s = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(s, 16).ok()
        })
        .collect()
}

fn read_varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos)?;
        *pos += 1;
        value |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// First length-delimited field `number` of a protobuf message.
fn proto_field(data: &[u8], number: u64) -> Option<&[u8]> {
    let mut pos = 0;
    while pos < data.len() {
        let key = read_varint(data, &mut pos)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                read_varint(data, &mut pos)?;
            }
            1 => pos = pos.checked_add(8)?,
            2 => {
                let len = usize::try_from(read_varint(data, &mut pos)?).ok()?;
                let end = pos.checked_add(len)?;
                let value = data.get(pos..end)?;
                if field == number {
                    return Some(value);
                }
                pos = end;
            }
            5 => pos = pos.checked_add(4)?,
            _ => return None,
        }
    }
    None
}
