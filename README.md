# winch

**The agent's hand on Windows.** An MCP server that drives real desktop apps through the
UI Automation tree — indexed controls, verified clicks, no vision model, no pixel guessing.

> Not affiliated with the OpenCode project. Built to work with any MCP client.

## Why another computer-use server

Most computer-use stacks hand the model a screenshot and hope it can aim. winch asks the
application what its controls actually are:

| | screenshot-based CUA | winch |
|---|---|---|
| How it aims | model reads a PNG and estimates pixels | `[57] Button "Copy" (1474,780,40x40)` → click the centre of that control |
| Cost per observation | one image per step | a text tree you can cut down by set (see *filters*) |
| Failure mode | "clicked 20 px off, nothing happened" | `hit: {role:"Button", rect:[…]}` says what the pixel landed on |
| Verification | squint at the next screenshot | `set_value` reads the value back, `clipboard` proves a copy |

Measured on a real Edge window (bookmarks bar + extensions visible): **116 nodes / 6.7k chars
unfiltered → 73 nodes / 4.1k chars** with the default browser blacklist, and **43 / 2.1k** with
`keep: ["role:Document"]`. A single process idles around 6 MB; nine parallel instances measured
~51 MB, where the Node-based equivalent we compared against took ~530 MB.

## What it does

- **Batch steps** — one MCP call runs up to 40 steps (launch → state → click → type → state),
  so an action loop is not 40 round trips.
- **Indexed controls** — `state` returns the accessibility tree; `click {element_index}` resolves
  to the centre of that control at click time, re-reading its live rect.
- **Window-relative coordinates** — multi-monitor and DPI handled server-side.
- **Set-based tree filtering** — `skip` / `keep` name *groups* (a toolbar, a sidebar, a role), and
  the whole subtree is folded into one line.
- **Clipboard in and out** — so "did the copy work?" is a question with an answer.
- **Screenshots** as MCP image content, `PrintWindow(PW_RENDERFULLCONTENT)` with a BitBlt fallback.
- **Takeover indicator** — a click-through glow + banner while the agent controls the desktop;
  `Esc` aborts the running batch.

Windows only (UI Automation). No Node, no PowerShell child processes, no runtime deps —
`serde`, `serde_json`, `base64`, and hand-written Win32/COM FFI.

## Build

Needs the rustup **GNU** toolchain (no MSVC SDK required):

```bash
export PATH="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained:$PATH"
export RUSTFLAGS="-C link-self-contained=yes"
cargo +stable-x86_64-pc-windows-gnu build --release
```

Or on Windows, `powershell -File build.ps1` → `target/release/winch-mcp.exe` (~0.55 MB).

## Configure

**OpenCode** (`~/.config/opencode/opencode.json`):

```json
{
  "mcp": {
    "winch": {
      "type": "local",
      "command": ["C:\\path\\to\\winch-mcp.exe"],
      "timeout": 120000
    }
  }
}
```

`timeout` matters: batches with screenshots and window waits routinely exceed the default 30 s.

**Claude Desktop / Codex CLI** — same idea, point the stdio server at the exe:

```json
{ "mcpServers": { "winch": { "command": "C:\\path\\to\\winch-mcp.exe" } } }
```

## Tools

| Tool | Purpose |
|---|---|
| `winch_run` | runs up to 40 steps in one call; returns per-step results plus screenshots |
| `winch_health` | backend availability, window count, pid, version |
| `winch_end` | end the session and remove the takeover banner (always safe, idempotent) |

### Step ops

`list_windows` · `launch` · `activate` · `show` · `state` · `click` · `set_value` · `type` ·
`press` · `scroll` · `drag` · `tray` · `clipboard` · `wait` · `close`

```json
{
  "steps": [
    { "op": "activate", "window": { "title_contains": "Notepad" } },
    { "op": "state", "window": { "title_contains": "Notepad" } },
    { "op": "click", "window": { "title_contains": "Notepad" }, "element_index": 3, "snapshot": 7 },
    { "op": "type", "window": { "title_contains": "Notepad" }, "text": "hello" },
    { "op": "clipboard", "text": "hello" },
    { "op": "state", "window": { "title_contains": "Notepad" }, "screenshot": false }
  ]
}
```

Window refs: `{id}`, `{title_contains}`, `{app}`. `click`/`scroll`/`drag` coordinates are
window-relative. Aim at the **visual centre** of a control — adjacent toolbar icons sit 15–20 px
apart and an edge coordinate lands on the neighbour.

## The accessibility tree

`state {include_text: true}` (default) returns:

```json
{
  "snapshot": 7,
  "accessibility": {
    "elements": 176,
    "chars": 9820,
    "tree": "[0] Window \"Notepad\" (0,0,1445x802)\n  [1] Pane ...\n    [133] Edit \"\" (315,687,664x26) focus",
    "focused_element": "Edit \"\" (315,687,664x26)",
    "filter": { "hidden": {}, "merged_duplicates": 10, "char_capped": false }
  }
}
```

Line format: `[index] Role "name" (x,y,WxH) flags`, coordinates window-relative. Invisible
nodes, 2×2 layout artifacts and duplicated text inside buttons are dropped. Two caps apply:
`max_nodes` (default 300) and `max_chars` (default 14000).

