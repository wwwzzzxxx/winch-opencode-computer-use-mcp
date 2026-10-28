//! UI Automation client: element tree with indexes + element-index actions.
//!
//! Hand-written COM vtable calls. The `windows` crate cannot link under this
//! rustup GNU toolchain (raw-dylib needs a working dlltool), so the few slots
//! we need are called through the raw vtable.
//!
//! Slot numbers come from `windows` 0.62's generated `*_Vtbl` structs, where
//! field 0 is the embedded IUnknown (`base__`, 3 pointers), so
//!     mem_slot = field_index + 2.
//! Verified against IUIAutomation_Vtbl / IUIAutomationElement_Vtbl /
//! IUIAutomationTreeWalker_Vtbl / IUIAutomationValuePattern_Vtbl.

use std::cell::RefCell;
use std::ffi::c_void;

use serde_json::{json, Value};
use std::ptr::null_mut;
use std::sync::OnceLock;

type Hr = i32;
type Hwnd = *mut c_void;
type Bstr = *mut u16;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

const CLSID_CUIAUTOMATION: Guid = Guid {
    d1: 0xff48_dba4,
    d2: 0x60ef,
    d3: 0x4201,
    d4: [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e],
};
const IID_IUIAUTOMATION: Guid = Guid {
    d1: 0x30cb_e57d,
    d2: 0xd9d0,
    d3: 0x452a,
    d4: [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee],
};
const IID_VALUE_PATTERN: Guid = Guid {
    d1: 0xa94c_d8b1,
    d2: 0x0844,
    d3: 0x4cd6,
    d4: [0x9d, 0x2d, 0x64, 0x05, 0x37, 0xab, 0x39, 0xe9],
};

const UIA_VALUEPATTERN_ID: i32 = 10002;
const CLSCTX_INPROC_SERVER: u32 = 1;
const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

type CoInitializeExFn = unsafe extern "system" fn(*mut c_void, u32) -> Hr;
type CoCreateInstanceFn =
    unsafe extern "system" fn(*const Guid, *mut c_void, u32, *const Guid, *mut *mut c_void) -> Hr;
type SysAllocStringFn = unsafe extern "system" fn(*const u16) -> Bstr;
type SysFreeStringFn = unsafe extern "system" fn(Bstr);

struct Ole {
    co_init: CoInitializeExFn,
    co_create: CoCreateInstanceFn,
    sys_alloc: SysAllocStringFn,
    sys_free: SysFreeStringFn,
}

unsafe fn proc(module: *mut c_void, name: &[u8]) -> *const c_void {
    GetProcAddress(module, name.as_ptr())
}

fn load_ole() -> Result<Ole, String> {
    unsafe {
        let mut wide: Vec<u16> = "ole32.dll".encode_utf16().collect();
        wide.push(0);
        let ole32 = LoadLibraryW(wide.as_ptr());
        if ole32.is_null() {
            return Err("cannot load ole32.dll".into());
        }
        let mut wide: Vec<u16> = "oleaut32.dll".encode_utf16().collect();
        wide.push(0);
        let oleaut = LoadLibraryW(wide.as_ptr());
        if oleaut.is_null() {
            return Err("cannot load oleaut32.dll".into());
        }
        let co_init = proc(ole32, b"CoInitializeEx\0");
        let co_create = proc(ole32, b"CoCreateInstance\0");
        let sys_alloc = proc(oleaut, b"SysAllocString\0");
        let sys_free = proc(oleaut, b"SysFreeString\0");
        if co_init.is_null() || co_create.is_null() || sys_alloc.is_null() || sys_free.is_null() {
            return Err("ole32/oleaut32 missing CoInitializeEx/CoCreateInstance/SysAllocString".into());
        }
        Ok(Ole {
            co_init: std::mem::transmute::<*const c_void, CoInitializeExFn>(co_init),
            co_create: std::mem::transmute::<*const c_void, CoCreateInstanceFn>(co_create),
            sys_alloc: std::mem::transmute::<*const c_void, SysAllocStringFn>(sys_alloc),
            sys_free: std::mem::transmute::<*const c_void, SysFreeStringFn>(sys_free),
        })
    }
}

fn ole() -> Result<&'static Ole, String> {
    static OLE: OnceLock<Result<Ole, String>> = OnceLock::new();
    match OLE.get_or_init(load_ole) {
        Ok(o) => Ok(o),
        Err(e) => Err(e.clone()),
    }
}

