//! Combines screen detection, the agent's session store and any OSC 7501 the child emits itself
//! into the records `w1` reports to the outer terminal.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use super::rules::{self, DetectedState, DetectionInput};
use super::screen::{PromptMark, VirtualScreen};
use super::stabilizer::StateStabilizer;
use super::store::{ExternalInsight, LaunchContext, SessionInspector, SubmitLog, POLL_INTERVAL};
use crate::agents::{AgentKind, NativeQuirks, ResolvedTarget};
use crate::osc7501::{parse_report, BlockedKind, OscEmitter, OscRecord, OscState, ParsedReport};

const MAX_NATIVE_RECORDS: usize = 64;

pub struct Reporter {
    app_id: String,
    display_name: String,
    agent: Option<AgentKind>,
    screen_title_fallback: bool,
    screen: VirtualScreen,
    stabilizer: StateStabilizer,
    emitter: OscEmitter,
    inspector: InspectorHandle,
    native_root: Option<OscRecord>,
    native_children: BTreeMap<String, OscRecord>,
    native_quirks: NativeQuirks,
    /// A root record held back until it has stood for `native_quirks.settle`.
    native_pending: Option<(Instant, OscRecord)>,
    /// Whether the dialog of the current native `blocked` has been on screen.
    native_blocked_shown: bool,
    last_emit: Option<(Instant, OscState)>,
    mark_error: Option<String>,
    /// Last store state and turn seen, and whether its turn-ending value still has to be
    /// applied. A store lags behind a fresh submit, so an old done/idle/error must not end the
    /// new turn.
    ext_seen: (Option<OscState>, u64),
    ext_end_pending: bool,
    submits: SubmitLog,
    has_tracker: bool,
    /// When the screen alone ended a turn the store still shows as running.
    done_hold: Option<Instant>,
    /// The child's own OSC 0/1/2 titles are dropped, so `w1` sets the terminal title itself.
    owns_title: bool,
    sent_title: Option<String>,
}

/// How long a turn end seen only on screen waits for the store to say how it ended (an abort
/// or a declined permission must not flash `done`).
const STORE_CONFIRM_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Same-state refreshes (streaming text, spinner captions) are coalesced to this rate.
const DETAIL_REFRESH: std::time::Duration = std::time::Duration::from_millis(400);

impl Reporter {
    pub fn new(
        target: &ResolvedTarget,
        child_pid: i32,
        cols: u16,
        rows: u16,
        owns_title: bool,
    ) -> Self {
        let ctx = LaunchContext::new(target.agent, &target.argv, Some(child_pid));
        let submits = ctx.submits.clone();
        let inspector = SessionInspector::new(target.agent, ctx);
        let screen_title_fallback = !inspector.has_tracker();
        Self {
            app_id: target.app_id.clone(),
            display_name: target.display_name.clone(),
            agent: target.agent,
            screen_title_fallback,
            screen: VirtualScreen::new(cols, rows),
            stabilizer: StateStabilizer::default(),
            emitter: OscEmitter::new(&target.app_id),
            inspector: InspectorHandle::spawn(inspector),
            native_root: None,
            native_children: BTreeMap::new(),
            native_quirks: target
                .agent
                .map_or(NativeQuirks::NONE, |a| a.profile().native_quirks),
            native_pending: None,
            native_blocked_shown: false,
            last_emit: None,
            mark_error: None,
            ext_seen: (None, 0),
            ext_end_pending: false,
            submits,
            has_tracker: !screen_title_fallback,
            done_hold: None,
            owns_title,
            sent_title: None,
        }
    }

