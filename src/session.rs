//! The embeddable core of `w1`: everything that happens between an agent's PTY and the outer
//! terminal, without owning any file descriptor. The `w1` binary drives it from its own PTY
//! loop; a host that already runs agents in PTYs (a terminal, a multiplexer, a relay) can drive
//! it the same way.

use std::time::{Duration, Instant};

use crate::agents::{AgentKind, ResolvedTarget};
use crate::osc7501::OscState;
use crate::report::Reporter;
use crate::stream::{Captured, OutputStream, TitleFilter};

/// Status refresh cadence while the agent is active, and once it has been quiet for a while.
const TICK_INTERVAL: Duration = Duration::from_millis(180);
const IDLE_TICK_INTERVAL: Duration = Duration::from_millis(900);
const ACTIVE_WINDOW: Duration = Duration::from_secs(3);
/// An OSC sequence whose head arrived but whose body did not is released after this long, so a
/// half-written title never holds back the agent's output.
const OSC_HEAD_STALL: Duration = Duration::from_millis(120);

/// One wrapped agent.
///
/// Feed it the user's keystrokes ([`Session::input`]) and the agent's output
/// ([`Session::output`]), call [`Session::tick`] whenever the deadline it returns passes, and
/// write what it puts into `to_terminal` / `to_child` to the respective side. Status records
/// (OSC 7501, and the terminal title for agents whose own titles are dropped) are inserted into
/// `to_terminal` only between complete escape sequences.
pub struct Session {
    stream: OutputStream,
    reporter: Option<Reporter>,
    captured: Vec<Captured>,
    pending: Vec<u8>,
    dirty: bool,
    last_activity: Instant,
    last_output: Instant,
    next_tick: Instant,
}

impl Session {
    /// `child_pid` is the process running in the PTY; on-disk sessions written by it or its
    /// descendants are attributed to this session. `cols`/`rows` is the PTY size.
    pub fn new(target: &ResolvedTarget, child_pid: i32, cols: u16, rows: u16) -> Self {
        let titles = title_filter(target.agent);
        let now = Instant::now();
        Self {
            stream: OutputStream::new(titles),
            reporter: Some(Reporter::new(target, child_pid, cols, rows, titles == TitleFilter::Drop)),
            captured: Vec::new(),
            pending: Vec::new(),
            dirty: false,
            last_activity: now,
            last_output: now,
            next_tick: now,
        }
    }

    /// For an agent that reports OSC 7501 itself but runs where nothing answers its support probe
    /// (behind a terminal multiplexer): the probe is answered, every other byte is forwarded
    /// unchanged, and no records of its own are emitted.
    pub fn probe_only() -> Self {
        let now = Instant::now();
        Self {
            stream: OutputStream::probe_only(),
            reporter: None,
            captured: Vec::new(),
            pending: Vec::new(),
            dirty: false,
            last_activity: now,
            last_output: now,
            next_tick: now,
        }
    }

    /// The initial `idle` record (and title). Write it before any of the agent's output.
    pub fn start(&mut self, to_terminal: &mut Vec<u8>) {
        if let Some(reporter) = &mut self.reporter {
            reporter.initial(to_terminal);
        }
    }

    /// Bytes the user typed, before they are written to the agent.
    pub fn input(&mut self, data: &[u8]) {
        if let Some(reporter) = self.reporter.as_mut().filter(|_| is_submit(data)) {
            reporter.user_submitted();
        }
        self.last_activity = Instant::now();
    }

    /// Bytes read from the agent's PTY. The filtered output goes to `to_terminal`; replies the
    /// agent is waiting for (answers to its OSC 7501 support probe) go to `to_child`.
    pub fn output(&mut self, data: &[u8], to_terminal: &mut Vec<u8>, to_child: &mut Vec<u8>) {
        self.stream.process(data, to_terminal, &mut self.captured);
        let reset = self.stream.take_reset();
        if let Some(reporter) = &mut self.reporter {
            reporter.feed_output(data, reset);
        }
        for item in self.captured.drain(..) {
            match item {
                Captured::Probe(term) => {
                    to_child.extend_from_slice(b"\x1b]7501;?");
                    to_child.extend_from_slice(term.bytes());
                }
                Captured::Report(payload) => {
                    if let Some(reporter) = &mut self.reporter {
                        reporter.native_report(&payload);
                    }
                }
            }
        }
        let now = Instant::now();
        self.dirty = true;
        self.last_activity = now;
        self.last_output = now;
    }

    /// The PTY was resized.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if let Some(reporter) = &mut self.reporter {
            reporter.resize(cols, rows);
        }
    }

    /// Re-evaluates the agent's status and appends any changed record to `to_terminal`. Call it
    /// after [`Session::output`] and no later than the returned deadline.
    pub fn tick(&mut self, now: Instant, to_terminal: &mut Vec<u8>) -> Instant {
        if self.stream.stalled_in_osc_head() && now.duration_since(self.last_output) >= OSC_HEAD_STALL {
            self.stream.flush_stalled_osc(to_terminal);
        }
        if self.dirty || now >= self.next_tick {
            self.dirty = false;
            if let Some(reporter) = &mut self.reporter {
                reporter.evaluate(now, &mut self.pending);
            }
            let active = now.duration_since(self.last_activity) < ACTIVE_WINDOW
                || self.state() == OscState::Working;
            self.next_tick = now + if active { TICK_INTERVAL } else { IDLE_TICK_INTERVAL };
        }
        if !self.pending.is_empty() && self.stream.at_boundary() {
            to_terminal.append(&mut self.pending);
        }
        let deadline = if self.stream.stalled_in_osc_head() {
            self.next_tick.min(self.last_output + OSC_HEAD_STALL)
        } else {
            self.next_tick
        };
        deadline.max(now)
    }

    /// The state last reported to the terminal (`idle` for a [`Session::probe_only`] session).
    pub fn state(&self) -> OscState {
        self.reporter
            .as_ref()
            .map_or(OscState::Idle, Reporter::current_state)
    }

    /// The agent exited with `exit_code` (128 + signal for a signal). Appends the final records:
    /// `error` for an abnormal exit, otherwise a clear, and resets the title `w1` set.
    pub fn finish(mut self, exit_code: u8, to_terminal: &mut Vec<u8>) {
        // Whatever the agent left half-written is gone; reset the terminal's parser first.
        if !self.stream.at_boundary() {
            to_terminal.push(0x18);
        }
        to_terminal.append(&mut self.pending);
        if let Some(reporter) = self.reporter {
            reporter.finish(exit_code, to_terminal);
        }
    }
}

fn title_filter(agent: Option<AgentKind>) -> TitleFilter {
    match agent {
        Some(AgentKind::Omp) => TitleFilter::Omp,
        Some(AgentKind::Codex) => TitleFilter::Drop,
        Some(_) => TitleFilter::Despin,
        None => TitleFilter::Off,
    }
}

/// Enter in any keyboard encoding agents enable: plain CR/LF, kitty (`CSI 13 u`) or
/// xterm modifyOtherKeys (`CSI 27;…;13 ~`).
fn is_submit(input: &[u8]) -> bool {
    input.iter().any(|b| matches!(b, b'\r' | b'\n'))
        || contains(input, b"\x1b[13u")
        || contains(input, b"\x1b[13;")
        || contains(input, b";13~")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
