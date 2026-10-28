#![recursion_limit = "512"]

mod capture;
mod clipboard;
mod ffi;
mod input;
mod overlay;
mod uia;
mod windows;

use std::io::{BufRead, Write};

use base64::Engine as _;
use serde_json::{json, Value};
use ffi::SetProcessDPIAware;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_IMAGES: usize = 6;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

fn main() {
    unsafe {
        SetProcessDPIAware();
    }
    overlay::start();
    eprintln!("winch {VERSION} ready");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}});
                let _ = writeln!(out, "{err}");
                let _ = out.flush();
                continue;
            }
        };
        if let Some(resp) = handle(&req) {
            let _ = writeln!(out, "{resp}");
            let _ = out.flush();
        }
    }
}

fn handle(req: &Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));

    // notifications have no id and must not be answered
    if id.is_none() {
        return None;
    }
    let id = id.unwrap();

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "winch", "version": VERSION}
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_defs()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match name {
                "winch_run" => Ok(run_steps(&args)),
                "winch_health" => Ok(health()),
                "winch_end" => {
                    let stopped = overlay::is_cancelled();
                    overlay::hide_now();
                    Ok(json!({"content":[{"type":"text","text": json!({"ok": true, "stopped_by_user": stopped}).to_string()}]}))
                }
                other => Err(format!("unknown tool: {other}")),
            }
        }
        other => Err(format!("unknown method: {other}")),
    };

    Some(match result {
        Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
        Err(e) => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":e}}),
    })
}

fn health() -> Value {
    let wins = windows::list_windows();
    let text = json!({
        "ok": true,
        "platform": "windows",
        "version": VERSION,
        "pid": std::process::id(),
        "windows": wins.len(),
        "accessibility": uia::available(),
        "note": "screenshot + UIA element tree + SendInput; window-relative coordinates"
    });
    json!({"content":[{"type":"text","text":text.to_string()}]})
}

fn is_browser(w: &windows::WindowInfo) -> bool {
    let app = w.app.to_ascii_lowercase();
    let class = w.class.as_deref().unwrap_or("").to_ascii_lowercase();
    // Only real browsers: Electron apps share Chrome_WidgetWin_1, and a
    // "extensions" key would then eat VS Code's Extensions pane.
    ["msedge", "chrome", "chromium", "firefox", "brave", "vivaldi", "opera"]
        .iter()
        .any(|b| app.contains(b))
        || class.contains("mozillawindowclass")
}

fn filter_empty(v: &Value) -> bool {
    match v {
        Value::String(s) => s.trim().is_empty() || s.trim().eq_ignore_ascii_case("all"),
        Value::Array(a) => a.is_empty(),
        _ => true,
    }
}

