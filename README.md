# w1 (Wrap7501)

> **[Vibe42](https://vibe42.ai) — The AI remote workspace, always on.**
> Reach your home or office computer from your phone, tablet, or browser, and keep Claude Code,
> Codex, Gemini CLI, OpenCode and other agents working while you're away. Connections are
> end-to-end encrypted and need no VPS or port forwarding. Vibe42 shows each agent's status
> from OSC 7501 and pushes an alert when the agent finishes or needs you. `w1` makes agents
> report that status even when they don't speak OSC 7501 themselves.
> **[Try Vibe42 →](https://vibe42.ai)**

`w1` is a PTY wrapper that makes TUI coding agents report their progress over
**OSC 7501** (Program Status Protocol), even when the agent itself does not speak it.

```sh
w1 codex --yolo
w1 opencode
w1 gemini -m gemini-3-pro
```

For each wrapped agent, `w1` reports **state**, **blocked kind**, **progress**, **title**,
**app**, and a **summary** (`msg`) to the outer terminal. It does this without touching the
agent's config files and without installing hooks. The information comes from three sources:

1. **The screen.** `w1` renders the child's output into a virtual terminal
   (`alacritty_terminal`) and matches per-agent rules against it (status lines, approval dialogs,
   login prompts). It also uses the agent's own OSC 0/2 titles and OSC 9;4 progress.
2. **The session store.** It reads, read-only, the session files or databases the agent already
   writes (JSONL transcripts, SQLite stores). These give turn boundaries, replies, errors, todo
   progress, titles, and questions.
3. **Native OSC 7501.** Some agents emit partial OSC 7501 themselves. `w1` answers their
   `OSC 7501;?` probe and merges what they report with what it learns from the two sources
   above.

Agents that already fully support OSC 7501 (Claude Code, Pi) are not wrapped. `w1` prints a
hint, runs them directly, and prints the hint again when they exit.

Inside a terminal multiplexer, nothing may answer such an agent's `OSC 7501;?` support probe.
The multiplexer swallows it, so the agent concludes the terminal has no OSC 7501 and stays
silent. When `w1` detects a multiplexer, it runs the agent in a PTY and answers the probe
itself. Every other byte, including the agent's own OSC 7501 records, is forwarded unchanged.

| Multiplexer | Detected by |
|---|---|
| Herdr | `HERDR_ENV=1` |
| tmux | `TMUX`, or `TERM=tmux*` |
| Zellij | `ZELLIJ` |
| GNU Screen | `STY`, or `TERM=screen*` |

To turn this off and launch the agent directly, pass `--no-probe-answer` or set
`W1_NO_PROBE_ANSWER=1`. Any value other than empty, `0` or `false` turns it off. The switch
only affects these fully supporting agents. Agents that `w1` wraps always have their probe
answered by `w1`, because `w1` is their reporter.

`w1` forwards argv, the environment, the terminal size, signals, and the exit status unchanged.

## Install

### Requirements

- macOS or Linux (glibc or musl). Tested on macOS, and on x86_64 Ubuntu 22.04/24.04,
  Debian 12/13, Fedora 42, Rocky 9, Arch, openSUSE Leap 15.6 and Alpine 3.22.
- Rust 1.85 or newer. If you don't have it, install it with [rustup](https://rustup.rs):

  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

- `sqlite3` on `PATH`. `w1` reads agents' SQLite stores through `sqlite3 -readonly`.
  - macOS ships it.
  - Debian/Ubuntu: `sudo apt install sqlite3`.
  - Fedora: `sudo dnf install sqlite`.

### From crates.io

```sh
cargo install w1
```

This puts the `w1` binary in `~/.cargo/bin`, which must be on your `PATH` (rustup adds it
by default). Check the install:

```sh
w1 --version
w1 --list-agents
```

### From source

```sh
cargo install --git https://github.com/krhougs/w1
# or, from a local checkout
git clone https://github.com/krhougs/w1 && cd w1 && cargo install --path .
```

### Upgrade and uninstall

```sh
cargo install w1      # installs the latest release over the old one
cargo uninstall w1
```

`w1` keeps no state of its own. The only thing it writes is session claim locks, in
`$TMPDIR/wrap7501-<uid>/`, and it is safe to delete that directory.

## Usage

```
w1 [w1 options] <agent command> [agent args...]

  --list-agents       list recognised agents and how each is handled
  --force-wrap        wrap even agents that natively support OSC 7501
  --no-probe-answer   inside a multiplexer, do not answer the OSC 7501 probe of agents that
                      natively support OSC 7501 (same as W1_NO_PROBE_ANSWER=1)
  -h, --help
  -V, --version
```

If stdin is not a TTY, `w1` runs the command directly without a PTY.

### Several instances in one directory

Several `w1` instances can run the same agent in the same directory. Each one claims a session
only after it is tied to that instance, by one of:

- the agent's pid (lock or registry files);
- a session that appeared right after the user submitted a prompt in this terminal;
- an explicit `--resume <id>`.

Claims are `flock`ed files in `$TMPDIR/wrap7501-<uid>/`, so two instances never adopt the same
session.

## Agent behaviour

Mode:

- **Passthrough**: the agent supports OSC 7501 fully. `w1` only prints a hint. Inside a
  multiplexer, `w1` also answers the agent's probe (see above).
- **Merge**: the agent emits partial OSC 7501. `w1` wraps it, answers its probe, and fills in
  the rest.
- **Wrap**: the agent has no OSC 7501. Everything is derived by `w1`.

| Agent | Commands recognised | Mode | Session store read by `w1` | How the session is claimed | Turn end / error / cancel | Blocked (permission · question · auth) | Progress | Title | Verified with |
|---|---|---|---|---|---|---|---|---|---|
| Claude Code | `claude` | Passthrough (≥ 2.1.295) | — | — | native | native | native | native | real CLI |
| Pi | `pi` | Passthrough (≥ 1.1.0) | — | — | native | native | native | native | — |
| omp (oh-my-pi) | `omp` | Merge: native reports state only. It never sends `working`, and its OSC 0 title carries an ~80 ms spinner | `$PI_CODING_AGENT_DIR/sessions/<cwd-slug>/*.jsonl` (default `~/.omp/agent`) | cwd + submit time | assistant `stop`/`end_turn`, `error`, `aborted` | screen + native | `todo_hud_state` | `title` / `title_change`. Spinner and status glyph are stripped from the forwarded title | real CLI |
| OpenAI Codex | `codex`, `npx @openai/codex` | Merge (≥ 0.163): native reports `idle`/`working`/`blocked` only, and `idle` where a turn finished, which counts as `done` when `w1` saw the turn end | `$CODEX_HOME/state_5.sqlite` `threads` → `sessions/**/rollout-*.jsonl` | Thread prompt time matches a submit in this terminal, or the first user message equals an argv element | `task_complete`, `error`, `turn_aborted` | screen (approval dialog shows `$ cmd`) + native; `request_user_input` → question | `update_plan` | Thread `name`, else first prompt; `w1 - OpenAI Codex CLI` before the session exists. Codex's own OSC 0/2 title (just the directory name) is replaced by this one | real CLI 0.163 |
| OpenCode | `opencode` | Wrap | `$XDG_DATA_HOME/opencode/opencode.db` (`session`/`message`/`part`/`todo`) | Root session created right after our submit; `--session` | Latest message `finish`; a rejected permission counts as idle | screen (`$ cmd`); `question` tool → question | `todo` table | session title | real CLI 1.17 |
| Kilo Code | `kilo` | Wrap | Same schema as OpenCode under `kilo/kilo.db` | as OpenCode | as OpenCode | as OpenCode | as OpenCode | as OpenCode | scratch install + mock, 7.8.12 |
| Gemini CLI | `gemini` | Wrap | `$GEMINI_CLI_HOME/.gemini/tmp/<project>/chats/session-*.jsonl` | `projectHash` + submit time | Screen only: the store has no turn-end record. Errors come from the store | screen | `write_todos` | `update_topic`, else first prompt | isolated HOME + mock API, 0.46 |
| Kimi Code | `kimi` | Wrap | `$KIMI_CODE_HOME/sessions/wd_*/session_*/` `state.json` + `agents/main/wire.jsonl` | cwd hash + submit time | `turn.ended` (`completed`/`cancelled`/error) | `interaction.request` (permission and question, including the confirm screen) | `tools.update_store` todo | `state.json` title | isolated HOME + mock, 2.1.1 |
| Qwen Code | `qwen` | Wrap | `~/.qwen/sessions/<pid>.json` → `projects/<slug>/chats/<id>.jsonl` (honours `QWEN_RUNTIME_DIR`, `advanced.runtimeOutputDir`) | pid registry | Assistant record without `functionCall`; `request_lifecycle` cancelled/error | screen; `ask_user_question` → question | `todo_write` | `custom_title` | scratch install + mock, 0.25 |
| GitHub Copilot CLI | `copilot` | Wrap | `$COPILOT_HOME/session-state/<id>/events.jsonl`, `workspace.yaml`, `session.db` | `inuse.<pid>.lock` | `assistant.turn_end` with no tool requests; `session.error`; `abort` → idle | `permission.requested`; `ask_user` → question; login screen → auth | `todos` table | `workspace.yaml` `name` | scratch install + BYOK mock, 1.0.97 |
| Crush | `crush` | Merge: native reports state/kind only, and errors as idle | `<data dir>/crush.db` (`-D`, else `projects.json`, else `.crush`) | Root session created after our submit; `--session` | Last `finish` part: `end_turn`/`canceled`/`error` | screen + native | session `todos` | session title | scratch install + mock, 0.98 |
| Factory Droid | `droid` | Merge: native reports state/kind only | `$FACTORY_HOME_OVERRIDE/.factory/sessions/<cwd-slug>/<id>.jsonl`, re-parsed whole because line 1 is rewritten | cwd + submit time; `--resume <id>` | `agent_turn_outcome` (`completed`, `cancelled`/`permission_rejected`, error) | screen + native; `AskUser` → question | `todo_state` | `session_start.title` | scratch install + mock, 0.240 |
| Devin CLI | `devin` | Wrap | `$XDG_DATA_HOME/devin/cli/` `session_locks/<id>.lock` + `sessions.db` | pid in the lock file | Head message `finish_reason=stop` | screen (trust, `$ cmd`) | `todo_write` calls | session title | real CLI 3000.11 |
| Antigravity CLI | `agy` | Merge (≥ 1.4.0): native reports no title, reports `blocked` with no dialog on screen (`auth` while signing in at startup; `Approve plan` from when it writes an artifact until the next prompt, even after the turn ends), and a `done` for a declined permission that it replaces with `idle` a frame later. A native `blocked` counts only while the dialog is on screen, and a native record only after standing 300 ms | `~/.gemini/antigravity-cli/` `log/cli-*.log` → `conversation_summaries.db` + `conversations/<id>.db` | pid in the log's first line | native (cancel → idle); `status` leaves `*_RUNNING`; reply decoded from protobuf steps | screen (trust, sign-in, "Requesting permission for:") + native | — | summary title | real CLI 1.4.2 |
| Cursor Agent | `cursor-agent`, `agent` | Wrap | `~/.cursor/chats/<md5(cwd)>/<id>/meta.json` + `projects/<slug>/agent-transcripts/` | cwd + submit time | `turn_ended` `success`/`error`/`aborted` | screen; login → auth | todo items | `meta.json` | login screen only |
| Grok Build | `grok` | Merge (≥ 1.0.52; silent inside tmux) | `$GROK_HOME/active_sessions.json` → `sessions/<cwd>/<id>/` `events.jsonl`, `updates.jsonl`, `summary.json` | pid in `active_sessions.json` | `turn_ended` / `turn_completed`; cancelled → idle | `permission_requested`; login → auth | — | `summary.json` | login screen only |
| Sourcegraph Amp | `amp` | Merge: native reports state/kind only | none (threads are server-side) | — | screen + native | screen (approval, login → auth) | — | OSC title | login screen only |
| Cline, Kiro, Hermes, Qoder, Letta, Maki, Muse, Aider, Goose, ZCode, Mastra Code | see `--list-agents` | Wrap | none | — | screen | screen | OSC 9;4 | OSC title | rules only |
| Any other program | — | Wrap with generic rules, or Passthrough if its binary contains an OSC 7501 marker | none | — | screen | screen | OSC 9;4 | OSC title | — |

Behaviour that applies to every agent:

- **Native merge.** A known agent whose binary contains an OSC 7501 marker is treated as
  Merge, never Passthrough. Agents adding OSC 7501 so far report less than `w1` can add.
- **Done vs. cancelled.** A screen-only "done" is held for up to 2 s until the store confirms
  it. This keeps an interrupt or a rejected approval from flashing `done`, which would trigger a
  "finished" notification.
- **Stale turns.** A `done`/`idle`/`error` read from the store is applied once per turn. It
  cannot override a new turn the user has just started.
- **OSC 9;4.** State `3` (indeterminate) means working. It does not mean blocked.
- **Title filtering.** Braille spinner frames are stripped from the OSC 0/1/2 titles of every
  known agent, and repeated titles are not re-sent. Codex's OSC 0/1/2 titles are dropped
  and `w1` sets the terminal title (OSC 0) to the same title it reports over OSC 7501:
  `w1 - <agent name>` until a session exists, then the session title, cleared on exit. State detection still sees the raw title.

## Using w1 as a library

The crate is also a library (`w1` on crates.io), for hosts that already run agents in their
own PTYs, such as terminals, multiplexers or relays, and want the same status reporting without
spawning the `w1` binary.

```toml
[dependencies]
w1 = "0.4"
```

- `ResolvedTarget::resolve(program, args)` identifies the agent from its command line.
- `Session` is the whole engine, without any file descriptors:
  - `start` emits the initial record;
  - `input` takes the user's keystrokes (to detect submits);
  - `output` takes the agent's output. It returns the filtered bytes for the terminal and the
    replies for the agent (answers to its OSC 7501 probe);
  - `tick` re-evaluates the status and returns its next deadline;
  - `resize` follows PTY size changes;
  - `finish` emits the final records.

  Status records are only inserted between complete escape sequences.

  `Session::probe_only()` is for an agent that fully supports OSC 7501 but sits behind
  something that cannot answer its probe. It answers the probe, forwards every other byte
  unchanged, and emits no records of its own.
- `run_target(target, RunOptions)` runs the full `w1` behaviour (passthrough, probe answering,
  or PTY wrapping) on the current process's terminal. `RunOptions::from_env()` applies
  `W1_NO_PROBE_ANSWER`. `detect_multiplexer()` reports the multiplexer `w1` detected.
- `osc7501` encodes and parses OSC 7501 records. `AgentKind::ALL` lists the recognised agents
  with their `app_id()`, `display_name()`, `native_support()` and `summary()`. Since 0.4 the
  lower layers (screen rules, session-store trackers) are internal.

```rust
use std::time::Instant;
use w1::{ResolvedTarget, Session};

let target = ResolvedTarget::resolve("codex".into(), vec!["--yolo".into()]);
let mut session = Session::new(&target, child_pid, cols, rows);
let (mut to_terminal, mut to_child) = (Vec::new(), Vec::new());
session.start(&mut to_terminal);
// I/O loop:
session.input(&keys);                                          // then write keys to the PTY
session.output(&pty_bytes, &mut to_terminal, &mut to_child);   // write both buffers out
let deadline = session.tick(Instant::now(), &mut to_terminal); // call again by `deadline`
// On exit:
session.finish(exit_code, &mut to_terminal);
```

## Project layout

```
src/
├── main.rs              CLI, --list-agents
├── lib.rs               library root and re-exports
├── session.rs           Session: the embeddable engine between an agent's PTY and the terminal
├── pty.rs               the w1 PTY: spawn, I/O loop, signal and resize forwarding, passthrough
├── stream.rs            splits the child's output: OSC 7501 reports and probes, title filtering
├── osc7501.rs           OSC 7501 encoding, parsing, emitter
├── procs.rs             process tree
├── engine/              status derivation; nothing here names an agent
│   ├── rules.rs         screen rule DSL: parsing, matching, blocked kind
│   ├── regions.rs       the screen regions a rule can match against
│   ├── message.rs       summaries (msg) read from the screen
│   ├── stabilizer.rs    screen detections → states without flicker
│   ├── screen.rs        virtual terminal (alacritty) and prompt marks
│   ├── store.rs         what trackers share: Tracker, LaunchContext, ExternalInsight, claims, helpers
│   └── report.rs        merges screen, store and native reports into the records sent out
└── agents/
    ├── mod.rs           AgentKind, the profile registry, command → agent identification
    ├── profile.rs       AgentProfile: everything an agent can declare
    ├── generic.toml     screen rules for unknown programs
    └── <agent>/
        ├── mod.rs       the agent's PROFILE and hooks
        ├── rules.toml   its screen rules
        └── tracker.rs   its session-store tracker (when it has one)
```

Everything `w1` knows about one agent lives in its directory. The engine reads agent
differences only from the agent's `AgentProfile`.

## Contributing: adding a new agent

There are two levels of support. Screen rules alone give state and blocked kind. Add a tracker
when the agent writes a local session store, to get turn ends, summaries, titles, and progress.

### 1. Profile (`src/agents/<agent>/mod.rs`)

- Create `src/agents/<agent>/` and declare `pub(crate) const PROFILE: AgentProfile`. Copy an
  existing one: `kiro` has screen rules only, `copilot` also has a tracker.
- Set `app_id` (the OSC 7501 `app`, `[A-Za-z0-9_.+-]{1,32}`), `display_name`, `commands` (its
  executable names) and `rules`. Everything else defaults through `..AgentProfile::DEFAULTS`;
  `src/agents/profile.rs` documents each field:
  - `install_paths`, if it is often launched as an npm package or through `node`/`bunx`;
  - `native`, using the **latest** release as the reference:
    - `Full`: the agent reports everything itself. It is passed through untouched.
    - `Partial`: it reports some OSC 7501. It is wrapped and merged.
    - `None`: it is wrapped.
  - `native_quirks`, when its own reports cannot be taken at face value;
  - `titles`, `resume`, `tracker`, `hooks`;
  - `summary`, its line in `--list-agents`.
- Register it in `src/agents/mod.rs`: add a variant to `AgentKind` and to `AgentKind::ALL`, a
  `mod <agent>;`, and a line in `AgentKind::profile()`. That match has no fallback, so a missing
  line does not compile.

### 2. Screen rules (`src/agents/<agent>/rules.toml`)

- Each `[[rules]]` entry has:
  - `id`;
  - `state`: `idle`/`working`/`blocked`/`unknown`;
  - `priority`: the highest matching rule wins;
  - `region`, for example `bottom_non_empty_lines(16)`, `osc_title`, `above_prompt_box`
    (`src/engine/regions.rs` lists them all);
  - matchers: `contains` (lower-cased), `regex`, `line_regex`, and nested `all`/`any`/`not` gates;
  - `visible_*` flags.
- Prefer stable affordances such as "esc to interrupt", the approval dialog's key hints, and
  login prompts. Avoid model output. Copy an existing `rules.toml` as a starting point (the
  Copilot and Crush ones are short). `cargo test` checks that every rules file compiles.
- The blocked kind comes from the rule id (`permission`/`question`/`auth`, `login`, `trust`, …).
  If it cannot, extend `classify_blocked_kind()` in `src/engine/rules.rs`.
- Summaries (`msg`) come from shared heuristics in `src/engine/message.rs`. If the agent's
  footer or status line leaks into them, add the pattern to `is_footer_hint_line()` or
  `ACTIVITY_NOISE`. A summary only this agent's screen needs goes into `hooks` in its
  `mod.rs` instead.

### 3. Session-store tracker (`src/agents/<agent>/tracker.rs`, optional)

- Implement `Tracker::poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight)` (from
  `crate::engine::store`). It is called every 500 ms. Throttle directory scans yourself (about
  1 s), and read files incrementally with `JsonlTail` or SQLite through `sqlite_rows()`.
- **Claim the right session.** Prefer, in this order:
  1. A pid link: a lock or registry file holding a pid equal to, or descended from,
     `ctx.child_pid` (`procs::is_descendant_of`).
  2. An explicit resume id (`ctx.arg_value`).
  3. A session created right after a prompt typed here (`ctx.created_by_our_submit`,
     `ctx.prompted_here`) in a directory matching `ctx.matches_cwd`.

  Wrap the choice in `claim_first()` so concurrent `w1` instances never share a session. Only
  switch to a newer session (e.g. after `/new`) when it also follows a submit made here.
- **Ignore history.** Records older than `ctx.min_created_ms()` belong to a resumed session.
  They may supply the title but must not drive the state.
- Update `insight` with the helpers:
  - `begin_turn()`;
  - `finish_turn(reply)`;
  - `fail_turn(error)`;
  - `abort_turn()` (cancel or reject → `idle`);
  - `blocked_kind`/`blocked_msg` for permissions and questions;
  - `progress` from todos (`todo_progress()`);
  - `title`.

  Pass all text through `clean_text_summary()`.
- In the profile, set `tracker: Some(|| Box::new(tracker::<Agent>Tracker::default()))` and the
  agent's `resume` flags.
- Add a row to the table above.

### 4. Rules

- Never write to the agent's config, install hooks, or modify its store. Open SQLite read-only.
- No `unwrap`/`expect`/`panic!`/indexing that can panic outside tests (see `AGENTS.md`).
  Degrade to `None` instead.
- Do not add permanent tests for screen rules or store parsing. Verify by driving the real agent
  (or the agent against a mock API in an isolated `HOME`), and keep throwaway scripts out of
  the repo.
- Before sending a PR, these must pass:

  ```sh
  cargo clippy --all-targets -- -D warnings
  cargo test
  ```

  Report how you verified the change: which agent version, and whether you used a real account
  or a mock. Cover each of permission, question, done, error, and cancel.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