// --- vtable plumbing -------------------------------------------------------

unsafe fn slot<T: Copy>(obj: *mut c_void, index: usize) -> T {
    let vtbl = *(obj as *mut *mut usize);
    let p = *vtbl.add(index);
    std::mem::transmute_copy::<usize, T>(&p)
}

unsafe fn release(obj: *mut c_void) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = slot(obj, 2);
        f(obj);
    }
}

unsafe fn bstr_slot(el: *mut c_void, index: usize) -> String {
    let mut b: Bstr = null_mut();
    let f: unsafe extern "system" fn(*mut c_void, *mut Bstr) -> Hr = slot(el, index);
    if f(el, &mut b) < 0 || b.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *b.add(len) != 0 {
        len += 1;
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(b, len));
    if let Ok(o) = ole() {
        (o.sys_free)(b);
    }
    s
}

unsafe fn get_name(el: *mut c_void) -> String {
    bstr_slot(el, 23) // CurrentName
}

/// Tooltip / `aria-describedby` text. Icon-only web buttons (a copy, reveal or
/// overflow icon) expose no Name at all; their HelpText is the only label.
unsafe fn get_help_text(el: *mut c_void) -> String {
    bstr_slot(el, 31) // CurrentHelpText
}

unsafe fn get_i32(el: *mut c_void, index: usize) -> Option<i32> {
    let mut v: i32 = 0;
    let f: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hr = slot(el, index);
    if f(el, &mut v) < 0 {
        None
    } else {
        Some(v)
    }
}

unsafe fn get_rect(el: *mut c_void) -> Option<Rect> {
    let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
    let f: unsafe extern "system" fn(*mut c_void, *mut Rect) -> Hr = slot(el, 43); // CurrentBoundingRectangle
    if f(el, &mut r) < 0 {
        None
    } else {
        Some(r)
    }
}

// --- tree model ------------------------------------------------------------

#[derive(Clone)]
struct CachedNode {
    el: *mut c_void,
    role: &'static str,
    name: String,
    /// window-relative, for the printed tree
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// screen coordinates, for clicking
    sx: i32,
    sy: i32,
}

/// One observation belongs to one window: taking a state on another window
/// must not invalidate this one (that used to happen and caused bogus
/// `stale element_index` errors).
struct Entry {
    generation: u64,
    hwnd: isize,
    nodes: Vec<CachedNode>,
}

struct Cache {
    next_gen: u64,
    entries: Vec<Entry>,
}

impl Default for Cache {
    fn default() -> Self {
        Cache { next_gen: 0, entries: Vec::new() }
    }
}

impl Cache {
    fn clear(&mut self) {
        for e in self.entries.drain(..) {
            for n in e.nodes {
                unsafe { release(n.el) };
            }
        }
    }

    fn drop_window(&mut self, hwnd: isize) {
        if let Some(i) = self.entries.iter().position(|e| e.hwnd == hwnd) {
            for n in self.entries.remove(i).nodes {
                unsafe { release(n.el) };
            }
        }
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        self.clear();
    }
}

thread_local! {
    static UIA: RefCell<*mut c_void> = const { RefCell::new(null_mut()) };
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

fn uia_instance() -> Result<*mut c_void, String> {
    UIA.with(|cell| {
        let existing = *cell.borrow();
        if !existing.is_null() {
            return Ok(existing);
        }
        unsafe {
            let o = ole()?;
            let hr = (o.co_init)(null_mut(), 0); // COINIT_MULTITHREADED
            if hr < 0 && hr != RPC_E_CHANGED_MODE {
                return Err(format!("CoInitializeEx failed (0x{:08x})", hr as u32));
            }
            let mut p: *mut c_void = null_mut();
            let hr = (o.co_create)(
                &CLSID_CUIAUTOMATION,
                null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_IUIAUTOMATION,
                &mut p,
            );
            if hr < 0 || p.is_null() {
                return Err(format!("CoCreateInstance(CUIAutomation) failed (0x{:08x})", hr as u32));
            }
            *cell.borrow_mut() = p;
            Ok(p)
        }
    })
}

fn role_name(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50025 => "Custom",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        50039 => "SemanticZoom",
        50040 => "AppBar",
        _ => "Element",
    }
}

