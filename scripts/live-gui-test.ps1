# Live GUI integration test for ocu-replica on an interactive Windows desktop.
# Talks MCP stdio, drives Notepad, and writes a markdown summary plus JSON/PNG evidence.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$OutDir = "gui-artifacts"
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

$script:Rows = New-Object System.Collections.Generic.List[object]
$script:Clients = @()

function Add-Row([string]$Name, [string]$Status, [string]$Detail) {
    if ($null -eq $Detail) { $Detail = "" }
    $d = $Detail -replace "\s+", " "
    if ($d.Length -gt 500) { $d = $d.Substring(0, 500) }
    $script:Rows.Add([pscustomobject]@{ Name = $Name; Status = $Status; Detail = $d })
    Write-Host ("[{0}] {1} — {2}" -f $Status.ToUpper(), $Name, $d)
}

function Save-Json([string]$Name, $Obj) {
    $path = Join-Path $OutDir "$Name.json"
    try {
        ($Obj | ConvertTo-Json -Depth 40 -Compress:$false) | Set-Content -Path $path -Encoding utf8
    } catch {
        Set-Content -Path $path -Encoding utf8 -Value ("{0}" -f $Obj)
    }
}

function New-OcuClient([string]$Label, [hashtable]$ExtraEnv) {
    if (-not (Test-Path $Exe)) { throw "exe not found: $Exe" }
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = (Resolve-Path $Exe).Path
    $psi.WorkingDirectory = (Split-Path -Parent $psi.FileName)
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    $psi.StandardOutputEncoding = New-Object System.Text.UTF8Encoding $false
    $psi.StandardErrorEncoding = New-Object System.Text.UTF8Encoding $false
    # $HOME is a read-only automatic variable in PowerShell.
    $stateDir = Join-Path $OutDir "home-$Label"
    New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
    $psi.Environment["OCU_REPLICA_HOME"] = $stateDir
    $psi.Environment["OCU_PREAPPROVE"] = "notepad.exe,mspaint.exe"
    $psi.Environment["OCU_INPUT_MODE"] = "foreground"
    $psi.Environment["OCU_IDLE_MS"] = "0"
    if ($ExtraEnv) {
        foreach ($k in $ExtraEnv.Keys) { $psi.Environment[$k] = [string]$ExtraEnv[$k] }
    }
    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi
    [void]$proc.Start()
    $proc.StandardInput.AutoFlush = $true
    $proc.StandardInput.NewLine = "`n"
    $errTask = $proc.StandardError.ReadToEndAsync()
    $client = @{
        Label   = $Label
        Proc    = $proc
        In      = $proc.StandardInput
        Out     = $proc.StandardOutput
        ErrTask = $errTask
        NextId  = 1
    }
    $script:Clients += $client
    return $client
}

function Close-OcuClient($Client) {
    if (-not $Client) { return }
    try {
        if (-not $Client.Proc.HasExited) {
            try { $Client.In.Close() } catch {}
            if (-not $Client.Proc.WaitForExit(2000)) { $Client.Proc.Kill($true) }
        }
    } catch {}
    try {
        if ($Client.ErrTask.Wait(2000)) {
            $path = Join-Path $OutDir ("stderr-{0}.txt" -f $Client.Label)
            [System.IO.File]::WriteAllText($path, [string]$Client.ErrTask.Result)
        }
    } catch {}
}

function Read-McpLine($Client, [int]$TimeoutMs) {
    $task = $Client.Out.ReadLineAsync()
    if (-not $task.Wait($TimeoutMs)) {
        throw "timed out after ${TimeoutMs}ms waiting for MCP response (process exited=$($Client.Proc.HasExited))"
    }
    if ($task.IsFaulted) { throw $task.Exception.GetBaseException().Message }
    return $task.Result
}

function Send-Rpc($Client, [string]$Method, $Params, [int]$TimeoutMs = 20000) {
    if ($Client.Proc.HasExited) { throw "ocu-replica exited early (code $($Client.Proc.ExitCode))" }
    $id = $Client.NextId
    $Client.NextId = $id + 1
    $req = @{ jsonrpc = "2.0"; id = $id; method = $Method; params = $Params }
    $json = $req | ConvertTo-Json -Compress -Depth 30
    $Client.In.WriteLine($json)
    $line = Read-McpLine $Client $TimeoutMs
    if ([string]::IsNullOrWhiteSpace($line)) { throw "empty MCP response for $Method" }
    $resp = $line | ConvertFrom-Json
    return $resp
}

