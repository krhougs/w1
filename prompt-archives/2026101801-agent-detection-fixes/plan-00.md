# Plan 00：修复交付实测发现的检测问题

## 背景

`982e68b`（0.4.0，按 agent 聚合）交付前的实测发现一批与重构无关的既有问题，清单见 `2026101800-agent-profile-layout/plan-00-result.md` 的“后续事项”。本计划逐项给出根因与最小修复。根因均已用直接运行 agent 的录制回放或带临时调试日志的真机运行确认。

## 根因与修复

| 问题 | 根因 | 修复 |
| --- | --- | --- |
| Codex 回合结束报 idle | Codex 0.163 起自己发 OSC 7501（`idle`/`working`/`blocked`，无 `done`，无探测），回合结束报 `idle`；w1 合并时原生静止态覆盖了 w1 的 `done` | profile 声明 `Partial`；新增 `NativeQuirks::idle_for_done`，Codex 的原生 `idle` 不再覆盖 w1 判出的 `done` |
| Codex 启动约 2 s working | Codex 启动时自己原生报 `working`（加载 MCP 等），属 Codex 自身的状态 | 不改，按原生上报 |
| Crush 拒绝权限后卡在 working | 拒绝后会话库最新一条是 `tool` 结果 `User denied permission`（Crush 常量），tracker 视为仍在运行，盖过原生 `done` | tracker 将其视为回合中止（idle）；会话库明确报中止时，其 `idle` 优先于原生 `done`（与“拒绝不闪 done”的既有约定一致） |
| Crush 权限框只能靠原生 | 对话框标题与按钮行在 `bottom_non_empty_lines(12)` 之外 | 增加按整屏匹配“Allow for Session + Deny + 操作提示”的规则 |
| Kimi 重复错误报 done | tracker 的回合开始与报错在同一次轮询内完成，reporter 只看到 `Error → Error`，不认为有新结果 | `ExternalInsight` 增加回合计数（`begin_turn` 递增），reporter 以“状态 + 回合”判断变化 |
| Kimi / Gemini / Qoder 信任框，Gemini / Qoder 认证与等待认证，Droid / Qoder / Kiro 登录页，Kilo 拒绝原因输入框，ZCode 权限框 | 规则未覆盖这些界面；ZCode 已有的 `permission_blocked` 优先级低于标题 spinner，且该行已在底部 10 行之外 | 各自 `rules.toml` 增加规则；ZCode 权限规则优先级提到标题规则之上 |
| Hermes 流式输出时 done/working 跳变 | prompt_toolkit 每次重绘先擦除提示区、请求光标位置后再画回，读块拆分使连续三次判定都看不到提示行，稳定器提前结束回合 | 底部看不到 `❯` 提示行时视为重绘空档，不更新状态 |
| Aider 快速回复/流式输出无 working | 流式期间屏幕没有任何 working 特征 | 规则 DSL 增加 `turn_only`（仅在用户提交后的回合内生效）；Aider 增加“底部不是提示行即 working”的 `turn_only` 规则 |
| `node …/@scope/pkg/x.js` 不识别 | `identify_token` 先按 `@` 截版本再取文件名，作用域目录把路径截断 | 先取文件名再去掉 `@版本` |

## 约束

- 规则改动只针对实测见到的界面；不新增规则样例测试。识别问题是真实回归，在已有识别测试中补一条。
- 不改变 `21fbeca`/`982e68b` 已验证的其它 agent 行为：以修复前代码为基线，用全部录制回放对比，只允许预期变化。

## 验证

- `cargo clippy --all-targets -- -D warnings`、`cargo test`、`cargo doc`。
- 回放（`build/` 临时工具，交付前删除）：问题录制在新代码下得到预期状态；其余全部录制与修复前逐条对比，差异只来自本次修复。
- 真机复跑：Codex（真实账号）、Crush/Kilo/ZCode/Hermes/Aider/Gemini/Letta（mock）、Kimi（失效账号）、Droid/Qoder/Kiro（登录页）。