fn interactive(role: &str) -> bool {
    matches!(
        role,
        "Button"
            | "CheckBox"
            | "RadioButton"
            | "ComboBox"
            | "Edit"
            | "Hyperlink"
            | "ListItem"
            | "MenuItem"
            | "TabItem"
            | "TreeItem"
            | "DataItem"
            | "Slider"
            | "Spinner"
            | "SplitButton"
            | "Document"
    )
}

fn noisy(role: &str) -> bool {
    matches!(role, "Separator" | "Thumb" | "ScrollBar" | "TitleBar" | "StatusBar")
}

// --- set filters -----------------------------------------------------------
//
// The caller names *sets*, not atoms: one key that matches a container drops
// (or keeps) the whole subtree, so "bookmarks" costs one token instead of one
// per bookmark. Result = keep - skip.

#[derive(Clone, Copy, PartialEq)]
enum RuleKind {
    Name,
    Role,
}

#[derive(Clone)]
struct Rule {
    /// the key the caller wrote, echoed back in the summary line
    key: String,
    kind: RuleKind,
    value: String,
}

impl Rule {
    fn matches(&self, role: &str, name: &str) -> bool {
        if self.value.is_empty() {
            return false;
        }
        match self.kind {
            RuleKind::Name => name.to_lowercase().contains(&self.value),
            RuleKind::Role => role.eq_ignore_ascii_case(&self.value),
        }
    }
}

/// Built-in keys that stand for a set the caller should not have to enumerate.
fn preset(key: &str) -> Option<&'static [&'static str]> {
    const BOOKMARKS: &[&str] = &[
        "收藏夹栏",
        "书签栏",
        "bookmarks bar",
        "bookmark bar",
        "favorites bar",
        "favourites bar",
    ];
    const EXTENSIONS: &[&str] = &["扩展", "extensions", "有权访问此网站", "has access to this site"];
    match key.to_ascii_lowercase().as_str() {
        "bookmarks" | "bookmark" | "favorites" | "收藏夹" => Some(BOOKMARKS),
        "extensions" | "扩展" => Some(EXTENSIONS),
        _ => None,
    }
}

/// `keep` (default: every element) minus `skip`.
pub struct Filter {
    keep: Vec<Rule>,
    skip: Vec<Rule>,
    full: bool,
}

impl Filter {
    pub fn parse(keep: &Value, skip: &Value, full: bool) -> Filter {
        Filter { keep: rules(keep), skip: rules(skip), full }
    }

    fn keep_all(&self) -> bool {
        self.keep.is_empty()
    }

    fn keep_ok(&self, role: &str, name: &str, inherited: bool) -> bool {
        inherited || self.keep_all() || self.keep_match(role, name)
    }

    fn keep_match(&self, role: &str, name: &str) -> bool {
        !self.full && self.keep.iter().any(|r| r.matches(role, name))
    }

    fn skip_key(&self, role: &str, name: &str) -> Option<&str> {
        if self.full {
            return None;
        }
        self.skip.iter().find(|r| r.matches(role, name)).map(|r| r.key.as_str())
    }
}