function Get-ToolText($Resp) {
    $blocks = @($Resp.result.content)
    $text = $blocks | Where-Object { $_.type -eq "text" } | Select-Object -First 1
    if ($text) { return [string]$text.text }
    return ""
}

function Get-ToolPayload($Resp) {
    if ($Resp.error) { throw ("JSON-RPC error {0}: {1}" -f $Resp.error.code, $Resp.error.message) }
    $text = Get-ToolText $Resp
    if ($Resp.result.isError) {
        $extra = ""
        if ($Resp.result.approvalRequest) { $extra = " approvalRequest=" + ($Resp.result.approvalRequest | ConvertTo-Json -Compress -Depth 6) }
        throw ("tool error: {0}{1}" -f $text, $extra)
    }
    if ([string]::IsNullOrWhiteSpace($text) -or $text -eq "null") { return $null }
    try { return ($text | ConvertFrom-Json) } catch { return $text }
}

function Save-Images($Resp, [string]$Prefix) {
    $saved = @()
    $i = 0
    foreach ($block in @($Resp.result.content)) {
        if ($block.type -eq "image" -and $block.data) {
            $bytes = [Convert]::FromBase64String([string]$block.data)
            $path = Join-Path $OutDir ("{0}-{1}.png" -f $Prefix, $i)
            [System.IO.File]::WriteAllBytes($path, $bytes)
            $saved += $path
            $i++
        }
    }
    # A one-element array is unrolled into a bare string on return. Indexing that
    # string yields the first character, so Test-Png then rejects a real PNG.
    return ,$saved
}

function Test-Png([string]$Path) {
    if (-not (Test-Path $Path)) { return $false }
    $fs = [System.IO.File]::OpenRead($Path)
    try {
        $buf = New-Object byte[] 8
        $n = $fs.Read($buf, 0, 8)
        return ($n -eq 8 -and $buf[0] -eq 137 -and $buf[1] -eq 80 -and $buf[2] -eq 78 -and $buf[3] -eq 71)
    } finally { $fs.Dispose() }
}

function Get-ImageStats([string]$Path) {
    try { Add-Type -AssemblyName System.Drawing -ErrorAction Stop } catch { return $null }
    $bmp = $null
    try {
        $bmp = [System.Drawing.Bitmap]::FromFile($Path)
        $w = $bmp.Width; $h = $bmp.Height
        $target = 2500
        $step = [Math]::Max(1, [int][Math]::Sqrt(($w * $h) / $target))
        $n = 0; $nonBlack = 0
        for ($y = 0; $y -lt $h; $y += $step) {
            for ($x = 0; $x -lt $w; $x += $step) {
                $c = $bmp.GetPixel($x, $y)
                $n++
                $max = [Math]::Max($c.R, [Math]::Max($c.G, $c.B))
                if ($max -gt 16) { $nonBlack++ }
            }
        }
        $ratio = 0.0
        if ($n -gt 0) { $ratio = [double]$nonBlack / [double]$n }
        return @{ Width = $w; Height = $h; NonBlackRatio = $ratio }
    } catch {
        return $null
    } finally {
        if ($bmp) { $bmp.Dispose() }
    }
}

function Find-EditIndex([string]$Tree) {
    if (-not $Tree) { return $null }
    foreach ($line in ($Tree -split "`n")) {
        if ($line -match '^(\d+)\s+(edit|document)\b') {
            return [int]$Matches[1]
        }
    }
    return $null
}

function Text-Has($Payload, [string]$Needle) {
    if (-not $Payload) { return $false }
    $doc = [string]$Payload.accessibility.document_text
    $tree = [string]$Payload.accessibility.tree
    return (($doc -and $doc.Contains($Needle)) -or ($tree -and $tree.Contains($Needle)))
}

