//! Wrap7501: makes TUI coding agents report their status over OSC 7501 (Program Status Protocol).
//!
//! The `w1` binary is a thin CLI over this crate. Hosts that already run agents in their own
//! PTYs can embed the same engine:
//!
//! - [`ResolvedTarget::resolve`] identifies the agent from its command line.
//! - [`Session`] sits between the agent's PTY and the terminal. It filters the agent's output,
//!   answers its OSC 7501 probe, reads its session store, and emits OSC 7501 records.
//! - [`run_target`] is the whole `w1` behaviour (passthrough, answering the probe behind a
//!   multiplexer, or PTY wrapping) for the current process's terminal.
//! - [`osc7501`] encodes and parses the protocol itself.
//!
//! ```no_run
//! use std::time::Instant;
//! use w1::{ResolvedTarget, Session};
//!
//! let target = ResolvedTarget::resolve("codex".into(), vec!["--yolo".into()]);
//! let child_pid = 4242; // the process the host spawned in the PTY
//! let mut session = Session::new(&target, child_pid, 120, 40);
//! let (mut to_terminal, mut to_child) = (Vec::new(), Vec::new());
//! session.start(&mut to_terminal);
//!
//! // In the host's I/O loop:
//! session.input(b"fix the tests\r");
//! session.output(b"...agent output...", &mut to_terminal, &mut to_child);
//! let next_deadline = session.tick(Instant::now(), &mut to_terminal);
//!
//! // When the agent exits:
//! session.finish(0, &mut to_terminal);
//! # let _ = next_deadline;
//! ```

mod agents;
mod engine;
pub mod osc7501;
mod procs;
mod pty;
mod session;
mod stream;

pub use agents::{AgentKind, NativeSupport, ResolvedTarget};
pub use osc7501::{BlockedKind, OscRecord, OscState};
pub use pty::{detect_multiplexer, run_target, RunOptions, NO_PROBE_ANSWER_ENV};
pub use session::Session;