fn rule_list(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => s.split([',', ';']).map(|p| p.trim().to_string()).collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .flat_map(|s| s.split([',', ';']))
            .map(|p| p.trim().to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn rules(v: &Value) -> Vec<Rule> {
    let mut out = Vec::new();
    for key in rule_list(v) {
        if key.is_empty() || key.eq_ignore_ascii_case("all") {
            continue;
        }
        if let Some(rest) = key.strip_prefix("role:").or_else(|| key.strip_prefix("Role:")) {
            for role in rest.split(',') {
                let role = role.trim();
                if !role.is_empty() {
                    out.push(Rule {
                        key: key.clone(),
                        kind: RuleKind::Role,
                        value: role.to_ascii_lowercase(),
                    });
                }
            }
            continue;
        }
        match preset(&key) {
            Some(names) => out.extend(names.iter().map(|n| Rule {
                key: key.clone(),
                kind: RuleKind::Name,
                value: n.to_lowercase(),
            })),
            None => out.push(Rule {
                key: key.clone(),
                kind: RuleKind::Name,
                value: key.to_lowercase(),
            }),
        }
    }
    out
}

fn clean(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
    }
    let trimmed = out.trim();
    if trimmed.chars().count() > max {
        let mut s: String = trimmed.chars().take(max).collect();
        s.push('…');
        s
    } else {
        trimmed.to_string()
    }
}

/// An emitted ancestor, so a child that is the same control in a different
/// role (Chromium's ListItem wrapping its Button) can be merged into it.
struct Anchor {
    name: String,
    rect: Rect,
    interactive: bool,
}

struct Build {
    nodes: Vec<CachedNode>,
    lines: Vec<String>,
    max_nodes: usize,
    max_depth: usize,
    text_budget: usize,
    truncated: bool,
    ox: i32,
    oy: i32,
    filter: Filter,
    anchors: Vec<Anchor>,
    hidden: Vec<(String, usize)>,
    merged: usize,
    keep_hits: usize,
    chars: usize,
    max_chars: usize,
    char_truncated: bool,
}

impl Build {
    fn push_line(&mut self, line: String) -> bool {
        if self.max_chars > 0 && self.chars + line.len() > self.max_chars {
            self.char_truncated = true;
            return false;
        }
        self.chars += line.len();
        self.lines.push(line);
        true
    }

    fn add_hidden(&mut self, key: &str, n: usize) {
        match self.hidden.iter_mut().find(|(k, _)| k == key) {
            Some(e) => e.1 += n,
            None => self.hidden.push((key.to_string(), n)),
        }
    }
}

fn same_rect(a: &Rect, b: &Rect) -> bool {
    (a.left - b.left).abs() <= 1
        && (a.top - b.top).abs() <= 1
        && (a.right - b.right).abs() <= 1
        && (a.bottom - b.bottom).abs() <= 1
}

/// Walks one element. Returns true when the element reference was moved into
/// the cache (caller must not release it).
unsafe fn walk(
    walker: *mut c_void,
    el: *mut c_void,
    depth: usize,
    indent: usize,
    parent_interactive: bool,
    parent_anchor: Option<usize>,
    kept_above: bool,
    b: &mut Build,
) -> bool {
    if depth > b.max_depth {
        return false;
    }
    if get_i32(el, 38).unwrap_or(0) != 0 {
        return false; // offscreen
    }
    let ctype = get_i32(el, 21).unwrap_or(0); // CurrentControlType
    let role = role_name(ctype);
    let is_interactive = interactive(role);
    let mut name = clean(&get_name(el), 80);
    if name.is_empty() && is_interactive {
        let tip = clean(&get_help_text(el), 60);
        if !tip.is_empty() {
            name = tip;
        }
    }
    let kept = kept_above || b.filter.keep_ok(role, &name, false);
    let skip_hit = b.filter.skip_key(role, &name).map(str::to_string);
    if !b.filter.full && b.filter.keep_match(role, &name) {
        b.keep_hits += 1;
    }

    // set subtraction: one matching container removes its whole subtree
    if !b.filter.full && kept {
        if let Some(key) = skip_hit {
            let mut budget = 120usize;
            let hidden = 1 + count_subtree(walker, el, depth, &mut budget);
            b.add_hidden(&key, hidden);
            // A leaf costs nothing to drop and a line about it would cost more
            // than the leaf: only summarise a real fold (a set inside it).
            if hidden < 2 {
                return false;
            }
            let line = match get_rect(el) {
                Some(r) => format!(
                    "{}{}[--] {} \"{}\" ({},{},{}x{}) hidden={} set={}",
                    "  ".repeat(indent),
                    "",
                    role,
                    name,
                    r.left - b.ox,
                    r.top - b.oy,
                    (r.right - r.left).max(0),
                    (r.bottom - r.top).max(0),
                    hidden,
                    key
                ),
                None => format!(
                    "{}{}[--] {} \"{}\" hidden={} set={}",
                    "  ".repeat(indent),
                    "",
                    role,
                    name,
                    hidden,
                    key
                ),
            };
            b.push_line(line);
            return false;
        }
    }

    let mut emit = if !kept {
        false
    } else if is_interactive {
        true
    } else if role == "Text" {
        // Text runs inside a button/list row duplicate the parent's name.
        !parent_interactive && !name.is_empty() && name.chars().count() <= 60 && b.text_budget > 0
    } else {
        !name.is_empty() && !noisy(role)
    };
    if emit && b.nodes.len() >= b.max_nodes {
        b.truncated = true;
        emit = false;
    }

    let rect = if emit || role == "Text" { get_rect(el) } else { None };
    if role == "Text" {
        // 2x2 "labels" are invisible layout artifacts.
        if let Some(r) = rect {
            if r.right - r.left < 8 || r.bottom - r.top < 8 {
                emit = false;
            }
        }
    }

    // merge the wrapper/inner duplicate (ListItem "Account" + Button "Account"
    // on the same rect): same click target, so one line is enough.
    if emit && is_interactive && !b.filter.full {
        if let (Some(pi), Some(r)) = (parent_anchor, rect) {
            let a = &b.anchors[pi];
            if a.interactive && a.name == name && same_rect(&a.rect, &r) {
                b.merged += 1;
                emit = false;
            }
        }
    }

    let mut taken = false;
    let mut child_anchor = parent_anchor;
    if emit {
        if role == "Text" {
            b.text_budget -= 1;
        }
        if let Some(r) = rect {
            let x = r.left - b.ox;
            let y = r.top - b.oy;
            let w = (r.right - r.left).max(0);
            let h = (r.bottom - r.top).max(0);
            let focus = get_i32(el, 26).unwrap_or(0) != 0; // CurrentHasKeyboardFocus
            let enabled = get_i32(el, 28).unwrap_or(1) != 0; // CurrentIsEnabled
            let password = get_i32(el, 35).unwrap_or(0) != 0; // CurrentIsPassword
            let idx = b.nodes.len();
            let mut flags = String::new();
            if focus {
                flags.push_str(" focus");
            }
            if !enabled {
                flags.push_str(" disabled");
            }
            if password {
                flags.push_str(" password");
            }
            let line = format!(
                "{}{}[{}] {} \"{}\" ({},{},{}x{}){}",
                "  ".repeat(indent),
                "",
                idx,
                role,
                name,
                x,
                y,
                w,
                h,
                flags
            );
            if b.push_line(line) {
                b.nodes.push(CachedNode { el, role, name: name.clone(), x, y, w, h, sx: r.left, sy: r.top });
                b.anchors.push(Anchor { name, rect: r, interactive: is_interactive });
                child_anchor = Some(b.anchors.len() - 1);
                taken = true;
            } else {
                b.truncated = true;
            }
        }
    }

    let child_indent = indent + usize::from(taken);
    let get_first: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hr = slot(walker, 4);
    let get_next: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hr = slot(walker, 6);
    let mut child: *mut c_void = null_mut();
    if get_first(walker, el, &mut child) >= 0 && !child.is_null() {
        while !child.is_null() {
            let consumed = walk(
                walker,
                child,
                depth + 1,
                child_indent,
                taken && is_interactive,
                child_anchor,
                kept,
                b,
            );
            let mut next: *mut c_void = null_mut();
            get_next(walker, child, &mut next);
            if !consumed {
                release(child);
            }
            child = next;
        }
    }
    if taken {
        b.anchors.pop();
    }
    taken
}

/// Counts the nodes a pruned subtree would have emitted (bounded, no rect
/// reads) so the summary line can say how much the set saved.
unsafe fn count_subtree(
    walker: *mut c_void,
    el: *mut c_void,
    depth: usize,
    budget: &mut usize,
) -> usize {
    if *budget == 0 || depth > 24 {
        return 0;
    }
    let mut n = 0;
    let get_first: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hr = slot(walker, 4);
    let get_next: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hr = slot(walker, 6);
    let mut child: *mut c_void = null_mut();
    if get_first(walker, el, &mut child) >= 0 && !child.is_null() {
        while !child.is_null() {
            if *budget == 0 {
                release(child);
                break;
            }
            *budget -= 1;
            if get_i32(child, 38).unwrap_or(0) == 0 {
                let role = role_name(get_i32(child, 21).unwrap_or(0));
                let name = clean(&get_name(child), 80);
                if interactive(role) || (!name.is_empty() && !noisy(role)) {
                    n += 1;
                }
                n += count_subtree(walker, child, depth + 1, budget);
            }
            let mut next: *mut c_void = null_mut();
            get_next(walker, child, &mut next);
            release(child);
            child = next;
        }
    }
    n
}

pub struct Snapshot {
    pub generation: u64,
    pub tree: String,
    pub focused: String,
    pub count: usize,
    pub truncated: bool,
    /// key -> number of hidden elements
    pub hidden: Vec<(String, usize)>,
    pub merged: usize,
    pub keep_hits: usize,
    pub chars: usize,
    pub char_truncated: bool,
}

pub struct Options {
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_chars: usize,
}

pub fn snapshot(
    hwnd: isize,
    ox: i32,
    oy: i32,
    opts: &Options,
    filter: Filter,
) -> Result<Snapshot, String> {
    let uia = uia_instance()?;
    CACHE.with(|c| c.borrow_mut().drop_window(hwnd));
    let generation = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.next_gen += 1;
        c.next_gen
    });

    unsafe {
        let mut root: *mut c_void = null_mut();
        let from_handle: unsafe extern "system" fn(*mut c_void, Hwnd, *mut *mut c_void) -> Hr =
            slot(uia, 6); // ElementFromHandle
        let hr = from_handle(uia, hwnd as Hwnd, &mut root);
        if hr < 0 || root.is_null() {
            return Err(format!("UIA ElementFromHandle failed (0x{:08x})", hr as u32));
        }

        let mut walker: *mut c_void = null_mut();
        let get_walker: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hr =
            slot(uia, 14); // ControlViewWalker
        let whisper = get_walker(uia, &mut walker);
        if whisper < 0 || walker.is_null() {
            release(root);
            return Err(format!("UIA ControlViewWalker failed (0x{:08x})", whisper as u32));
        }

        let mut b = Build {
            nodes: Vec::new(),
            lines: Vec::new(),
            max_nodes: if opts.max_nodes >= 10 { opts.max_nodes } else { 300 },
            max_depth: if opts.max_depth >= 2 { opts.max_depth } else { 14 },
            text_budget: 40,
            truncated: false,
            ox,
            oy,
            filter,
            anchors: Vec::new(),
            hidden: Vec::new(),
            merged: 0,
            keep_hits: 0,
            chars: 0,
            max_chars: opts.max_chars,
            char_truncated: false,
        };
        let consumed = walk(walker, root, 0, 0, false, None, false, &mut b);
        if !consumed {
            release(root);
        }
        release(walker);

        let focused = focused_line(uia, hwnd, ox, oy);
        let count = b.nodes.len();
        let tree = b.lines.join("\n");
        let truncated = b.truncated || b.char_truncated;
        CACHE.with(|c| c.borrow_mut().entries.push(Entry { generation, hwnd, nodes: b.nodes }));
        Ok(Snapshot {
            generation,
            tree,
            focused,
            count,
            truncated,
            hidden: b.hidden,
            merged: b.merged,
            keep_hits: b.keep_hits,
            chars: b.chars,
            char_truncated: b.char_truncated,
        })
    }
}

