# w1 (Wrap7501)

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

`w1` forwards argv, the environment, the terminal size, signals, and the exit status unchanged.

## Install

```sh
cargo install w1
```

Requirements:

- macOS or another Unix. Only macOS has been tested.
- `sqlite3` on `PATH`. SQLite stores are read through `sqlite3 -readonly`.

## Usage

```
w1 [w1 options] <agent command> [agent args...]

  --list-agents   list recognised agents and how each is handled
  --force-wrap    wrap even agents that natively support OSC 7501
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

- **Passthrough**: the agent supports OSC 7501 fully. `w1` only prints a hint.
- **Merge**: the agent emits partial OSC 7501. `w1` wraps it, answers its probe, and fills in
  the rest.
- **Wrap**: the agent has no OSC 7501. Everything is derived by `w1`.

| Agent | Commands recognised | Mode | Session store read by `w1` | How the session is claimed | Turn end / error / cancel | Blocked (permission · question · auth) | Progress | Title | Verified with |
|---|---|---|---|---|---|---|---|---|---|
| Claude Code | `claude` | Passthrough (≥ 2.1.295) | — | — | native | native | native | native | real CLI |
| Pi | `pi` | Passthrough (≥ 1.1.0) | — | — | native | native | native | native | — |
| omp (oh-my-pi) | `omp` | Merge: native reports state only. It never sends `working`, and its OSC 0 title carries an ~80 ms spinner | `$PI_CODING_AGENT_DIR/sessions/<cwd-slug>/*.jsonl` (default `~/.omp/agent`) | cwd + submit time | assistant `stop`/`end_turn`, `error`, `aborted` | screen + native | `todo_hud_state` | `title` / `title_change`. Spinner and status glyph are stripped from the forwarded title | real CLI |
| OpenAI Codex | `codex`, `npx @openai/codex` | Wrap | `$CODEX_HOME/state_5.sqlite` `threads` → `sessions/**/rollout-*.jsonl` | Thread prompt time matches a submit in this terminal, or the first user message equals an argv element | `task_complete`, `error`, `turn_aborted` | screen (approval dialog shows `$ cmd`); `request_user_input` → question | `update_plan` | thread title | real CLI 0.162 |
| OpenCode | `opencode` | Wrap | `$XDG_DATA_HOME/opencode/opencode.db` (`session`/`message`/`part`/`todo`) | Root session created right after our submit; `--session` | Latest message `finish`; a rejected permission counts as idle | screen (`$ cmd`); `question` tool → question | `todo` table | session title | real CLI 1.17 |
| Kilo Code | `kilo` | Wrap | Same schema as OpenCode under `kilo/kilo.db` | as OpenCode | as OpenCode | as OpenCode | as OpenCode | as OpenCode | **not tested** (path assumed) |
| Gemini CLI | `gemini` | Wrap | `$GEMINI_CLI_HOME/.gemini/tmp/<project>/chats/session-*.jsonl` | `projectHash` + submit time | Screen only: the store has no turn-end record. Errors come from the store | screen | `write_todos` | `update_topic`, else first prompt | isolated HOME + mock API, 0.46 |
| Kimi Code | `kimi` | Wrap | `$KIMI_CODE_HOME/sessions/wd_*/session_*/` `state.json` + `agents/main/wire.jsonl` | cwd hash + submit time | `turn.ended` (`completed`/`cancelled`/error) | `interaction.request` (permission and question, including the confirm screen) | `tools.update_store` todo | `state.json` title | isolated HOME + mock, 2.1.1 |
| Qwen Code | `qwen` | Wrap | `~/.qwen/sessions/<pid>.json` → `projects/<slug>/chats/<id>.jsonl` (honours `QWEN_RUNTIME_DIR`, `advanced.runtimeOutputDir`) | pid registry | Assistant record without `functionCall`; `request_lifecycle` cancelled/error | screen; `ask_user_question` → question | `todo_write` | `custom_title` | scratch install + mock, 0.25 |
| GitHub Copilot CLI | `copilot` | Wrap | `$COPILOT_HOME/session-state/<id>/events.jsonl`, `workspace.yaml`, `session.db` | `inuse.<pid>.lock` | `assistant.turn_end` with no tool requests; `session.error`; `abort` → idle | `permission.requested`; `ask_user` → question; login screen → auth | `todos` table | `workspace.yaml` `name` | scratch install + BYOK mock, 1.0.97 |
| Crush | `crush` | Merge: native reports state/kind only, and errors as idle | `<data dir>/crush.db` (`-D`, else `projects.json`, else `.crush`) | Root session created after our submit; `--session` | Last `finish` part: `end_turn`/`canceled`/`error` | screen + native | session `todos` | session title | scratch install + mock, 0.98 |
| Factory Droid | `droid` | Merge: native reports state/kind only | `$FACTORY_HOME_OVERRIDE/.factory/sessions/<cwd-slug>/<id>.jsonl`, re-parsed whole because line 1 is rewritten | cwd + submit time; `--resume <id>` | `agent_turn_outcome` (`completed`, `cancelled`/`permission_rejected`, error) | screen + native; `AskUser` → question | `todo_state` | `session_start.title` | scratch install + mock, 0.240 |
| Devin CLI | `devin` | Wrap | `$XDG_DATA_HOME/devin/cli/` `session_locks/<id>.lock` + `sessions.db` | pid in the lock file | Head message `finish_reason=stop` | screen (trust, `$ cmd`) | `todo_write` calls | session title | real CLI 3000.11 |
| Antigravity CLI | `agy` | Wrap | `~/.gemini/antigravity-cli/` `log/cli-*.log` → `conversation_summaries.db` + `conversations/<id>.db` | pid in the log's first line | `status` leaves `*_RUNNING`; reply decoded from protobuf steps | screen (trust, "Requesting permission for:") | — | summary title | real CLI 1.3.4 |
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
  known agent, and repeated titles are not re-sent. State detection still sees the raw title.

## Project layout

| File | Role |
|---|---|
| `src/main.rs` | CLI parsing, `--list-agents` notes |
| `src/agents.rs` | `AgentKind`: command → agent identification, OSC `app` id, native support level |
| `src/pty.rs` | PTY spawn, I/O loop, signal and resize forwarding, passthrough mode, title filter selection |
| `src/stream.rs` | Splits the child's output: captures OSC 7501 reports and probes, filters titles |
| `src/screen.rs` | Virtual terminal (alacritty) and prompt marks |
| `src/detect.rs` | Per-agent screen rule manifests (TOML), message extraction, state stabiliser |
| `src/inspect/mod.rs` | `Tracker` trait, `LaunchContext` (cwd, launch time, submits, resume), session claims, shared helpers |
| `src/inspect/<agent>.rs` | One session-store tracker per agent |
| `src/report.rs` | Merges screen, store and native reports into the records sent to the terminal |
| `src/osc7501.rs` | OSC 7501 encoding, parsing, emitter |

## Contributing: adding a new agent

There are two levels of support. Screen rules alone give state and blocked kind. Add a tracker
when the agent writes a local session store, to get turn ends, summaries, titles, and progress.

### 1. Identify the agent (`src/agents.rs`)

- Add a variant to `AgentKind` and to `AgentKind::ALL`.
- Fill in `app_id()` (the OSC 7501 `app`, `[A-Za-z0-9_.+-]{1,32}`), `display_name()` and
  `manifest_id()`.
- Map its executable names in `lookup_agent_name()`. If it is often launched as an npm package
  or through `node`/`bunx`, also add the package path to `lookup_from_package_path()`.
- Set `native_support()`:
  - `Full`: the agent reports everything itself. It is passed through untouched.
  - `Partial`: it reports some OSC 7501. It is wrapped and merged.
  - `None`: it is wrapped.

  Use the **latest** release of the agent as the reference.

### 2. Screen rules (`src/detect.rs`)

- Write a `MANIFEST_<AGENT>` TOML string and register it in `BUNDLED_MANIFESTS` under the
  `manifest_id`.
- Each `[[rules]]` entry has:
  - `id`;
  - `state`: `idle`/`working`/`blocked`/`unknown`;
  - `priority`: the highest matching rule wins;
  - `region`, for example `bottom_non_empty_lines(16)`, `osc_title`, `above_prompt_box`;
  - matchers: `contains` (lower-cased), `regex`, `line_regex`, and nested `all`/`any`/`not` gates;
  - `visible_*` flags.
- Prefer stable affordances such as "esc to interrupt", the approval dialog's key hints, and
  login prompts. Avoid model output. Copy an existing manifest as a starting point (the Copilot
  and Crush ones are short).
- If the blocked kind cannot be inferred from the rule id (`permission`/`question`/`auth`),
  extend `classify_blocked_kind()`.
- If the agent's footer or status line leaks into summaries, add the pattern to
  `is_footer_hint_line()` or `ACTIVITY_NOISE`.

### 3. Session-store tracker (`src/inspect/<agent>.rs`, optional)

- Implement `Tracker::poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight)`. It
  is called every 500 ms. Throttle directory scans yourself (about 1 s), and read files
  incrementally with `JsonlTail` or SQLite through `sqlite_rows()`.
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
- Register the tracker in `SessionInspector::new()` (`src/inspect/mod.rs`), and add the agent's
  resume flags to `is_resume_invocation()`.
- Add a one-line note for the agent in `print_agents()` (`src/main.rs`), and a row to the table
  above.

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
