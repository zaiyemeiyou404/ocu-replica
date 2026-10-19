# ocu-replica

[![Windows CI](https://github.com/zaiyemeiyou404/ocu-replica/actions/workflows/windows.yml/badge.svg)](https://github.com/zaiyemeiyou404/ocu-replica/actions/workflows/windows.yml)

OpenAI Codex 桌面版 Windows 辅助程序 `codex-computer-use.exe` 的**行为复刻**（clean-room）。

- 不复制原版源码、二进制或逐字错误文案
- 协议字段名、方法名来自对原版的**静态分析**（字符串表 / serde 字段 / JS 调用方）
- 目标：`x86_64-pc-windows-gnu`，依赖 `windows` crate 0.61

分析依据：`/workspace/codex-app/ANALYSIS_windows_zh.md`、`ANALYSIS_zh.md`，以及解包后的
`codex-computer-use.exe` 字符串与 `@oai/cua` 的 window2 类型定义。

## 变更摘要（相对上一版）

1. **后台模式（BACKGROUND）**：可选 `input_mode` / `OCU_INPUT_MODE`=`foreground|background|auto`（默认 `auto`）。后台：WGC `CreateForWindow` + PrintWindow 回退；UIA 优先（Invoke/Toggle/SelectionItem/ExpandCollapse/LegacyIAccessible、ValuePattern、ScrollPattern）；可选 `OCU_ALLOW_POSTMESSAGE=1`；`auto` 下 SendInput 需空闲 `OCU_IDLE_MS`（默认 3s，超时 `user_busy`）；结果与 `diagnostic_state` 报告 `backend`。
2. **浏览器 CDP 扩展工具**：探测 `OCU_CDP_PORT` 或 `127.0.0.1:9222-9229`，提供 `browser_*`（不激活标签；localhost WebSocket）。
3. **单屏范围（SINGLE-MONITOR SCOPE）**：`list_displays` / `scope_monitor` / `OCU_MONITOR` / `move_window_to_scope`；窗口列表按 >50% 面积过滤；越界坐标 `out_of_scope`；`launch_app` 把新窗口放到工作区。
4. 既有真实 WGC（CreateForMonitor）与 WASAPI loopback 保留。

## 快速开始

```text
ocu-replica.exe                 # MCP stdio（默认）
ocu-replica.exe --stdio-native  # 原版同款换行 JSON
ocu-replica.exe --parent-pid N  # 父进程退出时一起退出
ocu-replica.exe --help
```

环境变量：

| 变量 | 作用 |
|---|---|
| `CODEX_HOME` / `OCU_REPLICA_HOME` | 状态目录（审批、中断标记） |
| `SKY_ENABLE_AUDIO=1` | 打开录音工具 |
| `OCU_REPLICA_MAX_EDGE` | 截图长边上限，默认 1600 |
| `OCU_INPUT_MODE` | `foreground` / `background` / `auto`（默认 `auto`） |
| `OCU_IDLE_MS` / `OCU_IDLE_TIMEOUT_MS` | auto→SendInput 空闲门槛 / 等待超时（默认 3000 / 30000） |
| `OCU_ALLOW_POSTMESSAGE=1` | 允许 WM_LBUTTON*/WM_CHAR PostMessage（Chromium/Electron/DirectX 通常无效） |
| `OCU_CDP_PORT` | Chromium 远程调试端口；未设则扫描 9222–9229 |
| `OCU_MONITOR` | 单屏范围：显示器序号、`\\.\DISPLAYn` 或 `primary` |
| `OCU_PREAPPROVE` | 逗号/分号分隔的 exe 名或完整路径。命中则**跳过**首次 MessageBox，且**不写入**审批文件。默认不设置，行为与原先一致（仍弹窗）。仅用于无人值守 / CI |
| `OCU_PREAPPROVE_FILE` | 同上，文本文件，一行一个 token，`#` 开头为注释 |

## 两种协议

### 1. `--stdio-native`（与原版 helper 一致）【已验证】

请求（一行一个 JSON）：

```json
{"id":1,"method":"get_window_state","params":{"window":{"app":"notepad.exe","id":123},"include_screenshot":true,"include_text":true},"meta":{"x-oai-cua-request-budget-ms":10000,"x-codex-turn-metadata":{"session_id":"s","turn_id":"t"}}}
```

响应：

```json
{"id":1,"ok":true,"result":{...}}
{"id":1,"ok":false,"error":"..."}
{"id":1,"ok":false,"approvalRequest":{"app":"...","displayName":"...","riskLevel":"low","allowPersistentApproval":true}}
```

物理 Esc：写中断标记到 `<home>/cache/computer-use/interrupts/<session>/<turn>`，并以退出码 **130** 退出（与原版一致）。

### 2. 默认 MCP stdio（JSON-RPC 2.0）

协议版本：`2024-11-05` / `2025-03-26`。支持 `initialize`、`tools/list`、`tools/call`；截图以 `image` content block（PNG base64）返回。Esc 时返回错误，不退出进程；`end_turn` 清除本轮中断闩锁。

## 20 个方法对照

| 方法 | 原版 | 本复刻 | 参数字段（已验证除非注明） |
|---|---|---|---|
| `list_apps` | ✓ | ✓ | （无）→ `[{id, displayName?, windows, lastUsedDate?, useCount?, isRunning?}]` |
| `launch_app` | ✓ | ✓ | `app` |
| `list_windows` | ✓ | ✓ | → `[{app,id,title?}]` |
| `get_window` | ✓ | ✓ | `id`, `app?` |
| `activate_window` | ✓ | ✓ | `window` |
| `get_window_state` | ✓ | ✓ | `window`, `include_screenshot?`(默认 true), `include_text?`(默认 false) → `window`, `screenshots[]`, `accessibility` |
| `click` | ✓ | ✓ | `window`, `x?`, `y?`, `element_index?`, `screenshotId?`, `mouse_button?`, `click_count?` |
| `click_element` | ✓ | ✓ | `window`, `element_index`, `mouse_button?`, `click_count?` |
| `drag` | ✓ | ✓ | `window`, `from_x/y`, `to_x/y`, `screenshotId?` |
| `scroll` | ✓ | ✓ | `window`, `x`, `y`, `scrollX?`, `scrollY?`, `screenshotId?` |
| `scroll_element` | ✓ | ✓ | `window`, `element_index`, `direction`, `pages?` 【`pages`/`direction` 形态为推断，来自字符串表】 |
| `type_text` | ✓ | ✓ | `window`, `text` |
| `press_key` | ✓ | ✓ | `window`, `key`（`+` 分隔，X11 keysym 风格） |
| `set_value` | ✓ | ✓ | `window`, `element_index`, `value` |
| `perform_secondary_action` | ✓ | ✓ | `window`, `element_index`, `action`（Raise / Scroll Up\|Down\|Left\|Right / Expand / Collapse） |
| `start_audio_recording` | ✓ | ✓* | `max_duration_ms?`（1..=300000，默认 60000）；需 `SKY_ENABLE_AUDIO=1` |
| `stop_audio_recording` | ✓ | ✓* | → `{filepath}`（24 kHz 立体声 WAV） |
| `diagnostic_state` | ✓ | ✓ | `window?` → `cacheDiagnostics`（字段名已验证；具体计数语义为推断） |
| `end_turn` | ✓ | ✓ | （无） |
| `close` | ✓ | ✓ | （无） |

\* 录音：已接真实 WASAPI loopback；无默认播放设备或权限不足时 `start`/`stop` 返回明确错误（不再写静音占位）。仍须 `SKY_ENABLE_AUDIO=1` + 会话审批。

### `get_window_state` 返回字段

| 字段 | 状态 |
|---|---|
| `screenshots[].id` / `zIndex` / `url` | 已验证 |
| `screenshots[].originX/Y` / `width` / `height` | 已验证（物理像素原点 + 图像尺寸） |
| `accessibility.tree` / `focused_element` / `selected_text` / `selected_elements` / `document_text` | 已验证 |
| `snapshotRevision` / `accessibilityRevision` | 已验证存在于原版诊断/缓存；本复刻在 `diagnostic_state` 与内部缓存使用 revision，**未**塞进 `accessibility` 对象本身（JS 客户端也不读取它们）【推断：对 JS 路径非必需】 |

### 坐标约定【推断，与分析报告一致】

模型给的 `x/y` 是**相对该窗口截图像素**的坐标。辅助程序按截图时的 `origin + x*scale` 换成物理屏幕坐标再 `SendInput`。注入前会再读一次窗口矩形；偏差超过 2px 则报错，要求重新 `get_window_state`。

## 行为对齐要点

| 能力 | 原版 | 本复刻 |
|---|---|---|
| 截屏 | WGC `CreateForMonitor` + 裁剪；无 BitBlt | 前台：CreateForMonitor+裁剪，失败回退 BitBlt；**后台：CreateForWindow（关边框/光标）+ PrintWindow(PW_RENDERFULLCONTENT)**；最小化窗口明确报错；`backend`/`lastCapturePath` = `wgc`/`wgc_window`/`printwindow`/`bitblt` |
| 输入 | `SendInput` + `SetCursorPos`，前台激活 | `input_mode`：后台 UIA 优先（不 SetForegroundWindow）；可选 PostMessage；`auto` 空闲后才 SendInput，注入中遇物理输入立即中止并恢复光标/前台；结果含 `backend: uia\|postmessage\|cdp\|sendinput` |
| UIA | CUIAutomation 缓存 + element_index | 同左；后台 click/type/scroll 走 Invoke/Value/Scroll 等 Pattern；过期 revision 拒绝 |
| Esc | `WH_KEYBOARD_LL` + `WH_MOUSE_LL`，忽略 `LLKHF_INJECTED` | 同左 |
| 叠加层 | Composition 动画光标 + `WDA_EXCLUDEFROMCAPTURE` | 轻量分层窗口 + `SetWindowDisplayAffinity(0x11)`；**无**原版动画/文案 |
| 禁止名单 | 终端 / 密码管理器 / AV / 系统壳 | 同名单（来自原版字符串表） |
| 首次审批 | elicitation / MessageBox | MessageBox（Yes=会话，Cancel=始终）；MCP 路径返回 `approvalRequest` |
| URL 策略 | 请求 chatgpt.com `site_status` | **故意不做**；预留 `UrlPolicy` trait，默认放行 |
| 音频 | WASAPI loopback | **真实 WASAPI loopback**：IMMDeviceEnumerator 默认 render → IAudioClient(`AUDCLNT_STREAMFLAGS_LOOPBACK`) → IAudioCaptureClient；重采样为 24 kHz 立体声 WAV；高风险且仅会话审批 |


## 后台模式

用户可继续操作本机，同时让代理在后台工作（默认 `OCU_INPUT_MODE=auto`）。

| 模式 | 行为 |
|---|---|
| `foreground` | 与原先一致：激活窗口 + SendInput |
| `background` | **绝不**动真实光标/焦点；仅 UIA /（可选）PostMessage / CDP；做不到则失败 |
| `auto` | 先试后台；仅当用户空闲（`OCU_IDLE_MS`，默认 3s）才回退 SendInput；等满 `OCU_IDLE_TIMEOUT_MS`（默认 30s）返回 `user_busy` |

**覆盖范围**

- 适用：标准 Win32 / WinUI / WPF / Office 等 UIA 友好应用；以及通过 CDP 控制的 Chromium/Edge
- 自定义绘制应用、游戏、多数 DirectX/Electron 无障碍树时：需要 `foreground`，或放到虚拟机 / 独立虚拟显示器

**安全启动 Chrome/Edge 调试端口（仅本机）**

```text
chrome.exe --remote-debugging-port=9222 --user-data-dir=%TEMP%\ocu-chrome-dbg
msedge.exe --remote-debugging-port=9222 --user-data-dir=%TEMP%\ocu-edge-dbg
```

务必使用**单独的** `--user-data-dir`，且只监听 localhost。CDP 工具同样走禁止名单与审批；Esc 仍可中断。

可选 PostMessage（`OCU_ALLOW_POSTMESSAGE=1`）对 Chromium/Electron/DirectX **通常无效**，仅作 Win32 控件兜底。

## 分屏 / 单屏模式

用 `OCU_MONITOR` 或工具 `scope_monitor` / `list_displays` 把代理限制在**一块屏幕**上：

- `list_windows` / `list_apps` 只返回面积 >50% 落在该显示器上的窗口
- 坐标或窗口越界 → `out_of_scope`
- `launch_app` 会把新顶层窗口挪到该显示器工作区（`SetWindowPos`，保尺寸）
- `move_window_to_scope` 显式搬窗（需审批）

**注意**：系统级鼠标与键盘焦点仍是跨显示器共享的。要真正并行使用，请与**后台模式**组合（代理用 UIA/CDP，不抢光标）。

只有一块物理屏时，可选用开源 **IddCx 虚拟显示器驱动**（例如社区项目 “Virtual Display Driver”）再加一块虚拟屏专给代理。**本仓库不捆绑任何驱动**，仅作可选建议。

## MCP 接入片段

### Cursor `~/.cursor/mcp.json`

```json
{
  "mcpServers": {
    "ocu-replica": {
      "command": "C:\\\\path\\\\to\\\\ocu-replica.exe",
      "args": [],
      "env": {
        "OCU_REPLICA_HOME": "%USERPROFILE%\\.ocu-replica"
      }
    }
  }
}
```

### Codex `~/.codex/config.toml`

```toml
[mcp_servers.ocu-replica]
command = "C:\\path\\to\\ocu-replica.exe"
args = []
```

### Hermes

把同一条 command 配成 MCP stdio server 即可（工具名与上表一致）。

## 构建

```bash
rustup target add x86_64-pc-windows-gnu
# Debian/Ubuntu: apt install mingw-w64
cargo test                          # 纯逻辑单测（Linux 可跑）
cargo build --release --target x86_64-pc-windows-gnu
```

产物：`target/x86_64-pc-windows-gnu/release/ocu-replica.exe`

GitHub Actions 使用 MSVC 目标（`x86_64-pc-windows-msvc`）。若某次 runner 上 MSVC 链接失败，可改回本机已验证的 gnu 目标。

## CI

工作流：`.github/workflows/windows.yml`（`push` / `pull_request` / `workflow_dispatch`）。

矩阵：`windows-latest` 与 `windows-2022`。每台机器上：

1. `cargo build --release --target x86_64-pc-windows-msvc`，并把 `ocu-replica.exe` 作为 artifact 上传
2. `cargo test`（协议 / 策略 / 几何等纯逻辑）
3. `cargo clippy`（**不阻断**，失败只记警告）
4. 实机 GUI：`scripts/live-gui-test.ps1` 在 runner 的交互桌面上启动记事本（以及可用的画图），通过 MCP stdio 调用 `initialize`、`tools/list`，并实际跑 `list_displays`、`list_windows` / `list_apps`、`get_window_state`（截图 + UIA 树）、`diagnostic_state`（报告 `wgc` / `bitblt` / `printwindow`）、`set_value` / `type_text`（写进记事本再从 UIA `document_text` 读回）、`click_element`、`press_key`、`scroll`、`input_mode=background` 与前台对照、`OCU_MONITOR=0` 以及把窗口挪出该屏后的 `out_of_scope`

审批对话框会卡住无头/无人值守会话，所以 CI 设置 `OCU_PREAPPROVE=notepad.exe,mspaint.exe`。不设置该变量时，首次控制应用仍会弹出 MessageBox。

Esc 钩子只做安装冒烟（`diagnostic_state.cacheDiagnostics.escHookInstalled`）。不在 CI 里真的按 Esc，否则会闩住本轮并中断后续调用。低层钩子装在独立线程上并跑消息循环，避免 stdio 线程阻塞时把全桌面的键盘鼠标卡住。

每项能力的 pass / fail / skip 写到 `$GITHUB_STEP_SUMMARY`，截图和 JSON 响应作为 `gui-evidence-<runner>` artifact。托管 runner 没有可用 GPU 时，WGC（`CreateForMonitor` / `CreateForWindow`）会失败并回退到 BitBlt 或 PrintWindow：这种情况下截图路径记为 **pass**（并写明实际路径），WGC 单独记为 **skip** 及原因，不会伪装成 WGC 成功。若连回退路径也因桌面帧缓冲不可用而失败，对应项记 **skip** 或 **fail**（脚本里按错误文本区分），不会记假通过。

## 验证状态

| 项 | 结果 |
|---|---|
| `cargo test`（Linux） | 通过：协议、mode/idle、CDP 消息、monitor 重叠/范围、MCP schema（20+扩展） |
| `cargo build --release --target x86_64-pc-windows-gnu` | 通过（RAM-light：opt-level=s, thin LTO） |
| Wine `--help` / MCP initialize+tools/list | 通过（/usr/lib/wine/wine64；GUI/WGC 不可用属预期） |
| 真 Windows：WGC / BitBlt / UIA / Esc 钩子安装 / 前台与后台输入 / 单屏 `out_of_scope` | 由 GitHub Actions `windows.yml` 在托管 runner 的交互桌面上实测；每项 pass/fail/skip 见该次运行的 step summary 与 `gui-evidence-*` artifact。WGC 在无 GPU 时允许 skip（回退路径必须真实成功，不能记成 WGC） |

## 目录

```
src/
  protocol.rs   原生协议 + 20 方法 + browser_*/list_displays 等扩展
  mcp.rs        MCP 前端
  mode.rs       input_mode / idle 门控
  monitor.rs    单屏范围重叠与解析
  activity.rs   物理输入跟踪
  cdp.rs        Chromium CDP（localhost）
  keys.rs / geometry.rs / blocklist.rs / policy.rs / engine.rs / image.rs
  win/          Windows 后端（capture/uia/input/postmessage/display/…）
ocu-replica.exe
README_zh.md
```

## 许可与免责

MIT。本项目是独立重写，与 OpenAI 无隶属关系。仅用于在你自己的 Windows 机器上做自动化研究；请遵守当地法律与软件许可。