fn filter_hint(
    hidden: &[(String, usize)],
    merged: usize,
    char_capped: bool,
    keep_hits: usize,
    keep_set: bool,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if !hidden.is_empty() {
        let sets: Vec<String> = hidden.iter().map(|(k, n)| format!("{k}={n}")).collect();
        parts.push(format!("hidden by set: {}", sets.join(", ")));
    }
    if merged > 0 {
        parts.push(format!("{merged} duplicate wrapper nodes merged"));
    }
    if char_capped {
        parts.push("char cap hit — raise or disable with max_chars".into());
    }
    if keep_set {
        if keep_hits == 0 {
            parts.push("keep matched no element: nothing was whitelisted (check the key or drop keep)".into());
        } else {
            parts.push(format!(
                "keep in effect ({keep_hits} matches): everything outside those sets is omitted"
            ));
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.push(
        "hidden elements keep working via click {name:\"...\"}; state {full:true} emits everything"
            .into(),
    );
    Some(parts.join("; "))
}

fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "winch_run",
            "description": "Run Windows desktop steps in ONE call. Coordinates are window-relative. ops: list_windows, launch, activate, show, state, click, set_value, type, press, scroll, drag, tray, clipboard, wait, close.\n\nOBSERVE THEN ACT: call state first. It returns the accessibility tree as lines `[index] Role \"name\" (x,y,WxH) flags` plus a screenshot, and a `snapshot` number. Act on ONE control, then state again. element_index is bound to the snapshot that produced it: every input step and every new state invalidates it, and using a stale index or stale `snapshot` is an error — re-observe instead of retrying.\n\nMODE: the default `verified` enforces that discipline. Set mode:\"batch\" when you already know the layout (from this conversation or a state you just took) and the steps do not open a new panel/dialog: indexes then stay valid through the whole call, and each indexed click re-reads the element's live rect before clicking. Batch saves the ~200ms state between every action; it does not make stale guesses safe, so use verified for anything exploratory.\n\nPREFER element_index over pixels: the server resolves it to the centre of that control and reports its role/name. When you must pass x/y, aim at the visual CENTER of the control, never its edge — adjacent toolbar icons, tabs and text fields can be 15-20px apart and an edge coordinate lands on the neighbour.\n\nset_value replaces an editable field's text through UIA ValuePattern (no focus or keystrokes needed). state {include_text:false} skips the tree (faster, but then no element_index). If a modal dialog is not inside the target window's tree, list_windows and state that dialog as its own window. A popup that is its own top-level window (search dropdown, combo list) can still be clicked from the window that opened it: same-process popups are allowed, and the click result reports landed_on.\n\nclick also accepts {name:\"...\"} when exactly one visible control matches. If an app seems to have no window because it lives in the notification area, run {op:\"list_windows\", include_hidden:true} first: a tray app usually keeps its window alive but hidden, and it appears with visible:false. list_windows returns {total, shown, windows}; when the result is truncated it says so, and the fix is a narrower filter (app, title or class), not paging through everything. Then {op:\"show\", window:{app:\"...\"}} restores it — show revives an existing hidden or minimized window, and when the app destroyed its window it re-runs the exe so a single-instance app shows its own window (nothing is killed). show verifies visibility before returning, but a window can be visible and still deaf (dead renderer) — see STUCK WINDOWS below. {op:\"tray\", name:\"QQ\"} clicks a notification-area icon when the window really is gone; {op:\"tray\"} alone lists icons, but classic (non-XAML) icons have no accessible name — prefer show. Use restart:true only as a last resort (it stops the instance). Set quiet:true for background/test runs — it suppresses the on-screen takeover banner.\n\nWindow refs: {id: <hwnd>} or {title_contains: \"...\"} or {app: \"clash-verge\"}. SAFETY: input to terminal windows (WindowsTerminal, conhost, cmd, powershell, WSL) and any Windows/Meta key chord is refused.\n\nSTUCK WINDOWS: a window that paints its last frame but ignores every click is a dead renderer, not a bad coordinate — common after a Chromium app (QQ, WeChat) stayed hidden for a long time. Do not keep clicking. Call op show with window.app once (it reopens through the app itself), and if it is still dead call op launch with app set to the exe path and restart:true. Never use show with a window id on a hidden Chromium window: that is what resurrects a dead frame.\n\nSTATE SET FILTERS (token control): never fight noise with long atom lists. name a SET and let the server subtract it. state {skip:[\"收藏夹栏\"]} drops that container and everything under it, reporting one line `[--] ToolBar \"收藏夹栏\" (…) hidden=12 set=收藏夹栏`. Keys are name substrings (case-insensitive), `role:Role`, or the presets bookmarks / extensions; {keep:[…]} is the whitelist (default \"all\") and the tree is keep minus skip. Matching a container applies to its whole subtree, so one key can stand for a hundred elements. Browser windows already drop bookmarks + extensions by default: {defaults:false} keeps them, {full:true} disables all filtering and duplicate merging. Identical wrapper/inner pairs (Chromium ListItem \"X\" + Button \"X\" on the same rect) are merged to one line. max_chars (default 14000) caps the tree text. Hidden elements have no element_index but click {name:\"...\"} still finds them in the live tree, so pruning never blocks an action.\n\nUNNAMED ICON BUTTONS: an icon-only control with no accessible name borrows its tooltip as the name (`Button \"Copy\"`); if it still prints `Button \"\"`, click it by element_index — the tree gives its rect, often `focus`, and the Text right beside it says what it belongs to. Never resolve copy/export icons with guessed pixels. After a copy, verify with {op:\"clipboard\"} (no text reads it, text:\"…\" writes it) instead of assuming; when the value is also on screen as Text, read it from the tree and skip the clipboard entirely.

FINISH — MANDATORY: as soon as the desktop work for the user's request is done — success, failure, or the user pressed Esc — call winch_end ONCE, then report the outcome. Never leave a session open: the takeover banner stays on screen until winch_end (it only auto-hides after 30s idle), and the user is left thinking the machine is still under control.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 40,
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": {"type": "string", "enum": ["list_windows","launch","activate","show","state","click","set_value","type","press","scroll","drag","tray","clipboard","wait","close"]},
                                "window": {"type": "object", "description": "{id} | {title_contains} | {app}"},
                                "app": {"type": "string", "description": "exe path for launch"},
                                "restart": {"type": "boolean", "default": false, "description": "launch: stop the running instance first (last resort for tray-only single-instance apps)"},
                                "reactivate": {"type": "boolean", "default": false, "description": "launch: when the app is already running, bring its hidden/minimized window back instead of returning already_running"},
                                "include_hidden": {"type": "boolean", "default": false, "description": "list_windows: also list hidden windows (tray/background), each with visible:true|false"},
                                "filter": {"type": "string", "description": "list_windows: case-insensitive substring over app, title and window class"},
                                "limit": {"type": "integer", "default": 80, "description": "list_windows: max windows per page (1-500)"},
                                "offset": {"type": "integer", "default": 0, "description": "list_windows: skip this many matches (use only after a filter still returns too many)"},
                                "screenshot": {"type": "boolean", "default": true},
                                "include_text": {"type": "boolean", "default": true, "description": "state: include the UIA element tree (needed for element_index)"},
                                "max_nodes": {"type": "integer", "default": 300, "description": "state: cap on emitted elements"},
                                "max_chars": {"type": "integer", "default": 14000, "description": "state: character cap on the tree text; 0 disables it"},
                                "max_depth": {"type": "integer", "default": 14, "description": "state: depth cap on the UIA walk"},
                                "skip": {
                                    "type": ["string", "array", "null"],
                                    "default": null,
                                    "description": "state: blacklist, null by default. Each key names a SET: matching a container drops its whole subtree and reports one `hidden=N set=key` line, so one key replaces many atoms. Keys: name substring (\"收藏夹栏\", \"Billing\"), role:Role, or the presets bookmarks / extensions. Example: [\"Billing\", \"role:ScrollBar\"]"
                                },
                                "keep": {
                                    "type": ["string", "array"],
                                    "default": "all",
                                    "description": "state: whitelist, \"all\" by default. Same key syntax; matching a container keeps its whole subtree. Result = keep - skip."
                                },
                                "full": {"type": "boolean", "default": false, "description": "state: ignore skip/keep, browser defaults and duplicate merging — every node, max tokens"},
                                "defaults": {"type": "boolean", "default": true, "description": "state: on a browser window apply the built-in blacklist (bookmarks bar + extension buttons)"},
                                "element_index": {"type": "integer", "description": "click/set_value: index from the latest state, e.g. 12"},
                                "name": {"type": "string", "description": "click: visible control name (must be unambiguous); tray: icon caption (omit on tray to list the icons)"},
                                "snapshot": {"type": "integer", "description": "optional: the snapshot the element_index came from; stale snapshots are rejected"},
                                "value": {"type": "string", "description": "set_value: replacement text"},
                                "max_width": {"type": "integer", "default": 1280},
                                "x": {"type": "integer", "description": "window-relative X; use the center of the target control"},
                                "y": {"type": "integer", "description": "window-relative Y; use the center of the target control"},
                                "from_x": {"type": "integer"}, "from_y": {"type": "integer"},
                                "to_x": {"type": "integer"}, "to_y": {"type": "integer"},
                                "button": {"type": "string", "enum": ["left","right","middle"]},
                                "count": {"type": "integer", "default": 1},
                                "text": {"type": "string", "description": "type: text to type; clipboard: text to put on the clipboard (omit to read it)"},
                                "key": {"type": "string", "description": "chord, e.g. Control_L+a"},
                                "scroll_y": {"type": "integer"},
                                "ms": {"type": "integer"}
                            },
                            "required": ["op"]
                        }
                    },
                    "stop_on_error": {"type": "boolean", "default": true},
                    "mode": {
                        "type": "string",
                        "enum": ["verified", "batch"],
                        "default": "verified",
                        "description": "verified: every input invalidates element indexes (forces re-observation). batch: indexes stay valid for the whole call — use when this conversation already established the layout and no step opens a new panel/dialog"
                    }
                },
                "required": ["steps"]
            }
        }),
        json!({
            "name": "winch_health",
            "description": "Report backend availability: window count, pid, version.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "winch_end",
            "description": "REQUIRED at the end of every computer-use task: call this as the LAST action after you finish (success, failure, or the user pressed Esc). It ends the session, clears the user-abort flag and removes the on-screen takeover banner immediately. Safe to call at any time, idempotent.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
    ]
}

