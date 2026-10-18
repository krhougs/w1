# Plan 00：按 agent 聚合适配代码（AgentProfile + 目录重组）

## 背景

截至 `21fbeca`（agy 1.4 原生 OSC 7501 适配），代码按机制分层，每层各自维护一张按 agent 分支的表。一个 agent 的知识散落在：

- `src/agents.rs`：`AgentKind` 枚举、`ALL`、`app_id`/`display_name`/`manifest_id`/`native_support` 四张 match、命令名与安装路径识别；
- `src/detect.rs`（3054 行）：规则引擎约 1070 行，29 份 TOML 规则以字符串常量内嵌约 1820 行，经 `BUNDLED_MANIFESTS` 用字符串 id 关联；通用摘要提取中夹有 agy（`Requesting permission for:`、`login_prompt`）与 Gemini（OSC 标题）特判；
- `src/inspect/mod.rs`：`is_resume_invocation`、tracker 工厂两张 match；tracker 本体在 `src/inspect/<agent>.rs`；
- `src/session.rs`：`title_filter()`（omp、Codex）；
- `src/report.rs`：agy 的稳定窗口与 blocked 门控（`agent == Antigravity`）；
- `src/main.rs`：`--list-agents` 说明表；README 行为表。

多数 match 带 `_ =>` 兜底，漏改不报错。

## 目标

引擎按机制分、agent 知识按 agent 聚合；`agents/` 之外不再出现 `AgentKind::X` 分支。纯搬迁，不改变任何 agent 的行为。

```
src/
├── lib.rs  main.rs  osc7501.rs  procs.rs
├── pty.rs  session.rs  stream.rs          运行时
├── engine/
│   ├── rules.rs       规则 DSL：解析、编译、匹配、detect、blocked kind 分类
│   ├── regions.rs     规则 region
│   ├── message.rs     屏幕摘要的通用启发式
│   ├── stabilizer.rs
│   ├── screen.rs      虚拟终端
│   ├── report.rs      三路合并，差异只读 profile
│   └── store.rs       Tracker trait、LaunchContext、ExternalInsight、claim、sqlite/jsonl 工具
└── agents/
    ├── mod.rs         AgentKind、ALL、profile() 注册、ResolvedTarget 与识别
    ├── profile.rs     AgentProfile 及其字段类型
    ├── generic.toml
    └── <agent>/       mod.rs（PROFILE + 钩子）、rules.toml、tracker.rs（有会话库时）
```

`AgentProfile` 字段：`app_id`、`display_name`、`commands`、`versioned_command`（muse-bin-<版本>）、`install_paths`、`native`、`native_quirks`（`settle`、`blocked_needs_screen`）、`rules`（`include_str!`）、`titles`、`resume`（flags、subcommands）、`tracker`、`hooks`（`blocked_detail`、`working_detail`）、`summary`（`--list-agents`）。可选字段经 `AgentProfile::DEFAULTS` 补齐。`AgentKind::profile()` 是唯一按 agent 的 match，无 `_` 兜底。

## 决策

- 库 API 破坏性变更，版本 0.4.0；不保留 `w1::detect`/`inspect`/`agents`/`report`/`screen`/`stream` 等旧路径，顶层只导出 `AgentKind`、`NativeSupport`、`ResolvedTarget`、`Session`、`run_target`、`RunOptions`、`detect_multiplexer`、`NO_PROBE_ANSWER_ENV`、`osc7501` 及其类型。`AgentKind` 新增 `summary()` 供 `--list-agents` 使用，移除 `manifest_id()`。
- Kilo 复用 OpenCode 的 tracker；Mastra Code 使用 `generic.toml`。
- agy 的 `Requesting permission for:` 提取原本对所有 agent 生效，只有 agy 的屏幕会出现，移入 agy 的 `blocked_detail` 钩子。
- TOML 规则逐字搬迁，仅删除不再使用的 `id = "..."` 行。
- 已有永久测试原样保留（规则可编译、各 agent 识别与检测样例），不新增永久测试。
- `prompt-archives/` 从 crate 打包中排除。

## 阶段

1. `agents/` 目录与 `AgentProfile`：迁移识别信息、native support、标题过滤、续接参数、说明；
2. 规则拆到 `agents/<agent>/rules.toml`，引擎按 `AgentKind` 编译；
3. tracker 迁到 `agents/<agent>/tracker.rs`，`inspect/mod.rs` 迁为 `engine/store.rs`；
4. 原生上报特殊处理与摘要钩子进 profile，`report.rs`、`detect.rs` 中的 agent 分支清零；
5. 拆分 `detect.rs` 为 `engine/{rules,regions,message,stabilizer}.rs`，`report.rs`、`screen.rs` 迁入 `engine/`；
6. 0.4.0、lib 文档、README（Project layout、Contributing、库用法）。

## 验证

- 临时等价性对比（`build/` 下的临时 crate，交付前删除）：以 `build/old`（`21fbeca`）为基线，经公开的 `Session` API，对全部 28 个 agent 喂入同一批屏幕与字节流（每条规则合成的屏幕、真实 agent 录制的 PTY 输出、标题与原生 OSC 7501 序列），逐字节比较输出；比较命令识别结果。
- 新旧二进制 `--list-agents` 输出逐字节一致。
- `cargo clippy --all-targets -- -D warnings`、`cargo test`。
- 交付前测试所有已支持的 agent：本机已安装的（agy、codex、opencode、gemini、omp、kimi、devin、grok、cursor-agent、claude）用真实 CLI 跑通启动、完成、权限/提问、取消、退出；未安装的尽量临时安装到系统临时目录后实测，无法实测的如实说明。