$winType = @"
using System;
using System.Runtime.InteropServices;
public static class OcuWin {
  [DllImport("user32.dll", SetLastError=true)]
  public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
  [DllImport("user32.dll")]
  public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
Add-Type -TypeDefinition $winType -ErrorAction Stop

function Get-WinRect([Int64]$Hwnd) {
    $r = New-Object OcuWin+RECT
    $ok = [OcuWin]::GetWindowRect([IntPtr]$Hwnd, [ref]$r)
    if (-not $ok) { return $null }
    return $r
}

# --- launch GUI apps ---
Get-Process notepad, mspaint -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400
$notepad = Start-Process -FilePath "$env:SystemRoot\System32\notepad.exe" -PassThru
$paint = $null
$paintPath = "$env:SystemRoot\System32\mspaint.exe"
if (Test-Path $paintPath) {
    try { $paint = Start-Process -FilePath $paintPath -PassThru } catch { $paint = $null }
}
Start-Sleep -Seconds 2

$client = $null
$scope = $null
$hardStop = $false
try {
    $client = New-OcuClient "main" @{}

    # initialize
    try {
        $resp = Send-Rpc $client "initialize" @{
            protocolVersion = "2024-11-05"
            capabilities    = @{}
            clientInfo      = @{ name = "ocu-ci"; version = "0.1.0" }
        } 15000
        Save-Json "initialize" $resp
        if ($resp.error) { throw $resp.error.message }
        $pv = [string]$resp.result.protocolVersion
        $name = [string]$resp.result.serverInfo.name
        if ($name -ne "ocu-replica") { throw "serverInfo.name=$name" }
        Add-Row "mcp_initialize" "pass" "protocolVersion=$pv server=$name"
        $client.In.WriteLine((@{ jsonrpc = "2.0"; method = "notifications/initialized" } | ConvertTo-Json -Compress))
    } catch {
        Add-Row "mcp_initialize" "fail" $_.Exception.Message
        $hardStop = $true
    }

    # tools/list
    $toolNames = @()
    if (-not $hardStop) {
        try {
            $resp = Send-Rpc $client "tools/list" @{} 15000
            $toolNames = @($resp.result.tools | ForEach-Object { [string]$_.name })
            Save-Json "tools-list" ($toolNames)
            $need = @(
                "list_displays", "list_windows", "list_apps", "get_window_state", "diagnostic_state",
                "set_value", "type_text", "click_element", "press_key", "scroll"
            )
            $missing = @($need | Where-Object { $toolNames -notcontains $_ })
            if ($missing.Count -gt 0) { throw ("missing tools: " + ($missing -join ", ")) }
            Add-Row "tools_list" "pass" ("count={0}" -f $toolNames.Count)
        } catch {
            Add-Row "tools_list" "fail" $_.Exception.Message
            $hardStop = $true
        }
    }

    $np = $null
    $editIndex = $null

    if (-not $hardStop) {
        try {
            $resp = Send-Rpc $client "tools/call" @{ name = "list_displays"; arguments = @{} } 20000
            $payload = Get-ToolPayload $resp
            Save-Json "list_displays" $payload
            $n = @($payload).Count
            if ($n -lt 1) { throw "no displays" }
            $d0 = @($payload)[0]
            Add-Row "list_displays" "pass" ("count={0} first={1} {2}x{3} primary={4}" -f $n, $d0.deviceName, $d0.rect.width, $d0.rect.height, $d0.primary)
        } catch {
            Add-Row "list_displays" "fail" $_.Exception.Message
        }

        try {
            $resp = Send-Rpc $client "tools/call" @{ name = "list_windows"; arguments = @{} } 20000
            $wins = @(Get-ToolPayload $resp)
            Save-Json "list_windows" $wins
            $np = $wins | Where-Object {
                ([string]$_.app) -match '(?i)notepad(\.exe)?$' -or ([string]$_.title) -match 'Notepad'
            } | Select-Object -First 1
            if (-not $np) { throw ("notepad not in list_windows; saw " + (($wins | ForEach-Object { $_.app + ":" + $_.title }) -join " | ")) }
            Add-Row "list_windows" "pass" ("notepad app={0} id={1} title={2} total={3}" -f $np.app, $np.id, $np.title, $wins.Count)
        } catch {
            Add-Row "list_windows" "fail" $_.Exception.Message
            $hardStop = $true
        }

        try {
            $resp = Send-Rpc $client "tools/call" @{ name = "list_apps"; arguments = @{} } 20000
            $apps = @(Get-ToolPayload $resp)
            Save-Json "list_apps" $apps
            $hit = $apps | Where-Object { ([string]$_.id) -match '(?i)notepad' -or (@($_.windows) | Where-Object { ([string]$_.app) -match '(?i)notepad' }) } | Select-Object -First 1
            if (-not $hit) { throw "notepad not in list_apps" }
            Add-Row "list_apps" "pass" ("apps={0} notepadId={1}" -f $apps.Count, $hit.id)
        } catch {
            Add-Row "list_apps" "fail" $_.Exception.Message
        }
    }

    function Get-WindowArg($Win) {
        return @{ app = [string]$Win.app; id = [int64]$Win.id }
    }

    $fgState = $null
    if (-not $hardStop -and $np) {
        try {
            $resp = Send-Rpc $client "tools/call" @{
                name      = "get_window_state"
                arguments = @{
                    window             = (Get-WindowArg $np)
                    include_screenshot = $true
                    include_text       = $true
                    input_mode         = "foreground"
                }
            } 60000
            $shots = @(Save-Images $resp "fg-state")
            $fgState = Get-ToolPayload $resp
            Save-Json "get_window_state_fg" $fgState
            $tree = [string]$fgState.accessibility.tree
            if (-not $tree) { throw "accessibility tree empty" }
            $editIndex = Find-EditIndex $tree
            $pngOk = ($shots.Count -ge 1) -and (Test-Png $shots[0])
            $stats = $null
            if ($pngOk) { $stats = Get-ImageStats $shots[0] }
            $statMsg = ""
            if ($stats) { $statMsg = (" png={0}x{1} nonBlack={2:N2}" -f $stats.Width, $stats.Height, $stats.NonBlackRatio) }
            if (-not $pngOk) { throw "screenshot missing or not a PNG" }
            if ($null -eq $editIndex) { throw ("no edit/document in tree: " + $tree.Substring(0, [Math]::Min(300, $tree.Length))) }
            Add-Row "get_window_state" "pass" ("backend={0} editIndex={1}{2}" -f $fgState.backend, $editIndex, $statMsg)
            if ($stats -and $stats.NonBlackRatio -lt 0.02) {
                Add-Row "capture_pixels" "fail" "screenshot is nearly black; capture did not show the desktop/window"
            } else {
                Add-Row "capture_pixels" "pass" $(if ($stats) { "nonBlack={0:N2}" -f $stats.NonBlackRatio } else { "png ok; pixel sample unavailable" })
            }
        } catch {
            Add-Row "get_window_state" "fail" $_.Exception.Message
            Add-Row "capture_pixels" "fail" "no screenshot"
            if ($_.Exception.Message -match "timed out") { $hardStop = $true }
        }

        if (-not $hardStop) { try {
            $resp = Send-Rpc $client "tools/call" @{ name = "diagnostic_state"; arguments = @{} } 20000
            $diag = Get-ToolPayload $resp
            Save-Json "diagnostic_fg" $diag
            $cd = $diag.cacheDiagnostics
            $hook = $cd.escHookInstalled
            if ($hook -eq $true) {
                Add-Row "esc_hook" "pass" "escHookInstalled=true (WH_KEYBOARD_LL + WH_MOUSE_LL)"
            } else {
                Add-Row "esc_hook" "fail" ("escHookInstalled=" + $hook)
            }
            $pathName = [string]$cd.lastCapturePath
            $detail = [string]$cd.lastCapturePathDetail
            if ($pathName -eq "wgc" -or $pathName -eq "wgc_window") {
                Add-Row "capture_path" "pass" ("path={0} {1}" -f $pathName, $detail)
                Add-Row "capture_wgc" "pass" $pathName
            } elseif ($pathName -eq "bitblt" -or $pathName -eq "printwindow") {
                Add-Row "capture_path" "pass" ("path={0} {1}" -f $pathName, $detail)
                Add-Row "capture_wgc" "skip" ("WGC not used ({0}). {1}" -f $pathName, $detail)
            } elseif (-not $pathName) {
                Add-Row "capture_path" "fail" "lastCapturePath empty"
                Add-Row "capture_wgc" "skip" "no capture path recorded"
            } else {
                Add-Row "capture_path" "fail" ("unexpected path={0} {1}" -f $pathName, $detail)
                Add-Row "capture_wgc" "skip" $detail
            }
        } catch {
            Add-Row "diagnostic_state" "fail" $_.Exception.Message
            Add-Row "esc_hook" "fail" $_.Exception.Message
            Add-Row "capture_path" "fail" $_.Exception.Message
        } }
    }

    $sentinelSet = "OCUCI_SETVALUE_ok"
    $sentinelType = "OCUCI_TYPE_fg"
    $sentinelBg = "OCUCI_TYPE_bg"

    if (-not $hardStop -and $np -and $null -ne $editIndex) {
        try {
            $resp = Send-Rpc $client "tools/call" @{
                name      = "set_value"
                arguments = @{
                    window         = (Get-WindowArg $np)
                    element_index  = $editIndex
                    value          = $sentinelSet
                    input_mode     = "foreground"
                }
            } 30000
            $setRes = Get-ToolPayload $resp
            Save-Json "set_value" $setRes
            $resp2 = Send-Rpc $client "tools/call" @{
                name      = "get_window_state"
                arguments = @{
                    window             = (Get-WindowArg $np)
                    include_screenshot = $false
                    include_text       = $true
                    input_mode         = "foreground"
                }
            } 30000
            $again = Get-ToolPayload $resp2
            Save-Json "after_set_value" $again
            if (-not (Text-Has $again $sentinelSet)) {
                throw ("value not in UIA tree/document_text. tree=" + [string]$again.accessibility.tree)
            }
            Add-Row "set_value" "pass" ("backend={0}; read back {1}" -f $setRes.backend, $sentinelSet)
        } catch {
            Add-Row "set_value" "fail" $_.Exception.Message
        }

        try {
            $resp = Send-Rpc $client "tools/call" @{
                name      = "click_element"
                arguments = @{
                    window         = (Get-WindowArg $np)
                    element_index  = $editIndex
                    mouse_button   = "left"
                    click_count    = 1
                    input_mode     = "foreground"
                }
            } 30000
            $clickRes = Get-ToolPayload $resp
            Save-Json "click_element" $clickRes
            Add-Row "click_element" "pass" ("backend={0} foreground" -f $clickRes.backend)
        } catch {
            $msg = $_.Exception.Message
            if ($msg -match "bounds changed") {
                try {
                    $null = Get-ToolPayload (Send-Rpc $client "tools/call" @{
                        name = "get_window_state"
                        arguments = @{ window = (Get-WindowArg $np); include_screenshot = $true; include_text = $true; input_mode = "foreground" }
                    } 45000)
                    $resp = Send-Rpc $client "tools/call" @{
                        name = "click_element"
                        arguments = @{ window = (Get-WindowArg $np); element_index = $editIndex; mouse_button = "left"; click_count = 1; input_mode = "foreground" }
                    } 30000
                    $clickRes = Get-ToolPayload $resp
                    Add-Row "click_element" "pass" ("backend={0} after bounds refresh" -f $clickRes.backend)
                } catch {
                    Add-Row "click_element" "fail" $_.Exception.Message
                }
            } else {
                Add-Row "click_element" "fail" $msg
            }
        }

        try {
            $resp = Send-Rpc $client "tools/call" @{
                name      = "type_text"
                arguments = @{
                    window     = (Get-WindowArg $np)
                    text       = $sentinelType
                    input_mode = "foreground"
                }
            } 30000
            $typeRes = Get-ToolPayload $resp
            Save-Json "type_text_fg" $typeRes
            $resp2 = Send-Rpc $client "tools/call" @{
                name      = "get_window_state"
                arguments = @{
                    window             = (Get-WindowArg $np)
                    include_screenshot = $false
                    include_text       = $true
                    input_mode         = "foreground"
                }
            } 30000
            $again = Get-ToolPayload $resp2
            Save-Json "after_type_text_fg" $again
            if (-not (Text-Has $again $sentinelType)) {
                throw ("typed text not in UIA tree. backend={0} doc={1}" -f $typeRes.backend, $again.accessibility.document_text)
            }
            Add-Row "type_text_foreground" "pass" ("backend={0}; read back {1}" -f $typeRes.backend, $sentinelType)
        } catch {
            Add-Row "type_text_foreground" "fail" $_.Exception.Message
        }

        try {
            $resp = Send-Rpc $client "tools/call" @{
                name      = "press_key"
                arguments = @{
                    window     = (Get-WindowArg $np)
                    key        = "End"
                    input_mode = "foreground"
                }
            } 20000
            $keyRes = Get-ToolPayload $resp
            Save-Json "press_key" $keyRes
            Add-Row "press_key" "pass" ("backend={0} key=End" -f $keyRes.backend)
        } catch {
            Add-Row "press_key" "fail" $_.Exception.Message
        }

        try {
            $resp = Send-Rpc $client "tools/call" @{
                name      = "scroll"
                arguments = @{
                    window     = (Get-WindowArg $np)
                    x          = 20
                    y          = 20
                    scrollX    = 0
                    scrollY    = 2
                    input_mode = "foreground"
                }
            } 20000
            $sc = Get-ToolPayload $resp
            Save-Json "scroll" $sc
            Add-Row "scroll" "pass" ("backend={0} scrollY=2" -f $sc.backend)
        } catch {
            $msg = $_.Exception.Message
            if ($msg -match "bounds changed") {
                try {
                    $null = Get-ToolPayload (Send-Rpc $client "tools/call" @{
                        name = "get_window_state"
                        arguments = @{ window = (Get-WindowArg $np); include_screenshot = $true; include_text = $false; input_mode = "foreground" }
                    } 45000)
                    $resp = Send-Rpc $client "tools/call" @{
                        name = "scroll"
                        arguments = @{ window = (Get-WindowArg $np); x = 20; y = 20; scrollY = 2; input_mode = "foreground" }
                    } 20000
                    $sc = Get-ToolPayload $resp
                    Add-Row "scroll" "pass" ("backend={0} after bounds refresh" -f $sc.backend)
                } catch {
                    Add-Row "scroll" "fail" $_.Exception.Message
                }
            } else {
                Add-Row "scroll" "fail" $msg
            }
        }

        # Background vs foreground: capture + UIA type without SendInput.
        try {
            $resp = Send-Rpc $client "tools/call" @{
                name      = "get_window_state"
                arguments = @{
                    window             = (Get-WindowArg $np)
                    include_screenshot = $true
                    include_text       = $true
                    input_mode         = "background"
                }
            } 60000
            $shots = @(Save-Images $resp "bg-state")
            $bg = Get-ToolPayload $resp
            Save-Json "get_window_state_bg" $bg
            $respD = Send-Rpc $client "tools/call" @{ name = "diagnostic_state"; arguments = @{} } 20000
            $diag = Get-ToolPayload $respD
            Save-Json "diagnostic_bg" $diag
            $pathName = [string]$diag.cacheDiagnostics.lastCapturePath
            $detail = [string]$diag.cacheDiagnostics.lastCapturePathDetail
            $pngOk = ($shots.Count -ge 1) -and (Test-Png $shots[0])
            if (-not $pngOk) { throw "background screenshot missing" }
            if ($bg.input_mode -and [string]$bg.input_mode -ne "background") {
                throw ("input_mode echoed as " + $bg.input_mode)
            }
            Add-Row "background_capture" "pass" ("resultBackend={0} lastCapturePath={1} {2}" -f $bg.backend, $pathName, $detail)
            if ($pathName -eq "wgc_window") {
                Add-Row "background_wgc" "pass" "CreateForWindow"
            } else {
                Add-Row "background_wgc" "skip" ("background capture used {0} instead of WGC CreateForWindow. {1}" -f $pathName, $detail)
            }
        } catch {
            $msg = $_.Exception.Message
            $envHint = $msg -match 'D3D11|CreateForWindow|PrintWindow|0x8007|not supported|GPU|desktop|GetDC'
            if ($envHint) {
                Add-Row "background_capture" "skip" ("hosted runner could not capture a background frame: " + $msg)
                Add-Row "background_wgc" "skip" $msg
            } else {
                Add-Row "background_capture" "fail" $msg
                Add-Row "background_wgc" "skip" $msg
            }
        }

        try {
            # Refresh the tree in background so the editable cache is current, then type.
            $null = Get-ToolPayload (Send-Rpc $client "tools/call" @{
                name = "get_window_state"
                arguments = @{ window = (Get-WindowArg $np); include_screenshot = $false; include_text = $true; input_mode = "background" }
            } 30000)
            $resp = Send-Rpc $client "tools/call" @{
                name      = "type_text"
                arguments = @{
                    window     = (Get-WindowArg $np)
                    text       = $sentinelBg
                    input_mode = "background"
                }
            } 30000
            $typeRes = Get-ToolPayload $resp
            Save-Json "type_text_bg" $typeRes
            $resp2 = Send-Rpc $client "tools/call" @{
                name      = "get_window_state"
                arguments = @{
                    window             = (Get-WindowArg $np)
                    include_screenshot = $false
                    include_text       = $true
                    input_mode         = "background"
                }
            } 30000
            $again = Get-ToolPayload $resp2
            Save-Json "after_type_text_bg" $again
            if ([string]$typeRes.backend -eq "sendinput") {
                throw "background type_text fell through to SendInput"
            }
            if (-not (Text-Has $again $sentinelBg)) {
                throw ("background text not in UIA tree. backend={0} doc={1}" -f $typeRes.backend, $again.accessibility.document_text)
            }
            Add-Row "background_type_text" "pass" ("backend={0}; read back {1} (no SendInput)" -f $typeRes.backend, $sentinelBg)
        } catch {
            Add-Row "background_type_text" "fail" $_.Exception.Message
        }
    } elseif (-not $hardStop) {
        foreach ($n in @("set_value", "click_element", "type_text_foreground", "press_key", "scroll", "background_capture", "background_type_text")) {
            Add-Row $n "skip" "notepad edit element was not available"
        }
    }

    # Single-monitor scope in a second process so OCU_MONITOR is applied at startup.
    if (-not $hardStop -and $np) {
        try {
            $scope = New-OcuClient "scope" @{ OCU_MONITOR = "0" }
            $init = Send-Rpc $scope "initialize" @{
                protocolVersion = "2024-11-05"
                capabilities    = @{}
                clientInfo      = @{ name = "ocu-ci-scope"; version = "0.1.0" }
            } 15000
            if ($init.error) { throw $init.error.message }
            $scope.In.WriteLine((@{ jsonrpc = "2.0"; method = "notifications/initialized" } | ConvertTo-Json -Compress))
            $disp = Get-ToolPayload (Send-Rpc $scope "tools/call" @{ name = "list_displays"; arguments = @{} } 20000)
            Save-Json "scope_displays" $disp
            $mon = @($disp) | Where-Object { [int]$_.index -eq 0 } | Select-Object -First 1
            if (-not $mon) { throw "display index 0 missing under OCU_MONITOR=0" }
            $wins = @(Get-ToolPayload (Send-Rpc $scope "tools/call" @{ name = "list_windows"; arguments = @{} } 20000))
            Save-Json "scope_windows" $wins
            $scopedNp = $wins | Where-Object { [int64]$_.id -eq [int64]$np.id } | Select-Object -First 1
            if (-not $scopedNp) {
                $scopedNp = $wins | Where-Object { ([string]$_.app) -match '(?i)notepad' } | Select-Object -First 1
            }
            if (-not $scopedNp) { throw "notepad not listed while scoped to monitor 0" }
            $diag = Get-ToolPayload (Send-Rpc $scope "tools/call" @{ name = "diagnostic_state"; arguments = @{} } 15000)
            Save-Json "scope_diagnostic" $diag
            $scopeLabel = [string]$diag.cacheDiagnostics.scopeMonitor
            Add-Row "monitor_scope" "pass" ("OCU_MONITOR=0 scopeMonitor={0} device={1}" -f $scopeLabel, $mon.deviceName)

            $hwnd = [int64]$scopedNp.id
            $before = Get-WinRect $hwnd
            if (-not $before) { throw "GetWindowRect failed" }
            $destX = [int]$mon.rect.right + 800
            $destY = [int]$mon.rect.top + 40
            $flags = [uint32](0x0001 -bor 0x0004 -bor 0x0010) # NOSIZE|NOZORDER|NOACTIVATE
            $moved = [OcuWin]::SetWindowPos([IntPtr]$hwnd, [IntPtr]::Zero, $destX, $destY, 0, 0, $flags)
            Start-Sleep -Milliseconds 400
            $after = Get-WinRect $hwnd
            $center = 0
            if ($after) { $center = [int](($after.Left + $after.Right) / 2) }
            $monRight = [int]$mon.rect.right
            if (-not $moved -or -not $after -or $center -lt $monRight) {
                Add-Row "out_of_scope" "skip" ("could not place notepad outside monitor 0 (SetWindowPos moved={0} centerX={1} monRight={2}); OS likely clamped the only desktop" -f $moved, $center, $monRight)
            } else {
                $gw = Send-Rpc $scope "tools/call" @{
                    name      = "get_window"
                    arguments = @{ id = $hwnd; app = [string]$scopedNp.app }
                } 20000
                $errText = Get-ToolText $gw
                Save-Json "out_of_scope_get_window" @{ isError = [bool]$gw.result.isError; text = $errText }
                if ($gw.result.isError -and $errText -match "out_of_scope") {
                    Add-Row "out_of_scope" "pass" $errText
                } elseif (-not $gw.result.isError) {
                    Add-Row "out_of_scope" "fail" "get_window succeeded for a window placed outside monitor 0"
                } else {
                    Add-Row "out_of_scope" "fail" $errText
                }
            }
            if ($before) {
                [void][OcuWin]::SetWindowPos([IntPtr]$hwnd, [IntPtr]::Zero, [int]$before.Left, [int]$before.Top, 0, 0, $flags)
            }
        } catch {
            Add-Row "monitor_scope" "fail" $_.Exception.Message
            if (-not ($script:Rows | Where-Object { $_.Name -eq "out_of_scope" })) {
                Add-Row "out_of_scope" "fail" $_.Exception.Message
            }
        }
    }

    if ($paint) {
        Add-Row "mspaint" "pass" ("launched pid={0} (available as a second top-level window)" -f $paint.Id)
    } else {
        Add-Row "mspaint" "skip" "mspaint.exe not launched"
    }
} catch {
    Add-Row "harness" "fail" $_.Exception.Message
} finally {
    Close-OcuClient $client
    Close-OcuClient $scope
    Get-Process notepad, mspaint -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

    $fails = @($script:Rows | Where-Object { $_.Status -eq "fail" }).Count
    $passes = @($script:Rows | Where-Object { $_.Status -eq "pass" }).Count
    $skips = @($script:Rows | Where-Object { $_.Status -eq "skip" }).Count
    $osName = "unknown"
    try { $osName = (Get-CimInstance Win32_OperatingSystem).Caption } catch {}
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("## ocu-replica live GUI")
    $lines.Add("")
    $lines.Add("- runner: ``$($env:RUNNER_NAME)``  OS: $osName")
    $lines.Add("- run: $($env:GITHUB_SERVER_URL)/$($env:GITHUB_REPOSITORY)/actions/runs/$($env:GITHUB_RUN_ID)")
    $lines.Add("- pass=$passes fail=$fails skip=$skips")
    $lines.Add("")
    $lines.Add("| Capability | Result | Detail |")
    $lines.Add("|---|---|---|")
    foreach ($r in $script:Rows) {
        $detail = ($r.Detail -replace "\|", "/")
        $lines.Add(("| ``{0}`` | {1} | {2} |" -f $r.Name, $r.Status, $detail))
    }
    $lines.Add("")
    $md = $lines -join "`n"
    Set-Content -Path (Join-Path $OutDir "summary.md") -Value $md -Encoding utf8
    if ($env:GITHUB_STEP_SUMMARY) {
        Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value $md -Encoding utf8
    }
    Write-Host $md
    if ($fails -gt 0) { exit 1 } else { exit 0 }
}