    pub fn feed_output(&mut self, data: &[u8], reset: bool) {
        self.screen.feed(data);
        let marks = self.screen.take_prompt_marks();
        if reset || marks.contains(&PromptMark::PromptStart) {
            self.emitter.invalidate();
            self.last_emit = None;
            self.sent_title = None;
        }
        for mark in marks {
            match mark {
                PromptMark::CommandStart if self.stabilizer.is_armed() => {
                    self.stabilizer.force_state(OscState::Working);
                }
                PromptMark::CommandFinished(code) if self.stabilizer.turn_active() => {
                    let state = match code {
                        Some(0) | None => OscState::Done,
                        Some(130) => OscState::Idle,
                        Some(_) => {
                            self.mark_error = error_line(&self.screen.viewport_text());
                            OscState::Error
                        }
                    };
                    self.stabilizer.force_state(state);
                }
                _ => {}
            }
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.screen.resize(cols, rows);
    }

    pub fn user_submitted(&mut self) {
        self.submits.record();
        if self.stabilizer.current() != OscState::Blocked {
            self.ext_end_pending = false;
        }
        self.stabilizer.arm();
    }

    pub fn current_state(&self) -> OscState {
        self.stabilizer.current()
    }

    pub fn native_report(&mut self, payload: &str) {
        match parse_report(payload) {
            Some(ParsedReport::Record { id, record }) => {
                if id.is_empty() && self.native_quirks.settle.is_some() {
                    self.native_pending = Some((Instant::now(), record));
                } else if id.is_empty() {
                    self.native_root = Some(record);
                } else if self.native_children.contains_key(&id)
                    || self.native_children.len() < MAX_NATIVE_RECORDS
                {
                    self.native_children.insert(id, record);
                }
            }
            Some(ParsedReport::Clear { id }) => {
                if id.is_empty() {
                    self.native_root = None;
                    self.native_pending = None;
                    self.native_children.clear();
                } else {
                    let prefix = format!("{id}/");
                    self.native_children
                        .retain(|k, _| *k != id && !k.starts_with(&prefix));
                }
            }
            None => {}
        }
    }

    pub fn initial(&mut self, out: &mut Vec<u8>) {
        let title = self.title(&ExternalInsight::default());
        self.set_terminal_title(title.as_deref(), out);
        let root = OscRecord::new(OscState::Idle)
            .with_app(&self.app_id)
            .with_title(title);
        self.emitter.sync(root, &[], out);
    }

    fn set_terminal_title(&mut self, title: Option<&str>, out: &mut Vec<u8>) {
        if !self.owns_title {
            return;
        }
        let title = title
            .and_then(|t| crate::osc7501::sanitize_single_line(t, crate::osc7501::MAX_TITLE_DECODED_BYTES))
            .unwrap_or_default();
        if self.sent_title.as_deref() == Some(title.as_str()) {
            return;
        }
        out.extend_from_slice(b"\x1b]0;");
        out.extend_from_slice(title.as_bytes());
        out.extend_from_slice(b"\x1b\\");
        self.sent_title = Some(title);
    }

    pub fn evaluate(&mut self, now: Instant, out: &mut Vec<u8>) {
        let insight = self.inspector.snapshot();
        let root = self.compose_root(&insight, now);
        self.set_terminal_title(root.title.clone().as_deref(), out);
        if self
            .last_emit
            .is_some_and(|(at, state)| state == root.state && now.duration_since(at) < DETAIL_REFRESH)
        {
            return;
        }
        let state = root.state;
        let mut children: Vec<OscRecord> = self.native_children.values().cloned().collect();
        children.extend(insight.children.iter().cloned());
        let before = out.len();
        self.emitter.sync(root, &children, out);
        if out.len() != before {
            self.last_emit = Some((now, state));
        }
    }

    /// Final report when the child exits: an abnormal exit stays visible as `error`, anything
    /// else clears the records so no stale status outlives the agent.
    pub fn finish(mut self, exit_code: u8, out: &mut Vec<u8>) {
        let insight = self.inspector.finish();
        let by_user_signal = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP]
            .iter()
            .any(|sig| u32::from(exit_code) == 128 + *sig as u32);
        let abnormal = exit_code != 0 && !by_user_signal;
        let final_record = abnormal.then(|| {
            let msg = insight
                .error_msg
                .clone()
                .unwrap_or_else(|| format!("{} exited with status {exit_code}", self.display_name));
            OscRecord::new(OscState::Error)
                .with_app(&self.app_id)
                .with_title(self.title(&insight))
                .with_msg(Some(msg))
        });
        self.emitter.emit_exit(final_record, out);
        if self.sent_title.is_some() {
            self.set_terminal_title(Some(""), out);
        }
    }