fn run_steps(args: &Value) -> Value {
    let steps = args.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
    if steps.is_empty() {
        return json!({"content":[{"type":"text","text":"winch_run needs a non-empty steps array"}],"isError":true});
    }
    let stop_on_error = args.get("stop_on_error").and_then(Value::as_bool).unwrap_or(true);
    // Quiet runs (tests, background probes) must not paint the takeover
    // banner on the user's screen.
    let quiet = args.get("quiet").and_then(Value::as_bool).unwrap_or(false);
    let batch = args
        .get("mode")
        .and_then(Value::as_str)
        .map(|m| m.eq_ignore_ascii_case("batch"))
        .unwrap_or(false);
    // Listing the tray is read-only; only a tray click takes over the desktop.
    let controls = steps.iter().any(|s| match s.get("op").and_then(Value::as_str).unwrap_or("") {
        "tray" => s.get("name").is_some(),
        op => matches!(
            op,
            "click" | "set_value" | "type" | "press" | "scroll" | "drag" | "launch" | "activate" | "show"
        ),
    });
    if controls && !quiet {
        overlay::reset_cancel();
        overlay::show();
    }
    let mut results: Vec<Value> = Vec::new();
    let mut images: Vec<String> = Vec::new();
    let mut image_bytes = 0usize;

    for (i, step) in steps.iter().enumerate() {
        let op = step.get("op").and_then(Value::as_str).unwrap_or("");
        if controls && overlay::is_cancelled() {
            results.push(json!({"step": i, "op": op, "ok": false,
                "error": "COMPUTER_USE_STOPPED_BY_USER: the user pressed Esc; do not retry desktop input in this turn"}));
            break;
        }
        let r = exec_step(op, step, &mut images, &mut image_bytes);
        match r {
            Ok(v) => {
                results.push(json!({"step": i, "op": op, "ok": true, "result": v}));
                // Codex-style discipline: indexes belong to one observation only.
                if !batch
                    && matches!(
                        op,
                        "click" | "set_value" | "type" | "press" | "scroll" | "drag" | "activate" | "launch" | "show" | "close" | "tray"
                    )
                {
                    uia::invalidate();
                }
            }
            Err(e) => {
                results.push(json!({"step": i, "op": op, "ok": false, "error": e}));
                if stop_on_error {
                    break;
                }
            }
        }
    }

    // Batch indexes survive the call but not beyond it.
    if batch {
        uia::invalidate();
    }

    // A control batch keeps the overlay up while it runs and for 30 seconds
    // after it ends; another batch calls show() again and cancels this timer.
    // (winch_end / Esc still close it immediately.)
    if controls && !quiet {
        overlay::hide_after(30_000);
    }

    let stopped = controls && overlay::is_cancelled();
    let mut summary = json!({
        "status": if stopped { "user_aborted" } else { "ok" },
        "stopped_by_user": stopped,
        "steps": results
    });
    if controls && !quiet {
        // Repeat the finish rule where the model actually reads it: the tool
        // result, not only the tool description.
        summary["finish"] = json!(
            "call winch_end now if this completed the user's desktop task (it removes the takeover banner)"
        );
    }
    let mut content = vec![json!({"type":"text","text":summary.to_string()})];
    for img in images {
        content.push(json!({"type":"image","data":img,"mimeType":"image/png"}));
    }
    json!({"content": content})
}

