# Plan 00 执行结果

## 交付

按 plan-00 完成目录与架构重组，纯搬迁，行为不变：

- `src/agents/profile.rs`：`AgentProfile`、`NativeQuirks`、`Resume`、`Hooks`；`AgentKind::profile()` 是唯一按 agent 的 match，无 `_` 兜底。
- `src/agents/<agent>/`：28 个 agent 各自的 `mod.rs`（PROFILE 与钩子）、`rules.toml`（27 份，Mastra Code 用 `generic.toml`）、`tracker.rs`（13 个，Kilo 复用 OpenCode）。
- `src/engine/`：`rules`、`regions`、`message`、`stabilizer`、`screen`、`report`、`store`。`agents/` 之外不再出现按 `AgentKind::X` 的分支（测试与 `AgentKind::ALL` 遍历除外）。
- agy 的 settle 和 blocked 门控改由 `native_quirks` 声明；`Requesting permission for:`/登录提示的摘要移入 agy 的 `blocked_detail`，Gemini 的 OSC 标题活动移入 `working_detail`。
- 0.4.0：库 API 只导出顶层类型（见 plan-00 决策），`manifest_id()` 移除，新增 `summary()`；README 更新 Project layout、Contributing 和库用法。

## 验证

- `cargo clippy --all-targets -- -D warnings`、`cargo test`（7 + 1）、`RUSTDOCFLAGS='-D warnings' cargo doc` 通过；新旧 `--list-agents` 逐字节一致；`cargo package --list` 不含 `prompt-archives/`。
- 等价性（`build/` 临时 crate，已删除）：以 `21fbeca` 为基线，经公开 `Session` API 对 28 个 agent 和未知程序逐字节比较。输入为规则合成屏幕、标题/原生 OSC 7501 序列，以及本次实测录下的 64 份真实 PTY 输出（共 2507 步），0 差异；654 条命令行的识别结果 0 差异；13 个 tracker 源码除 import 外未改。
- 实测（w1 + 真实 CLI，120x40 PTY，按“启动 → 回复 → 权限/提问 → 拒绝 → 取消 → 退出”跑）。未登录或无账号的 CLI 用隔离 HOME 加本地 OpenAI/Anthropic/Gemini 兼容 mock 驱动。发现差异时用旧二进制在相同条件下复跑对照，均与旧版一致：

| agent | 版本 | 方式 | 结果 |
| --- | --- | --- | --- |
| agy | 1.4.2 | 真实账号 | 全流程正常（working/done、blocked 带命令、拒绝/取消回 idle） |
| Codex | 0.163.0 | 真实账号（`-m gpt-6-luna`） | working、blocked/permission 带命令、拒绝、取消、退出正常；更新提示与信任框报 blocked；回合结束报 idle 而非 done（旧版同） |
| OpenCode | 1.17.18 | mock；真实账号 401 报 error | 全流程正常 |
| Kilo | 7.8.12 | mock | 回复、blocked 正常；Esc 后的“Reject permission”输入框报 working（旧版同） |
| Gemini CLI | 0.46.0 | mock；真实账号未登录 | mock 下全流程正常；未登录时信任框、认证框报 idle（规则未覆盖） |
| Qwen Code | 0.25.2 | mock | 全流程正常 |
| Crush | 0.98.1 | mock | 回复、blocked 正常；拒绝权限后一直报 working（旧版同）；极快回合偶尔跳过 working（新旧多次交替复跑分布一致） |
| Copilot CLI | 1.0.98 | mock（BYOK 离线） | 回复带 tracker 标题、退出正常；echo 未触发审批 |
| Cline | 3.0.73 | mock | 回复、取消、退出正常；未触发审批 |
| Goose | 1.55.0 | mock | 回复、blocked、退出正常；blocked 摘要偏弱（“Allow (Allow the tool call once)”） |
| Hermes | 0.19.0 | mock | 回复正常；流式输出期间 done/working 反复跳变（旧版同） |
| Aider | 0.86.2 | mock | 回复太快、流式输出期间无 working 特征，全程 idle（规则未覆盖） |
| Mastra Code | 0.47.1 | mock（generic 规则） | 第二轮起正常；首个极短回合漏报 |
| Letta Code | 0.34.16 | mock（local backend） | 回复、取消正常；`node letta.js` 直接启动时识别为 node（旧版同），经 `letta` 入口正常 |
| ZCode | 3.15.3 | mock | 回复、取消正常；权限框报 working（规则未覆盖） |
| Pi | 1.3.0 | mock | 原生支持，w1 直接透传，Pi 自身 OSC 7501 正常 |
| Claude Code | 2.1.302 | 真实 | 原生支持，w1 直接透传（未在新目录确认信任，避免改动 `~/.claude.json`） |
| Devin | 3000.11.92 | 真实账号 | 全流程正常（echo 未触发审批） |
| omp | 18.11.3 | 真实账号 | 全流程正常（原生 + 会话摘要） |
| Grok Build | 1.0.62 | 真实账号已过期 | 报 blocked/auth “Sign in again”，退出报 error |
| Kimi Code | 2.2.0 | 真实账号 token 失效 | 首轮报 error；后续同样的错误报 done；信任框报 idle |
| Cursor Agent | 2026.07.09 | 真实未登录 | 报 blocked/auth |
| Amp | 0.0.1792310430 | 无账号 | 登录询问报 blocked/auth，取消后报 error |
| Droid | 0.244.0 | 无账号 | 登录页报 idle（规则未覆盖） |
| Qoder CLI | 1.1.73 | 无账号 | 登录页、认证方式页报 idle（规则未覆盖） |
| Kiro CLI | 2.32.0 | DMG 只读挂载，无账号 | 登录提示报 idle（规则未覆盖），退出报 error |
| Maki、Muse | — | — | 未找到可信安装来源，未实测 |

## 后续事项（均为既有问题，未在本次修复）

- Codex 0.163：回合结束只报 idle，会话库中已有 `task_complete`，需排查 tracker 认领。启动时有约 2 s 的 working 闪烁。
- 规则缺口：Kimi 和 Gemini 的信任框；Gemini 的认证框与等待认证页；Droid、Qoder、Kiro 的登录页；Kilo 的拒绝原因输入框；ZCode 的权限框；Crush 权限框落在 `bottom_non_empty_lines(12)` 之外时只能依赖原生上报；Aider 和 Hermes 流式输出期间的判定。
- Crush 拒绝权限后停在 working。Kimi 重复出现的错误报 done。
- 通过 `node <script>.js` 启动时不按脚本名识别 agent。