    fn title(&self, insight: &ExternalInsight) -> Option<String> {
        insight
            .title
            .clone()
            .or_else(|| {
                self.screen_title_fallback
                    .then(|| self.screen.clean_title(&self.app_id, &self.display_name))
                    .flatten()
            })
            .or_else(|| self.owns_title.then(|| format!("w1 - {}", self.display_name)))
    }

    fn compose_root(&mut self, insight: &ExternalInsight, now: Instant) -> OscRecord {
        let settle = self.native_quirks.settle.unwrap_or_default();
        if let Some((_, record)) = self
            .native_pending
            .take_if(|(at, _)| now.duration_since(*at) >= settle)
        {
            if self.native_root.as_ref() != Some(&record) {
                self.native_blocked_shown = false;
            }
            self.native_root = Some(record);
        }
        let viewport = self.screen.viewport_text();
        let det = rules::detect(
            self.agent,
            DetectionInput {
                screen: &viewport,
                osc_title: self.screen.osc_title(),
                osc_progress: self.screen.osc_progress(),
            },
        );

        let mut state = self.stabilizer.update(&det, now);
        if (insight.state, insight.turn) != self.ext_seen {
            self.ext_seen = (insight.state, insight.turn);
            self.ext_end_pending = true;
        }
        if det.state == DetectedState::Blocked {
            state = OscState::Blocked;
        } else if let Some(ext) = insight.state {
            match (state, ext) {
                (_, OscState::Blocked) => {
                    state = ext;
                    self.stabilizer.force_state(ext);
                }
                (_, OscState::Error) if self.ext_end_pending || state == OscState::Error => {
                    self.ext_end_pending = false;
                    state = ext;
                    self.stabilizer.force_state(ext);
                }
                (OscState::Idle | OscState::Working | OscState::Blocked, OscState::Done)
                    if self.ext_end_pending && det.state != DetectedState::Working =>
                {
                    self.ext_end_pending = false;
                    state = OscState::Done;
                    self.stabilizer.force_state(OscState::Done);
                }
                (OscState::Idle | OscState::Done, OscState::Working) if !det.visible_idle => {
                    state = OscState::Working;
                    self.stabilizer.force_state(OscState::Working);
                }
                (OscState::Working | OscState::Done, OscState::Idle)
                    if self.ext_end_pending && !det.visible_working =>
                {
                    self.ext_end_pending = false;
                    state = OscState::Idle;
                    self.stabilizer.force_state(OscState::Idle);
                }
                _ => {}
            }
        }

        if state == OscState::Done && self.has_tracker && insight.state == Some(OscState::Working) {
            let since = *self.done_hold.get_or_insert(now);
            if now.duration_since(since) < STORE_CONFIRM_WAIT {
                state = OscState::Working;
            }
        } else {
            self.done_hold = None;
        }

        let store_question = insight.state == Some(OscState::Blocked)
            && insight.blocked_kind == Some(BlockedKind::Question);
        let mut kind = (state == OscState::Blocked).then(|| {
            if store_question {
                BlockedKind::Question
            } else {
                det.blocked_kind
                    .or(insight.blocked_kind)
                    .unwrap_or(BlockedKind::Permission)
            }
        });
        let mut progress = matches!(state, OscState::Working | OscState::Blocked)
            .then(|| self.screen.parsed_osc_progress_percent().or(insight.progress))
            .flatten();
        let mut title = self.title(insight);
        let mut msg = match state {
            // A pending request recorded by the store is exact; otherwise read the dialog.
            OscState::Blocked if insight.state == Some(OscState::Blocked) => {
                insight.blocked_msg.clone().or_else(|| det.detail.clone())
            }
            OscState::Blocked => det.detail.clone().or_else(|| insight.blocked_msg.clone()),
            OscState::Working => insight
                .working_msg
                .clone()
                .or_else(|| det.detail.clone()),
            OscState::Done => insight.done_msg.clone(),
            OscState::Error => insight.error_msg.clone().or_else(|| self.mark_error.clone()),
            OscState::Idle => None,
        };
        let mut app = None;

        // Redraws between dialogs must not drop a native `blocked` that was confirmed on screen.
        if det.state == DetectedState::Blocked {
            self.native_blocked_shown = true;
        } else if det.visible_idle {
            self.native_blocked_shown = false;
        }
        let native = self.native_root.clone().filter(|n| {
            n.state != OscState::Blocked
                || self.native_blocked_shown
                || !self.native_quirks.blocked_needs_screen
        });
        if let Some(native) = native {
            // The child's own report is authoritative for blocked/error and anything it fills in;
            // w1 only completes missing fields and the working phase the child does not report.
            // Natively reporting agents so far miss turn starts or errors (reporting `idle`
            // instead), so activity and errors w1 sees from the screen or the store win over a
            // resting native state.
            let merged_state = match (native.state, state) {
                (OscState::Idle | OscState::Done, OscState::Working | OscState::Error) => state,
                (OscState::Idle | OscState::Done, _) if det.state == DetectedState::Working => {
                    OscState::Working
                }
                (OscState::Idle, OscState::Done) if self.native_quirks.idle_for_done => state,
                // The store recorded the turn as aborted or declined.
                (OscState::Done, OscState::Idle) if insight.state == Some(OscState::Idle) => state,
                (native_state, _) => native_state,
            };
            if merged_state != state {
                msg = match merged_state {
                    OscState::Done => insight.done_msg.clone(),
                    OscState::Error => insight.error_msg.clone(),
                    OscState::Working => insight.working_msg.clone().or_else(|| det.detail.clone()),
                    OscState::Blocked => insight.blocked_msg.clone(),
                    OscState::Idle => None,
                };
            }
            state = merged_state;
            kind = (state == OscState::Blocked)
                .then(|| native.kind.or(kind).unwrap_or(BlockedKind::Permission));
            if native.state == state {
                progress = native.progress.or(progress);
                msg = native.msg.clone().or(msg);
            }
            if !matches!(state, OscState::Working | OscState::Blocked) {
                progress = None;
            }
            title = native.title.clone().or(title);
            app = native.app.clone();
        }

        let mut record = OscRecord::new(state)
            .with_app(app.unwrap_or_else(|| self.app_id.clone()))
            .with_kind(kind)
            .with_progress(progress)
            .with_title(title)
            .with_msg(msg);
        if record.state == OscState::Idle {
            record.msg = None;
        }
        record
    }
}

