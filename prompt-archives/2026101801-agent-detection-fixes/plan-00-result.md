# Plan 00 执行结果

## 改动

- **Codex**：profile 改为 `NativeSupport::Partial`，新增 `NativeQuirks::idle_for_done`。Codex 原生的 `idle` 不再覆盖 w1 判出的 `done`。启动时约 2 s 的 `working` 是 Codex 自己的原生上报，保持不变。
- **Crush**：
  - tracker 把 `User denied permission` 的 tool 结果视为回合中止。
  - 会话库明确报中止时，其 `idle` 优先于原生 `done`。
  - 新增整屏匹配的 `permission_dialog_full` 规则。
  - 两条权限规则的 msg 固定为 Crush 原生同款 `Permission required`。
- **Kimi 重复错误**：`ExternalInsight` 增加 `turn` 计数，reporter 以“状态 + 回合”判断变化。
- **新增规则**：
  - Kimi / Gemini / Qoder 信任框；
  - Gemini / Qoder 认证与等待认证；
  - Droid / Qoder / Kiro 登录页；
  - Kilo 拒绝原因输入框；
  - ZCode 权限面板，且权限规则优先级提到标题 spinner 之上。
- **规则 DSL**：
  - `turn_only`：working 只在用户提交后的回合内生效；
  - `msg`：固定摘要，用于登录页这类屏幕文字不适合作摘要的界面，取值 `Sign in required`。
- **ZCode**：新增 `blocked_detail` hook，从权限面板取 `<工具> <请求>`，例如 `Bash echo …`。
- **blocked 摘要兜底**：跳过没有字母数字的行，例如 Gemini 重绘时的 `▀▀▀` 分隔线。
- **Hermes**：底部 3 行看不到 `❯` 时视为重绘空档，不更新状态。
- **Aider**：新增 `turn_only` 规则：回合内底部不是提示行即为 working。
- **识别**：`identify_token` 先取文件名再去掉 `@版本`，`node …/@letta-ai/letta-code/letta.js` 可识别为 Letta。已有识别测试中补了一条。
- **README**：
  - Codex 行改为 Merge (≥ 0.163)，验证版本 0.163；
  - Kilo 行的验证状态改为 scratch install + mock, 7.8.12。

## 验证

- `cargo clippy --all-targets -- -D warnings`、`cargo test`、`RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` 全部通过。
- **回放**：以修复前代码为基线回放全部录制，差异只有以下预期项：
  - Aider 流式回复出现 working → done；
  - Droid / Qoder / Kiro 登录页和 Gemini 信任 / 认证判为 blocked；
  - Kilo 拒绝原因输入判为 blocked；
  - Crush 全屏权限框判为 blocked，拒绝后回到 idle；
  - Hermes 的跳变消失；
  - 细粒度录制下 Kimi 信任框和 ZCode 权限面板判为 blocked；
  - Codex 只有头部的 native 标记变化；
  - 上述 blocked 的 msg 改为固定或面板摘要。
- **真机**（隔离 HOME，Codex 使用真实账号）：

  | Agent | 结果 |
  | --- | --- |
  | Codex 0.163 | 回合结束报 `done`（msg `pong`）；审批报 blocked `$ echo …`；拒绝后 idle |
  | Crush（mock） | 权限 blocked；拒绝后 idle |
  | Kilo 7.8.12（mock） | 审批 blocked；拒绝后的原因输入框保持 blocked |
  | ZCode（mock） | 权限面板 blocked，msg `Bash echo w1probe-$((6*7))` |
  | Hermes（mock） | working → done，无跳变 |
  | Aider（mock） | 快速回复也出现 working → done |
  | Gemini（mock） | done / blocked / 拒绝后 idle 正常 |
  | Kimi（失效账号） | 两次报错都报 `error` |
  | Droid / Qoder / Kiro | 登录页报 blocked/auth `Sign in required` |
  | `node letta.js` | 识别为 letta |

## 已知限制

- Codex 的标题先变为 `Action Required`、审批框尚未画出时，有约 0.5 s 的 blocked msg 取到状态栏；审批框画出后即更正。这是修复前已有的行为，未改。
- ZCode 没有会话库：中途取消时仍按屏幕报 `done`，与修复前一致。
