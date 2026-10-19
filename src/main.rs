//! ocu-replica — clean-room Windows computer-use helper.
//!
//! Modes:
//!   (default) MCP stdio JSON-RPC 2.0
//!   --stdio-native   newline-delimited {id,method,params,meta}
//!   --parent-pid N   exit when parent process disappears
//!   --help

mod activity;
mod blocklist;
mod cdp;
mod engine;
mod mode;
mod monitor;
mod geometry;
mod image;
mod keys;
mod mcp;
mod policy;
mod protocol;

#[cfg(windows)]
mod win;

use engine::{Engine, EngineOutcome, NullBackend};
use policy::PersistScope;
use serde_json::{json, Map, Value};
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn print_help() {
    eprintln!(
        "ocu-native {} - retained Windows native controls\n\n\
         Use runtime/supervisor.py for MCP stdio with timeout and cancellation.\n\
         Direct helper options: --mcp, --stdio-native, --parent-pid N, --help.\n\
         Background mode only; explicit element_index required for input.\n\
         State: OCU_REPLICA_HOME. Image size: OCU_REPLICA_MAX_EDGE.\n\
         App approvals: normal first-use prompt, or caller-supplied OCU_PREAPPROVE.\n\
         CDP, audio, global keys, dragging, coordinate clicks and foreground fallback are not exposed.",
        env!("CARGO_PKG_VERSION")
    );
}

struct Args {
    native: bool,
    parent_pid: Option<u32>,
}

fn parse_args() -> Args {
    let mut native = false;
    let mut parent_pid = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--stdio-native" => native = true,
            "--mcp" => native = false,
            "--parent-pid" => {
                let v = it.next().unwrap_or_else(|| {
                    eprintln!("missing value for --parent-pid");
                    std::process::exit(2);
                });
                parent_pid = Some(v.parse().unwrap_or_else(|_| {
                    eprintln!("invalid --parent-pid");
                    std::process::exit(2);
                }));
            }
            other if other.starts_with('-') => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
            _ => {}
        }
    }
    Args { native, parent_pid }
}

fn build_engine(exit_on_esc: bool) -> Engine {
    #[cfg(windows)]
    {
        let flag = Arc::new(Mutex::new(false));
        let activity = activity::ActivityTracker::new();
        match win::WinBackend::start(flag.clone(), activity.clone()) {
            Ok(backend) => {
                let mut eng = Engine::with_activity(Box::new(backend), exit_on_esc, activity);
                // Watch the interrupt flag from the hook thread.
                eng.interrupts.latched = false;
                // Store flag on a side channel via env isn't great; poll in the loop.
                // We stash it in a global for the stdio pumps to check.
                INTERRUPT_FLAG.store(true, Ordering::SeqCst); // marker that hooks are live
                HOOK_FLAG.with(|f| *f.borrow_mut() = Some(flag));
                return eng;
            }
            Err(e) => {
                eprintln!("ocu-replica: Windows backend init failed ({e}); using null backend");
            }
        }
    }
    Engine::new(Box::new(NullBackend::default()), exit_on_esc)
}

thread_local! {
    static HOOK_FLAG: std::cell::RefCell<Option<Arc<Mutex<bool>>>> = const { std::cell::RefCell::new(None) };
}
static INTERRUPT_FLAG: AtomicBool = AtomicBool::new(false);

fn poll_esc(engine: &mut Engine) {
    HOOK_FLAG.with(|f| {
        if let Some(flag) = f.borrow().as_ref() {
            if let Ok(mut g) = flag.lock() {
                if *g {
                    *g = false;
                    engine.on_physical_esc();
                }
            }
        }
    });
}

fn main() {
    let args = parse_args();
    if let Some(pid) = args.parent_pid {
        spawn_parent_watcher(pid);
    }
    let mut engine = build_engine(args.native);
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    if args.native {
        run_native(&mut engine, stdin.lock(), &mut stdout);
    } else {
        run_mcp(&mut engine, stdin.lock(), &mut stdout);
    }
    if let Some(code) = engine.exit_code {
        std::process::exit(code);
    }
}

fn spawn_parent_watcher(pid: u32) {
    std::thread::spawn(move || loop {
        if !parent_alive(pid) {
            std::process::exit(0);
        }
        std::thread::sleep(Duration::from_secs(1));
    });
}