/// The most recent warning/error line on screen (`⚠ …`, `✗ …`, `Error: …`).
fn error_line(screen: &str) -> Option<String> {
    screen.lines().rev().take(40).find_map(|line| {
        let t = line.trim();
        let body = t
            .strip_prefix('⚠')
            .or_else(|| t.strip_prefix('✗'))
            .or_else(|| t.strip_prefix('✕'))
            .or_else(|| t.strip_prefix("Error:"))?;
        crate::osc7501::sanitize_single_line(body, 240)
    })
}

/// Runs the session inspector on its own thread: it shells out to `sqlite3` and reads files,
/// which must never stall terminal I/O.
struct InspectorHandle {
    shared: Arc<Mutex<ExternalInsight>>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl InspectorHandle {
    fn spawn(mut inspector: SessionInspector) -> Self {
        let shared = Arc::new(Mutex::new(ExternalInsight::default()));
        if !inspector.has_tracker() {
            return Self {
                shared,
                stop: None,
                thread: None,
            };
        }
        let (tx, rx) = mpsc::channel::<()>();
        let worker_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("w1-inspector".into())
            .spawn(move || loop {
                let snapshot = inspector.poll();
                match worker_shared.lock() {
                    Ok(mut guard) => *guard = snapshot,
                    Err(poisoned) => *poisoned.into_inner() = snapshot,
                }
                match rx.recv_timeout(POLL_INTERVAL) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                        let snapshot = inspector.poll();
                        match worker_shared.lock() {
                            Ok(mut guard) => *guard = snapshot,
                            Err(poisoned) => *poisoned.into_inner() = snapshot,
                        }
                        break;
                    }
                }
            })
            .ok();
        Self {
            shared,
            stop: thread.as_ref().map(|_| tx),
            thread,
        }
    }

    fn snapshot(&self) -> ExternalInsight {
        match self.shared.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn finish(&mut self) -> ExternalInsight {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.snapshot()
    }
}

impl Drop for InspectorHandle {
    fn drop(&mut self) {
        self.stop.take();
    }
}
