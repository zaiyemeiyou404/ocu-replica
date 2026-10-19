# ocu-native：保留可行部分

基于 ocu-replica 的 Windows 原生控件实验版本，只开放已经验证的窗口观察、指定控件输入和标准按钮操作。真实软件目前只验证了本机记事本，不代表通用后台桌面兼容。

## 能力与限制

| 能力 | 验证范围 |
| --- | --- |
| 列出窗口、读取 UIA 树 | 本机窗口与测试框架 |
| 遮挡窗口截图 | WGC CreateForWindow；最小化窗口明确报错 |
| 后台替换文字、光标处插入 | 标准原生 Edit、WinForms Edit、本机记事本 RichEditD2DPT |
| 后台按钮、复选框操作 | 标准 Button / WinForms 测试控件 |
| 控件身份检查 | 保留实际 UIA 元素并核对身份，不按新树相同序号重新定位 |
| 请求超时、MCP 取消与恢复 | supervisor 结束自己的 helper；不自动重试，恢复后要求重新观察 |

输入必须指定最近观察得到的 element_index。仅支持后台模式与单次左键按钮操作；不认识的控件明确失败，不回退到通用 UIA Invoke/Value 或系统键鼠注入。

接口不再提供 CDP、音频、前台模式、全局快捷键、坐标点击、拖拽、通用滚动、应用启动、窗口移动或屏幕范围调整。部分上游后端代码仍作为构建依赖保留，MCP 与原生协议的统一入口均拒绝这些移除的方法。浏览器控制可另外使用成熟的 CDP 方案。

WPF 测试未满足保持前台窗口不变的要求；微信、QQ、WPS、Office 未验证。物理 Esc 的端到端真人验收尚未完成，MCP 取消测试不能替代该项验收。

## 构建与启动

需要 Windows 10/11、Rust MSVC 工具链及 Visual Studio C++ Build Tools、Python 3。运行 supervisor 只用 Python 标准库。

在仓库根目录执行：

```powershell
cargo test --locked
cargo build --locked
New-Item -ItemType Directory -Path bin -Force | Out-Null
Copy-Item target/debug/ocu-replica.exe bin/ocu-native.exe
./run.ps1
```

若设置了 CARGO_TARGET_DIR，请从对应构建目录复制程序。可执行文件不纳入 Git。

这是 MCP stdio 服务，启动后等待客户端发送 JSON。客户端配置示例（替换路径）：

```json
{
  "mcpServers": {
    "ocu-native": {
      "command": "python.exe",
      "args": ["D:/path/to/ocu-replica/runtime/supervisor.py"]
    }
  }
}
```

也可给 supervisor.py 传入 --exe 指定 helper。状态默认写到仓库的 state/，可用 OCU_REPLICA_HOME 覆盖。首次访问应用沿用原项目的审批机制，不要把测试预批准环境变量复制到日常环境。

## 工具

list_apps、list_windows、get_window、get_window_state、list_displays、diagnostic_state、click_element、type_text、set_value、end_turn、close。

发生界面变化、错误或工作进程重启后先重新观察。超时或取消会返回 outcome_unknown，表示目标操作结果未知，不意味着目标软件已经回滚。

## 验证

无需 GUI 的协议检查：

```powershell
python tests/test_protocol.py
```

标准控件验收需要 Pillow 和 .NET Framework C# 编译器：

```powershell
python -m pip install Pillow
& "$env:WINDIR/Microsoft.NET/Framework64/v4.0.30319/csc.exe" /nologo /target:winexe /out:tests/ocu-lab-fixture.exe /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /reference:System.Web.Extensions.dll tests/Fixture.cs
python tests/run_lab.py --supervise
```

GUI 验收只操作自己创建的临时窗口，会短暂显示测试窗口；结果写入 tests/latest/results.json。测试运行记录与适用范围见 [docs/verification.md](docs/verification.md)。

目录：src/ 为原生 helper，runtime/ 为 supervisor，tests/ 为验收脚本和测试窗口源码，run.ps1 为入口。

来源：zaiyemeiyou404/ocu-replica，基线 2234a69，MIT。