/// Codex CUA's non-negotiable Windows deny list: never drive terminals.
const TERMINAL_APPS: &[&str] = &[
    "windowsterminal",
    "conhost",
    "openconsole",
    "cmd",
    "powershell",
    "pwsh",
    "wt",
    "mintty",
    "bash",
    "wsl",
    "ubuntu",
    "debian",
];

fn guard(w: &windows::WindowInfo) -> Result<(), String> {
    windows::ensure_visible(w)?;
    let app = w.app.to_ascii_lowercase();
    if TERMINAL_APPS.contains(&app.as_str()) {
        return Err(format!(
            "refusing to send input to a terminal ({}) — Windows terminal automation is disabled",
            w.app
        ));
    }
    Ok(())
}

fn guard_key(key: &str) -> Result<(), String> {
    let forbidden = key.split('+').any(|k| {
        matches!(
            k.trim().to_ascii_lowercase().as_str(),
            "super" | "super_l" | "win" | "windows" | "meta" | "cmd" | "command" | "os"
        )
    });
    if forbidden {
        return Err("Windows/Meta key chords are disabled (they drive the shell and system UI)".into());
    }
    Ok(())
}

/// HWNDs that actually host notification-area icons: the taskbar plus its XAML
/// content bridge (the UIA tree of Shell_TrayWnd itself only has 2 panes).
fn tray_hosts() -> Vec<i64> {
    let mut hosts = Vec::new();
    for w in windows::list_windows() {
        let c = w.class.as_deref().unwrap_or("");
        if c != "Shell_TrayWnd" && !c.starts_with("Shell_SecondaryTrayWnd") {
            continue;
        }
        hosts.push(w.id);
        for child in windows::child_windows(w.id) {
            let cl = child.class.as_deref().unwrap_or("");
            if cl.contains("DesktopWindowContentBridge") || cl.contains("TrayNotifyWnd") {
                hosts.push(child.id);
            }
        }
    }
    hosts
}