unsafe fn focused_line(uia: *mut c_void, hwnd: isize, ox: i32, oy: i32) -> String {
    let mut el: *mut c_void = null_mut();
    let get_focused: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hr = slot(uia, 8);
    if get_focused(uia, &mut el) < 0 || el.is_null() {
        return String::new();
    }
    let role = role_name(get_i32(el, 21).unwrap_or(0));
    let name = clean(&get_name(el), 80);
    let mut native: usize = 0;
    let get_native: unsafe extern "system" fn(*mut c_void, *mut usize) -> Hr = slot(el, 36);
    let _ = get_native(el, &mut native);
    let rect = get_rect(el);
    release(el);
    let suffix = if native != 0 && native != hwnd as usize { " (focus is in another window)" } else { "" };
    match rect {
        Some(r) => format!(
            "{} \"{}\" ({},{},{}x{}){}",
            role,
            name,
            r.left - ox,
            r.top - oy,
            (r.right - r.left).max(0),
            (r.bottom - r.top).max(0),
            suffix
        ),
        None => format!("{} \"{}\"{}", role, name, suffix),
    }
}

fn cached_node(hwnd: isize, generation: Option<u64>, index: usize) -> Result<CachedNode, String> {
    CACHE.with(|c| {
        let c = c.borrow();
        let entry = match generation {
            Some(gen) => match c.entries.iter().find(|e| e.generation == gen) {
                Some(e) if e.hwnd == hwnd => e,
                Some(e) => {
                    return Err(format!(
                        "snapshot {gen} was taken for window {} — take a state for this window instead",
                        e.hwnd
                    ))
                }
                None => {
                    return Err(format!(
                        "stale element_index: snapshot {gen} is no longer current — call state again"
                    ))
                }
            },
            None => c
                .entries
                .iter()
                .find(|e| e.hwnd == hwnd)
                .ok_or_else(|| "no accessibility snapshot for this window yet; call state first".to_string())?,
        };
        entry.nodes.get(index).cloned().ok_or_else(|| {
            format!(
                "element_index {index} out of range (snapshot has {} elements)",
                entry.nodes.len()
            )
        })
    })
}