### Set filters (`keep` − `skip`)

Filter by **set**, not by item: a key that matches a container drops (or keeps) its whole
subtree and reports one line, so `"收藏夹栏"` costs one token instead of twelve.

```
[--] ToolBar "收藏夹栏" (8,88,2560x32) hidden=27 set=bookmarks
```

| Param | Default | Meaning |
|---|---|---|
| `skip` | `null` | blacklist: name substring (case-insensitive), `role:Role`, or preset `bookmarks` / `extensions` |
| `keep` | `"all"` | whitelist; a matched container keeps its subtree. Result = keep − skip |
| `defaults` | `true` | browsers additionally drop the bookmarks bar and extension buttons |
| `full` | `false` | disable filtering, browser defaults and duplicate merging |

- Dropping a single leaf prints nothing (a summary line would cost more than the leaf); it is
  only counted in `filter.hidden`.
- Wrapper/inner duplicates (Chromium's `ListItem "Account"` wrapping a same-rect `Button`) are
  merged into one line — see `filter.merged_duplicates`.
- Hidden controls have no `element_index`, but `click {name:"…"}` searches the live tree, so
  pruning never blocks an action. Use `{full:true}` when you need the index.

### Clipboard, unnamed icons, hit receipts

- `{op:"clipboard"}` reads the clipboard (>4000 chars is truncated, `length` reports the true
  size); `{op:"clipboard", text:"…"}` writes it. Verify a copy instead of assuming it.
- An icon-only control with no accessible name falls back to its UIA **HelpText** (the web
  tooltip), so you get `Button "Copy"` rather than `Button ""`. If both are empty, click it by
  `element_index`: the tree has its rect and often `focus`.
- Pixel clicks return `hit: {role, name, rect}` — what the coordinate actually landed on.
  `landed_on` only tells you which window received the click.

## Modes

`winch_run` takes a top-level `mode`:

| Mode | Behaviour |
|---|---|
| `verified` (default) | every input invalidates `element_index`; forces re-observation |
| `batch` | indexes stay valid for the whole call; each indexed click re-reads the element's live rect |

Indexes are scoped **per window**: a `state` of window A does not invalidate window B's indexes.
A stale index is a hard error (`stale element_index`), never a click on whatever moved there.
`quiet: true` runs steps without painting the takeover banner (tests, background probes).

## Safety

Two hard refusals, enforced in the server rather than in a prompt:

- **No input to terminal windows**: WindowsTerminal, conhost, OpenConsole, cmd, powershell, pwsh,
  wt, WSL distros, mintty, bash.
- **No Windows/Meta key chords** at all (`win+e`, `super_l`, …).

`type` also checks `GetForegroundWindow()` before injecting and refuses if the target is not in
front, so text cannot land in whatever the user is actually typing into.

## Behaviour worth knowing

- **Tray and hidden windows.** Don't `ShowWindow` a hidden Chromium window (QQ, WeChat, aTrust):
  its renderer is gone, and you get a window that paints but ignores every click. `show` goes
  through the app's own tray icon/exe, and only `launch {restart:true}` actually kills an instance.
- **Popups.** Search dropdowns and Qt tool windows are separate top-level windows; a same-process
  popup may be clicked from the window that opened it, and `landed_on` reports where it landed.
  Windows of another process are still refused.
- **`set_value`** uses UIA `ValuePattern` and polls `CurrentValue` for ~400 ms to verify the write.
  If the app accepts but drops it, you get `verified: false` instead of a silent no-op.
- **Takeover indicator** lives for the task, not the call: it stays up between calls, `Esc` aborts
  the running batch, and it auto-hides 30 s after the last control step.

## Limitations

- The UIA tree is only as good as the target's provider: Chromium/Electron, Win32 and WinUI are
  good; games, custom-drawn UIs and some Java/Qt apps expose only `[0] Window`.
- Elevated (UIPI) windows expose no subtree; UIA calls have no timeout, so a hung target can stall
  `state`.
- `type` uses Unicode `SendInput`; IME-guarded surfaces (some games, remote desktop) may ignore it.

## 中文速览

- **它是什么**：用 UI Automation 控件树驱动 Windows 应用的 MCP server，不靠截图猜坐标。
- **一次调用跑多步**：`winch_run` 里最多 40 个 step，避免每步一次往返；`element_index` 点的是
  控件包围盒中心，点击瞬间重读实时矩形。
- **省 token 的开关**：`state {skip:["收藏夹栏"]}` 剪掉整棵子树只留一行摘要；`keep` 是白名单，
  结果是 keep − skip；浏览器默认已剪掉收藏夹栏与扩展图标，`{full:true}` 可关掉一切过滤。
- **复制类任务**：`{op:"clipboard"}` 读/写剪贴板，复制后用它可以验证；页面上的图标按钮若无
  无障碍名称，用 `element_index` 点，旁边的 `Text` 往往就是要的值。
- **坑**：隐藏的 Chromium 窗口不要 `ShowWindow`（renderer 已死，画面在但点不动），走 `show`；
  终端类窗口和 Win 键组合被硬拒绝；`timeout` 建议 120000 毫秒。

## License

MIT OR Apache-2.0.