fn tray_hits(hosts: &[i64], needle: &str) -> Vec<uia::Hit> {
    let needle = needle.to_lowercase();
    let mut out: Vec<uia::Hit> = Vec::new();
    for id in hosts {
        // XAML tray: UIA has the names.
        if let Ok(hits) = uia::find_in_window(*id as isize, &needle, 40) {
            for h in hits {
                if h.name.is_empty() || !h.name.to_lowercase().contains(&needle) {
                    continue;
                }
                if !out.iter().any(|o| o.x == h.x && o.y == h.y && o.name == h.name) {
                    out.push(h);
                }
            }
        }
    }
    out
}

fn tray_action(name: &str) -> Result<Value, String> {
    let hosts = tray_hosts();
    if name.is_empty() {
        let mut icons: Vec<Value> = Vec::new();
        for id in &hosts {
            if let Ok(hits) = uia::find_in_window(*id as isize, "", 200) {
                for h in hits {
                    if !icons.iter().any(|v: &Value| {
                        v["x"] == h.x && v["y"] == h.y && v["name"] == h.name
                    }) {
                        icons.push(json!({"name": h.name, "role": h.role, "x": h.x, "y": h.y}));
                    }
                }
            }
        }
        return Ok(json!({"icons": icons}));
    }

    if let Some(h) = tray_hits(&hosts, name).into_iter().next() {
        input::click_screen(h.x, h.y, "left", 1)?;
        return Ok(json!({"clicked": h.name, "via": "tray", "x": h.x, "y": h.y}));
    }

    // Hidden in the overflow: open it, then look in the flyout.
    let before: Vec<i64> = windows::list_windows().iter().map(|w| w.id).collect();
    let mut opened = false;
    'outer: for label in ["隐藏的图标", "hidden icons", "通知图标", "notification"] {
        if let Some(h) = tray_hits(&hosts, label).into_iter().next() {
            input::click_screen(h.x, h.y, "left", 1)?;
            opened = true;
            break 'outer;
        }
    }
    if !opened {
        let icons = tray_hits(&hosts, "");
        return Err(format!(
            "'{name}' is not on the visible tray and the hidden-icons button was not found; icons seen: {}",
            icons.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    std::thread::sleep(std::time::Duration::from_millis(700));

    let mut flyouts: Vec<i64> = windows::list_windows()
        .iter()
        .filter(|w| !before.contains(&w.id))
        .map(|w| w.id)
        .collect();
    for w in windows::list_windows_all() {
        let c = w.class.as_deref().unwrap_or("").to_lowercase();
        if c.contains("overflow") {
            flyouts.push(w.id);
        }
    }
    flyouts.sort_unstable();
    flyouts.dedup();

    if let Some(h) = tray_hits(&flyouts, name).into_iter().next() {
        input::click_screen(h.x, h.y, "left", 1)?;
        return Ok(json!({"clicked": h.name, "via": "overflow", "x": h.x, "y": h.y}));
    }
    Err(format!(
        "'{name}' was not in the overflow; flyout contents: {}",
        tray_hits(&flyouts, "").iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join(", ")
    ))
}

fn exec_step(op: &str, step: &Value, images: &mut Vec<String>, image_bytes: &mut usize) -> Result<Value, String> {
    match op {
        "list_windows" => {
            let filter = step.get("filter").and_then(Value::as_str).map(|s| s.to_lowercase());
            let include_hidden = step
                .get("include_hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut wins = if include_hidden {
                // Useful windows first: visible, then real UI (not a
                // WS_EX_TOOLWINDOW helper), then grouped by app.
                let mut all = windows::list_windows_all();
                all.sort_by(|a, b| {
                    b.visible
                        .cmp(&a.visible)
                        .then(a.tool_window.cmp(&b.tool_window))
                        .then(a.app.cmp(&b.app))
                        .then(a.title.cmp(&b.title))
                });
                all
            } else {
                windows::list_windows()
            };
            if let Some(f) = filter {
                // Match app, title and window class, so "chrome_widgetwin",
                // "message" or "weixin" all narrow the list.
                wins.retain(|w| {
                    w.app.to_lowercase().contains(&f)
                        || w.title.to_lowercase().contains(&f)
                        || w.class.as_deref().unwrap_or("").to_lowercase().contains(&f)
                });
            }
            let total = wins.len();
            let limit = step
                .get("limit")
                .and_then(Value::as_i64)
                .unwrap_or(80)
                .clamp(1, 500) as usize;
            let offset = step
                .get("offset")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0) as usize;
            let page: Vec<_> = wins.into_iter().skip(offset).take(limit).collect();
            let shown = page.len();
            let mut out = json!({
                "total": total,
                "offset": offset,
                "shown": shown,
                "windows": page,
            });
            if offset + shown < total {
                out["truncated"] = json!(true);
                out["hint"] = json!(format!(
                    "{total} windows matched; showing {shown} from offset {offset}. Narrow it with filter (matches app, title and window class) instead of paging through everything."
                ));
            }
            Ok(out)
        }
        "launch" => {
            let app = step.get("app").and_then(Value::as_str).ok_or("launch needs app (exe path)")?;
            let restart = step.get("restart").and_then(Value::as_bool).unwrap_or(false);
            let reactivate = step.get("reactivate").and_then(Value::as_bool).unwrap_or(false);
            windows::launch(app, restart, reactivate)
        }
        "show" => {
            let w = step.get("window").ok_or("show needs window")?;
            windows::show_ref(w)
        }
        "activate" => {
            let w = windows::resolve(step.get("window").ok_or("activate needs window")?)?;
            windows::activate(&w)?;
            Ok(serde_json::to_value(&w).unwrap())
        }
        "state" => {
            let w = windows::resolve(step.get("window").ok_or("state needs window")?)?;
            windows::ensure_visible(&w)?;
            let want_shot = step.get("screenshot").and_then(Value::as_bool).unwrap_or(true);
            let want_text = step.get("include_text").and_then(Value::as_bool).unwrap_or(true);
            let max_w = step.get("max_width").and_then(Value::as_i64).unwrap_or(1280) as i32;
            let max_nodes = step
                .get("max_nodes")
                .and_then(Value::as_i64)
                .unwrap_or(300)
                .clamp(20, 800) as usize;
            let max_depth = step
                .get("max_depth")
                .and_then(Value::as_i64)
                .unwrap_or(14)
                .clamp(2, 60) as usize;
            let max_chars = step
                .get("max_chars")
                .and_then(Value::as_i64)
                .unwrap_or(14000)
                .clamp(0, 200000) as usize;

            let full = step.get("full").and_then(Value::as_bool).unwrap_or(false);
            let defaults = step.get("defaults").and_then(Value::as_bool).unwrap_or(true);
            let keep = step.get("keep").cloned().unwrap_or(Value::Null);
            // Browser chrome the caller almost never needs: one key, one set.
            let browser_default = defaults && !full && is_browser(&w);
            let mut skip_items: Vec<Value> = match step.get("skip") {
                Some(Value::String(s)) => s
                    .split([',', ';'])
                    .map(|p| p.trim())
                    .filter(|p| !p.is_empty())
                    .map(|p| json!(p))
                    .collect(),
                Some(Value::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            if browser_default {
                skip_items.push(json!("bookmarks"));
                skip_items.push(json!("extensions"));
            }
            let skip = Value::Array(skip_items);
            let filter = uia::Filter::parse(&keep, &skip, full);

            let mut out = json!({
                "window": w,
                "coordinate_space": "window-relative, origin at window top-left"
            });
            if want_text {
                let opts = uia::Options { max_nodes, max_depth, max_chars };
                match uia::snapshot(w.id as isize, w.x, w.y, &opts, filter) {
                    Ok(s) => {
                        out["snapshot"] = json!(s.generation);
                        let hidden: serde_json::Map<String, Value> =
                            s.hidden.iter().map(|(k, n)| (k.clone(), json!(n))).collect();
                        out["accessibility"] = json!({
                            "tree": s.tree,
                            "focused_element": s.focused,
                            "elements": s.count,
                            "truncated": s.truncated,
                            "chars": s.chars,
                            "filter": {
                                "keep": keep,
                                "skip": skip,
                                "full": full,
                                "browser_defaults": browser_default,
                                "hidden": Value::Object(hidden),
                                "keep_matches": s.keep_hits,
                                "merged_duplicates": s.merged,
                                "char_capped": s.char_truncated
                            }
                        });
                        if let Some(hint) =
                            filter_hint(&s.hidden, s.merged, s.char_truncated, s.keep_hits, !filter_empty(&keep))
                        {
                            out["accessibility"]["hint"] = json!(hint);
                        }
                    }
                    Err(e) => {
                        out["accessibility"] = Value::Null;
                        out["accessibility_error"] = json!(e);
                    }
                }
            } else {
                // Old indexes must not survive a screenshot-only observation.
                uia::invalidate();
                out["accessibility"] = Value::Null;
            }
            if !want_shot {
                return Ok(out);
            }
            if images.len() >= MAX_IMAGES || *image_bytes >= MAX_IMAGE_BYTES {
                out["screenshot"] = Value::Null;
                out["note"] = json!("image budget exhausted");
                return Ok(out);
            }
            let shot = capture::capture(w.id, w.x, w.y, w.width, w.height, Some(max_w))?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&shot.png);
            *image_bytes += b64.len();
            images.push(b64);
            out["screenshot"] = json!({"width": shot.width, "height": shot.height, "mimeType": "image/png"});
            Ok(out)
        }
        "click" => {
            let w = windows::resolve(step.get("window").ok_or("click needs window")?)?;
            guard(&w)?;
            let x = step.get("x").and_then(Value::as_i64).map(|v| v as i32);
            let y = step.get("y").and_then(Value::as_i64).map(|v| v as i32);
            let button = step.get("button").and_then(Value::as_str).unwrap_or("left");
            let count = step.get("count").and_then(Value::as_i64).unwrap_or(1) as i32;
            let index = step.get("element_index").and_then(Value::as_i64);
            let by_name = step.get("name").and_then(Value::as_str);
            let given = usize::from(index.is_some()) + usize::from(by_name.is_some())
                + usize::from(x.is_some() || y.is_some());
            if given != 1 {
                return Err("click takes exactly one of element_index, name, or x/y".into());
            }
            if let Some(nm) = by_name {
                let mut hits = uia::find_in_window(w.id as isize, nm, 8)?;
                if hits.is_empty() {
                    return Err(format!("no visible element named '{nm}' in this window"));
                }
                let exact: Vec<usize> = hits
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| h.name.to_lowercase() == nm.to_lowercase())
                    .map(|(i, _)| i)
                    .collect();
                let pick = if exact.len() == 1 {
                    exact[0]
                } else if hits.len() == 1 {
                    0
                } else {
                    let list = hits
                        .iter()
                        .take(6)
                        .map(|h| format!("{} \"{}\" at ({},{})", h.role, h.name, h.x, h.y))
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(format!(
                        "'{nm}' is ambiguous ({} matches): {list} — use element_index from state",
                        hits.len()
                    ));
                };
                let h = hits.remove(pick);
                let mut r = input::click(&w, Some(h.x - w.x), Some(h.y - w.y), button, count)?;
                r["element"] = json!({"name": h.name, "role": h.role});
                return Ok(r);
            }
            if let Some(i) = index {
                let snapshot = step.get("snapshot").and_then(Value::as_u64);
                // node_center returns screen coordinates; click() re-adds the
                // window origin, so convert back to window-relative here.
                let (cx, cy, role, name) = uia::node_center(w.id as isize, snapshot, i as usize)?;
                let mut r = input::click(&w, Some(cx - w.x), Some(cy - w.y), button, count)?;
                r["element"] = json!({"index": i, "role": role, "name": name});
                Ok(r)
            } else if x.is_some() || y.is_some() {
                // Report what the pixel actually lands on: an off-by-20 click
                // on a 40x40 icon otherwise looks like a success.
                let hit = match (x, y) {
                    (Some(xx), Some(yy)) => uia::hit_test(w.x + xx, w.y + yy),
                    _ => None,
                };
                let mut r = input::click(&w, x, y, button, count)?;
                if let Some((role, name, hx, hy, hw, hh)) = hit {
                    r["hit"] = json!({
                        "role": role,
                        "name": name,
                        "rect": [hx - w.x, hy - w.y, hw, hh]
                    });
                }
                Ok(r)
            } else {
                Err("click needs element_index or x/y".into())
            }
        }
        "set_value" => {
            let w = windows::resolve(step.get("window").ok_or("set_value needs window")?)?;
            guard(&w)?;
            let i = step
                .get("element_index")
                .and_then(Value::as_i64)
                .ok_or("set_value needs element_index")?;
            let value = step.get("value").and_then(Value::as_str).ok_or("set_value needs value")?;
            let snapshot = step.get("snapshot").and_then(Value::as_u64);
            let mut r = uia::set_value(w.id as isize, snapshot, i as usize, value)?;
            r["index"] = json!(i);
            r["value"] = json!(value);
            Ok(r)
        }
        "type" => {
            let w = windows::resolve(step.get("window").ok_or("type needs window")?)?;
            guard(&w)?;
            let text = step.get("text").and_then(Value::as_str).ok_or("type needs text")?;
            input::type_text(&w, text)
        }
        "press" => {
            let w = windows::resolve(step.get("window").ok_or("press needs window")?)?;
            guard(&w)?;
            let key = step.get("key").and_then(Value::as_str).ok_or("press needs key")?;
            guard_key(key)?;
            input::press(&w, key)
        }
        "scroll" => {
            let w = windows::resolve(step.get("window").ok_or("scroll needs window")?)?;
            guard(&w)?;
            let x = step.get("x").and_then(Value::as_i64).unwrap_or((w.width / 2) as i64) as i32;
            let y = step.get("y").and_then(Value::as_i64).unwrap_or((w.height / 2) as i64) as i32;
            let amount = step.get("scroll_y").and_then(Value::as_i64).unwrap_or(3) as i32;
            input::scroll(&w, x, y, amount)
        }
        "drag" => {
            let w = windows::resolve(step.get("window").ok_or("drag needs window")?)?;
            guard(&w)?;
            let fx = step.get("from_x").and_then(Value::as_i64).ok_or("drag needs from_x")? as i32;
            let fy = step.get("from_y").and_then(Value::as_i64).ok_or("drag needs from_y")? as i32;
            let tx = step.get("to_x").and_then(Value::as_i64).ok_or("drag needs to_x")? as i32;
            let ty = step.get("to_y").and_then(Value::as_i64).ok_or("drag needs to_y")? as i32;
            input::drag(&w, (fx, fy), (tx, ty))
        }
        "wait" => {
            let ms = step.get("ms").and_then(Value::as_i64).unwrap_or(500).clamp(0, 30_000);
            let mut left = ms;
            while left > 0 {
                if overlay::is_cancelled() {
                    break;
                }
                let chunk = left.min(100);
                std::thread::sleep(std::time::Duration::from_millis(chunk as u64));
                left -= chunk;
            }
            Ok(json!({"waited_ms": ms - left, "cancelled": overlay::is_cancelled()}))
        }
        "clipboard" => {
            match step.get("text").and_then(Value::as_str) {
                Some(text) => {
                    clipboard::write(text)?;
                    Ok(json!({"op": "clipboard", "written": text.chars().count()}))
                }
                None => {
                    let text = clipboard::read()?;
                    let total = text.chars().count();
                    let shown: String = text.chars().take(4000).collect();
                    Ok(json!({
                        "op": "clipboard",
                        "text": shown,
                        "length": total,
                        "truncated": total > 4000,
                        "empty": total == 0
                    }))
                }
            }
        }
        "tray" => {
            let name = step.get("name").and_then(Value::as_str).unwrap_or("");
            tray_action(name)
        }
        "close" => {
            let w = windows::resolve(step.get("window").ok_or("close needs window")?)?;
            windows::close(&w)?;
            Ok(json!({"closed": w.id}))
        }
        other => Err(format!("unknown op: {other}")),
    }
}