/// Centre of an indexed element, in window-relative coordinates.
/// Centre of an indexed element in **screen** coordinates.
///
/// The rect is re-read from the live element first: batch mode reuses indexes
/// across several actions, and the layout may have moved since the snapshot.
/// Only when the element no longer reports a rect do we fall back to the one
/// captured with the snapshot.
pub fn node_center(
    hwnd: isize,
    generation: Option<u64>,
    index: usize,
) -> Result<(i32, i32, String, String), String> {
    let n = cached_node(hwnd, generation, index)?;
    unsafe {
        if let Some(r) = get_rect(n.el) {
            let w = (r.right - r.left).max(0);
            let h = (r.bottom - r.top).max(0);
            if w > 0 && h > 0 {
                return Ok((r.left + w / 2, r.top + h / 2, n.role.to_string(), n.name.clone()));
            }
        }
    }
    Ok((n.sx + n.w / 2, n.sy + n.h / 2, n.role.to_string(), n.name.clone()))
}

/// Replace the value of an indexed editable element through ValuePattern.
pub fn set_value(
    hwnd: isize,
    generation: Option<u64>,
    index: usize,
    value: &str,
) -> Result<Value, String> {
    let n = cached_node(hwnd, generation, index)?;
    let o = ole()?;
    unsafe {
        let mut unknown: *mut c_void = null_mut();
        let get_pattern: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> Hr =
            slot(n.el, 16); // GetCurrentPattern
        let hr = get_pattern(n.el, UIA_VALUEPATTERN_ID, &mut unknown);
        if hr < 0 || unknown.is_null() {
            return Err(format!(
                "element {index} ({} \"{}\") has no ValuePattern; click it and use type instead",
                n.role, n.name
            ));
        }
        let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hr = slot(unknown, 0);
        let mut pattern: *mut c_void = null_mut();
        let hr = qi(unknown, &IID_VALUE_PATTERN, &mut pattern);
        release(unknown);
        if hr < 0 || pattern.is_null() {
            return Err(format!("ValuePattern QueryInterface failed (0x{:08x})", hr as u32));
        }

        let mut wide: Vec<u16> = value.encode_utf16().collect();
        wide.push(0);
        let b = (o.sys_alloc)(wide.as_ptr());
        if b.is_null() && !value.is_empty() {
            release(pattern);
            return Err("SysAllocString failed".into());
        }
        let set: unsafe extern "system" fn(*mut c_void, Bstr) -> Hr = slot(pattern, 3); // SetValue
        let hr = set(pattern, b);
        if !b.is_null() {
            (o.sys_free)(b);
        }
        if hr < 0 {
            release(pattern);
            return Err(format!("ValuePattern.SetValue failed (0x{:08x})", hr as u32));
        }
        // Providers update asynchronously (Electron/React settles a frame later),
        // so poll briefly before calling it a silent no-op.
        let mut readback = current_value(pattern);
        let mut verified = readback
            .as_deref()
            .map(|v| v.trim_end() == value.trim_end() || (value.is_empty() && v.trim_end() == n.name.trim_end()))
            .unwrap_or(false);
        let mut tries = 0;
        while !verified && tries < 5 {
            std::thread::sleep(std::time::Duration::from_millis(80));
            readback = current_value(pattern);
            verified = readback
                .as_deref()
                .map(|v| v.trim_end() == value.trim_end() || (value.is_empty() && v.trim_end() == n.name.trim_end()))
                .unwrap_or(false);
            tries += 1;
        }
        release(pattern);
        // Silent no-op: contenteditable controls often accept SetValue and drop it.
        Ok(json!({
            "element": format!("{} \"{}\"", n.role, n.name),
            "verified": verified,
            "current_value": readback,
            "hint": if verified { Value::Null } else { json!("ValuePattern did not take the text; click the control and use type instead") }
        }))
    }
}