fn parent_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => {
                    let _ = CloseHandle(h);
                    true
                }
                Err(_) => false,
            }
        }
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

fn run_native(engine: &mut Engine, mut stdin: impl BufRead, stdout: &mut impl Write) {
    let mut line = String::new();
    loop {
        poll_esc(engine);
        if let Some(code) = engine.exit_code {
            let _ = writeln!(stdout, "{}", json!({"id":null,"ok":false,"error": engine::ESC_MSG}));
            let _ = stdout.flush();
            std::process::exit(code);
        }
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("stdin error: {e}");
                break;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match protocol::parse_native_line(trimmed) {
            Ok(req) => {
                let out = match engine.handle(&req.method, &req.params, &req.meta) {
                    EngineOutcome::Ok(v) => protocol::native_ok(&req.id, v),
                    EngineOutcome::Err(m) => protocol::native_err(&req.id, &m),
                    EngineOutcome::Approval(a) => protocol::native_approval(&req.id, &a),
                };
                let _ = writeln!(stdout, "{}", out);
                let _ = stdout.flush();
                if req.method == "close" {
                    break;
                }
                if let Some(code) = engine.exit_code {
                    std::process::exit(code);
                }
            }
            Err((id, msg)) => {
                let id = id.unwrap_or(Value::Null);
                let _ = writeln!(stdout, "{}", protocol::native_err(&id, &msg));
                let _ = stdout.flush();
            }
        }
    }
}

fn run_mcp(engine: &mut Engine, mut stdin: impl BufRead, stdout: &mut impl Write) {
    let mut line = String::new();
    loop {
        poll_esc(engine);
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match mcp::parse_rpc_line(trimmed) {
            Ok(None) => continue, // notification
            Ok(Some((id, method, params))) => {
                let resp = handle_mcp(engine, &id, &method, &params);
                let _ = writeln!(stdout, "{}", resp);
                let _ = stdout.flush();
                if method == "tools/call" {
                    if let Ok((name, _, _)) = mcp::tools_call_parts(&params) {
                        if name == "close" {
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                let _ = writeln!(stdout, "{}", mcp::rpc_err(&Value::Null, -32700, &e));
                let _ = stdout.flush();
            }
        }
    }
}

fn handle_mcp(engine: &mut Engine, id: &Value, method: &str, params: &Value) -> Value {
    match method {
        "initialize" => {
            let client = params.get("protocolVersion").and_then(Value::as_str);
            mcp::rpc_ok(id, mcp::initialize_result(client))
        }
        "ping" => mcp::rpc_ok(id, json!({})),
        "tools/list" => mcp::rpc_ok(id, mcp::tools_list()),
        "tools/call" => match mcp::tools_call_parts(params) {
            Err(e) => mcp::rpc_ok(id, mcp::tool_error_content(&e)),
            Ok((name, args, meta)) => {
                // MCP clients may grant approval by passing _meta.persist / approved app.
                if let Some(app) = protocol::approved_app(&meta) {
                    let scope = match meta.get("persist").and_then(Value::as_str) {
                        Some("always") => PersistScope::Always,
                        _ => PersistScope::Session,
                    };
                    engine.policy.grant(&app, scope);
                }
                match engine.handle(&name, &args, &meta) {
                    EngineOutcome::Ok(v) => mcp::rpc_ok(id, mcp::tool_result_content(&v)),
                    EngineOutcome::Err(m) => mcp::rpc_ok(id, mcp::tool_error_content(&m)),
                    EngineOutcome::Approval(a) => {
                        // Surface as a tool error carrying the approval payload so
                        // MCP hosts without elicitation can still see it.
                        let msg = format!(
                            "approval required for app '{}' (displayName='{}', riskLevel='{}', allowPersistentApproval={}). Re-call with meta x-oai-cua-approved-app set after the user accepts.",
                            a.app, a.display_name, a.risk_level, a.allow_persistent_approval
                        );
                        let mut content = mcp::tool_error_content(&msg);
                        content["approvalRequest"] = json!(a);
                        mcp::rpc_ok(id, content)
                    }
                }
            }
        },
        "notifications/initialized" | "initialized" => {
            // Should have been filtered as notification; ignore.
            mcp::rpc_ok(id, json!({}))
        }
        other => mcp::rpc_err(id, -32601, &format!("method not found: {other}")),
    }
}

// Re-export for native loop.
