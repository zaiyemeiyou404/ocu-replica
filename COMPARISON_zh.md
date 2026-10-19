# ocu-replica

OpenAI Codex 桌面版 Windows 辅助程序 `codex-computer-use.exe` 的**行为复刻**（clean-room）。

- 不复制原版源码、二进制或逐字错误文案
- 协议字段名、方法名来自对原版的**静态分析**（字符串表 / serde 字段 / JS 调用方）
- 目标：`x86_64-pc-windows-gnu`，依赖 `windows` crate 0.61

分析依据：`/workspace/codex-app/ANALYSIS_windows_zh.md`、`ANALYSIS_zh.md`，以及解包后的
`codex-computer-use.exe` 字符串与 `@oai/cua` 的 window2 类型定义。

## 变更摘要（相对上一版）

1. **真实 WGC 取帧**：`IGraphicsCaptureItemInterop::CreateForMonitor`、`CreateDirect3D11DeviceFromDXGIDevice`、`Direct3D11CaptureFramePool::CreateFreeThreaded`、`GraphicsCaptureSession`（关光标/边框）、超时等待一帧、staging `Map`、按窗口矩形裁剪后 PNG。BitBlt 仅作回退；路径写入 `diagnostic_state`。
2. **真实 WASAPI loopback**：默认 render 端点 + `AUDCLNT_STREAMFLAGS_LOOPBACK`，采集线程写出 24 kHz 立体声 WAV；会话级高风险审批不变。

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
| 截屏 | WGC `CreateForMonitor` + 裁剪；无 BitBlt | **真实 WGC 取帧**（CreateForMonitor → CreateFreeThreaded → StartCapture → TryGetNextFrame → staging Map → 裁剪 PNG）；失败/超时回退 BitBlt；`diagnostic_state.cacheDiagnostics.lastCapturePath` = `wgc`/`bitblt` |
| 输入 | `SendInput` + `SetCursorPos`，前台激活 | 同左（`AttachThreadInput` + `SetForegroundWindow` + `BringWindowToTop` + `SwitchToThisWindow`） |
| UIA | CUIAutomation 缓存 + element_index | 同左；过期 revision 拒绝 |
| Esc | `WH_KEYBOARD_LL` + `WH_MOUSE_LL`，忽略 `LLKHF_INJECTED` | 同左 |
| 叠加层 | Composition 动画光标 + `WDA_EXCLUDEFROMCAPTURE` | 轻量分层窗口 + `SetWindowDisplayAffinity(0x11)`；**无**原版动画/文案 |
| 禁止名单 | 终端 / 密码管理器 / AV / 系统壳 | 同名单（来自原版字符串表） |
| 首次审批 | elicitation / MessageBox | MessageBox（Yes=会话，Cancel=始终）；MCP 路径返回 `approvalRequest` |
| URL 策略 | 请求 chatgpt.com `site_status` | **故意不做**；预留 `UrlPolicy` trait，默认放行 |
| 音频 | WASAPI loopback | **真实 WASAPI loopback**：IMMDeviceEnumerator 默认 render → IAudioClient(`AUDCLNT_STREAMFLAGS_LOOPBACK`) → IAudioCaptureClient；重采样为 24 kHz 立体声 WAV；高风险且仅会话审批 |

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

## 验证状态

| 项 | 结果 |
|---|---|
| `cargo test`（Linux，43 项） | 通过：协议解析、按键、禁止名单、坐标、MCP schema、审批/中断标记 |
| `cargo build --release --target x86_64-pc-windows-gnu` | 通过，约 780KB |
| Wine `--help` / MCP initialize+tools/list | 通过（/usr/lib/wine/wine64；GUI/WGC 不可用属预期） |
| 真 Windows：WGC 取帧 / WASAPI loopback / UIA / Esc / MessageBox / 高 DPI | **未测**（Wine 无真 WGC/音频设备） |

## 目录

```
src/
  protocol.rs   原生协议 + 20 方法参数
  mcp.rs        MCP 前端
  keys.rs       键位表
  geometry.rs   坐标/裁剪
  blocklist.rs  禁止名单
  policy.rs     审批 / 规则 / 中断标记 / UrlPolicy 钩子
  engine.rs     调度
  image.rs      PNG
  win/          Windows 后端（cfg(windows)）
ocu-replica.exe
README_zh.md
```

## 许可与免责

MIT。本项目是独立重写，与 OpenAI 无隶属关系。仅用于在你自己的 Windows 机器上做自动化研究；请遵守当地法律与软件许可。