unsafe fn current_value(pattern: *mut c_void) -> Option<String> {
    let o = ole().ok()?;
    let mut b: Bstr = null_mut();
    let get: unsafe extern "system" fn(*mut c_void, *mut Bstr) -> Hr = slot(pattern, 4); // CurrentValue
    if get(pattern, &mut b) < 0 || b.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *b.add(len) != 0 {
        len += 1;
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(b, len));
    (o.sys_free)(b);
    Some(s)
}

pub struct Hit {
    pub x: i32,
    pub y: i32,
    pub name: String,
    pub role: String,
}

/// Every visible descendant of `hwnd` whose name contains `needle`
/// (case-insensitive), as screen-coordinate centres.
pub fn find_in_window(hwnd: isize, needle: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let uia = uia_instance()?;
    let needle = needle.to_lowercase();
    unsafe {
        let mut root: *mut c_void = null_mut();
        let from_handle: unsafe extern "system" fn(*mut c_void, Hwnd, *mut *mut c_void) -> Hr =
            slot(uia, 6);
        let hr = from_handle(uia, hwnd as Hwnd, &mut root);
        if hr < 0 || root.is_null() {
            return Err(format!("UIA ElementFromHandle failed (0x{:08x})", hr as u32));
        }

        let mut cond: *mut c_void = null_mut();
        let create_true: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hr =
            slot(uia, 21); // CreateTrueCondition
        if create_true(uia, &mut cond) < 0 || cond.is_null() {
            release(root);
            return Err("UIA CreateTrueCondition failed".into());
        }

        let mut arr: *mut c_void = null_mut();
        let find_all: unsafe extern "system" fn(
            *mut c_void,
            i32,
            *mut c_void,
            *mut *mut c_void,
        ) -> Hr = slot(root, 6); // FindAll
        let hr = find_all(root, 4, cond, &mut arr); // TreeScope_Descendants
        release(cond);
        release(root);
        if hr < 0 || arr.is_null() {
            return Err(format!("UIA FindAll failed (0x{:08x})", hr as u32));
        }

        let length: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hr = slot(arr, 3);
        let get: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> Hr = slot(arr, 4);
        let mut len = 0i32;
        length(arr, &mut len);

        let mut hits: Vec<Hit> = Vec::new();
        for i in 0..len.min(5000) {
            let mut el: *mut c_void = null_mut();
            if get(arr, i, &mut el) < 0 || el.is_null() {
                continue;
            }
            let name = clean(&get_name(el), 120);
            if name.to_lowercase().contains(&needle) && get_i32(el, 38).unwrap_or(1) == 0 {
                if let Some(r) = get_rect(el) {
                    let cx = (r.left + r.right) / 2;
                    let cy = (r.top + r.bottom) / 2;
                    if !hits.iter().any(|h| h.x == cx && h.y == cy && h.name == name) {
                        hits.push(Hit {
                            x: cx,
                            y: cy,
                            name,
                            role: role_name(get_i32(el, 21).unwrap_or(0)).to_string(),
                        });
                    }
                }
            }
            release(el);
            if hits.len() >= limit {
                break;
            }
        }
        release(arr);
        Ok(hits)
    }
}

/// Role and name of whatever sits at a **screen** point. Pixel clicks are
/// otherwise blind: `landed_on` only says which window got the click, so a
/// coordinate 20 px off a 40x40 icon reports success while hitting nothing.
pub fn hit_test(sx: i32, sy: i32) -> Option<(String, String, i32, i32, i32, i32)> {
    let uia = uia_instance().ok()?;
    unsafe {
        let mut el: *mut c_void = null_mut();
        // ElementFromPoint takes a POINT **by value**: an 8-byte struct in one
        // register, not two int arguments (declaring it as (i32, i32) corrupts
        // the stack and kills the process).
        let from_point: unsafe extern "system" fn(*mut c_void, Point, *mut *mut c_void) -> Hr =
            slot(uia, 7); // ElementFromPoint
        if from_point(uia, Point { x: sx, y: sy }, &mut el) < 0 || el.is_null() {
            return None;
        }
        let role = role_name(get_i32(el, 21).unwrap_or(0)).to_string();
        let name = clean(&get_name(el), 60);
        let r = get_rect(el).unwrap_or(Rect { left: 0, top: 0, right: 0, bottom: 0 });
        release(el);
        Some((
            role,
            name,
            r.left,
            r.top,
            (r.right - r.left).max(0),
            (r.bottom - r.top).max(0),
        ))
    }
}

/// Invalidates cached element indexes (called after every input action).
pub fn invalidate() {
    CACHE.with(|c| c.borrow_mut().clear());
}

/// True when the UIA client can be created on this thread.
pub fn available() -> bool {
    uia_instance().is_ok()
}
