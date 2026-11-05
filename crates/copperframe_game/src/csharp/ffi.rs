//! The boundary with the .NET runtime: finding and starting it
//! (`hostfxr`), the managed entry points of `Copperframe.Internal.Host`
//! (`crates/copperframe_assets/csharp/Host.cs`), and the `ctx` callback
//! managed code calls. This is the only module of the engine with
//! `unsafe` code; everything it hands out is safe to use.
//!
//! Memory rules (match `Host.cs`): strings the runtime returns are
//! allocated with `NativeMemory.Alloc` and released here with its `Free`
//! after copying. Strings passed to the runtime are borrowed for the call.
//! Strings the `ctx` callback returns live in the call's [`CallState`]
//! until the next `ctx` call, and managed code copies them at once.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use netcorehost::hostfxr::{AssemblyDelegateLoader, Hostfxr};
use netcorehost::pdcstring::PdCString;

use crate::script_api::ScriptValue;

/// Version of the entry points this engine expects from `Host.cs`.
pub const ABI_VERSION: i32 = 1;
const HOST_TYPE: &str = "Copperframe.Internal.Host, CopperframeScripts";

/// A value crossing the boundary (matches `NativeValue` in `Host.cs`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NativeValue {
    tag: i32,
    len: i32,
    i: i64,
    f: f64,
    s: *const u8,
}

const TAG_NIL: i32 = 0;
const TAG_BOOL: i32 = 1;
const TAG_INT: i32 = 2;
const TAG_NUM: i32 = 3;
const TAG_STR: i32 = 4;

impl NativeValue {
    const NIL: NativeValue = NativeValue {
        tag: TAG_NIL,
        len: 0,
        i: 0,
        f: 0.0,
        s: std::ptr::null(),
    };

    /// Reads a value managed code passed; its string is borrowed for the
    /// call.
    unsafe fn read(&self) -> ScriptValue {
        match self.tag {
            TAG_BOOL => ScriptValue::Bool(self.i != 0),
            TAG_INT => ScriptValue::Int(self.i),
            TAG_NUM => ScriptValue::Num(self.f),
            TAG_STR if !self.s.is_null() && self.len >= 0 => {
                // SAFETY: managed code pins `len` bytes at `s` for the call.
                let bytes = unsafe { std::slice::from_raw_parts(self.s, self.len as usize) };
                ScriptValue::Str(String::from_utf8_lossy(bytes).into_owned())
            }
            TAG_STR => ScriptValue::Str(String::new()),
            _ => ScriptValue::Nil,
        }
    }

    /// A value to hand to managed code; strings point into `keep`.
    fn write(v: &ScriptValue, keep: &mut Vec<Vec<u8>>) -> NativeValue {
        let mut n = NativeValue::NIL;
        match v {
            ScriptValue::Nil => {}
            ScriptValue::Bool(b) => {
                n.tag = TAG_BOOL;
                n.i = i64::from(*b);
            }
            ScriptValue::Int(i) => {
                n.tag = TAG_INT;
                n.i = *i;
            }
            ScriptValue::Num(x) => {
                n.tag = TAG_NUM;
                n.f = *x;
            }
            ScriptValue::Str(s) => {
                let bytes = s.as_bytes().to_vec();
                n.tag = TAG_STR;
                n.len = bytes.len() as i32;
                // The Vec's heap buffer does not move when `keep` grows.
                n.s = bytes.as_ptr();
                keep.push(bytes);
            }
        }
        n
    }
}

/// What a script's `ctx` call runs: method name and arguments in, results
/// or an error out.
pub type Dispatch<'a> = dyn FnMut(&str, &[ScriptValue]) -> Result<Vec<ScriptValue>, String> + 'a;

/// One callback's state, passed to managed code as an opaque pointer.
struct CallState<'a, 'b> {
    dispatch: &'a mut Dispatch<'b>,
    /// Strings of the last results.
    strings: Vec<Vec<u8>>,
}

/// The `ctx` callback managed code calls (`Host.CtxCall`). Returns the
/// number of results, or -1 with the error text in `out[0]`.
extern "system" fn ctx_call(
    state: *mut c_void,
    name: *const u8,
    name_len: i32,
    args: *const NativeValue,
    nargs: i32,
    out: *mut NativeValue,
    max_out: i32,
) -> i32 {
    if state.is_null() || out.is_null() || max_out < 1 {
        return -1;
    }
    // SAFETY: `state` is the `CallState` of the callback running on this
    // thread (see `Runtime::invoke`); managed code passes it back unchanged
    // and only during that callback.
    let st = unsafe { &mut *(state as *mut CallState<'_, '_>) };
    // SAFETY: managed code passes `name_len` bytes and `nargs` values that
    // stay valid for the call, and room for `max_out` results.
    let (name, values, out) = unsafe {
        (
            std::slice::from_raw_parts(name, name_len.max(0) as usize),
            std::slice::from_raw_parts(args, nargs.max(0) as usize),
            std::slice::from_raw_parts_mut(out, max_out as usize),
        )
    };
    st.strings.clear();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = String::from_utf8_lossy(name);
        // SAFETY: see `NativeValue::read`.
        let values: Vec<ScriptValue> = values.iter().map(|v| unsafe { v.read() }).collect();
        (st.dispatch)(&name, &values)
    }));
    let result = result.unwrap_or_else(|_| Err("the engine panicked in this call".into()));
    match result {
        Ok(values) if values.len() <= out.len() => {
            for (slot, v) in out.iter_mut().zip(&values) {
                *slot = NativeValue::write(v, &mut st.strings);
            }
            values.len() as i32
        }
        Ok(values) => {
            out[0] = NativeValue::write(
                &ScriptValue::Str(format!("{} results do not fit", values.len())),
                &mut st.strings,
            );
            -1
        }
        Err(e) => {
            out[0] = NativeValue::write(&ScriptValue::Str(e), &mut st.strings);
            -1
        }
    }
}

type InitFn = extern "system" fn(*const c_void, *const u8, i32) -> i32;
type DescribeFn = extern "system" fn(*mut i32) -> *mut u8;
type FreeFn = extern "system" fn(*mut u8);
type CreateFn = extern "system" fn(i32, *const u8, i32, *mut *mut u8, *mut i32) -> i64;
type ReleaseFn = extern "system" fn(i64);
type PromptFn = extern "system" fn(i64, *mut i32) -> *mut u8;
type InvokeFn = extern "system" fn(i64, *mut InvokeArgs) -> i32;

/// A callback to run (matches `InvokeArgs` in `Host.cs`).
#[repr(C)]
pub struct InvokeArgs {
    callback: i32,
    flag: i32,
    state: *mut c_void,
    me: i64,
    other: i64,
    text: *const u8,
    text_len: i32,
    error_len: i32,
    error: *mut u8,
}

/// The loaded scripts assembly of one project.
pub struct Runtime {
    describe: DescribeFn,
    free: FreeFn,
    create: CreateFn,
    release: ReleaseFn,
    prompt: PromptFn,
    invoke: InvokeFn,
    /// Kept so the runtime context stays open.
    _loader: AssemblyDelegateLoader,
}

// SAFETY: `Runtime` is plain function pointers into managed code plus the
// loader, which is only used while loading (under the caller's lock). The
// managed host locks its shared state and gives each thread its own `Ctx`
// buffers, so the entry points may be called from any thread.
unsafe impl Send for Runtime {}
// SAFETY: as above.
unsafe impl Sync for Runtime {}

/// Size of the signal stack threads get before they run managed code.
#[cfg(unix)]
const SIGNAL_STACK: usize = 1 << 20;

/// Gives this thread a signal stack big enough for .NET's signal handler.
///
/// .NET turns a null dereference in managed code into a
/// `NullReferenceException` from its SIGSEGV handler, which runs on the
/// thread's alternate signal stack. Rust gives every thread a small one
/// (a few pages, for its stack-overflow message) and .NET uses it rather
/// than its own when one is set, so the handler overflowed it and the
/// process crashed on the first script null dereference. Called before
/// each entry into managed code; does work once per thread.
#[cfg(unix)]
fn ensure_signal_stack() {
    use std::cell::Cell;
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    if DONE.with(Cell::get) {
        return;
    }
    DONE.with(|d| d.set(true));
    // SAFETY: `sigaltstack` with a null new stack only reads the current
    // one into `old`, a valid out-pointer.
    let mut old: libc::stack_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sigaltstack(std::ptr::null(), &mut old) } != 0 {
        return;
    }
    if old.ss_flags & libc::SS_ONSTACK != 0 {
        return;
    }
    if old.ss_flags & libc::SS_DISABLE == 0 && old.ss_size >= SIGNAL_STACK {
        return;
    }
    // Leaked on purpose: the thread may receive signals until it exits, and
    // a thread that runs scripts usually lives as long as the game.
    let stack: &'static mut [u8] = Vec::leak(vec![0u8; SIGNAL_STACK]);
    let new = libc::stack_t {
        ss_sp: stack.as_mut_ptr().cast(),
        ss_flags: 0,
        ss_size: stack.len(),
    };
    // SAFETY: `new` points at `SIGNAL_STACK` bytes that are never freed, and
    // the thread is not running on its signal stack (checked above).
    unsafe {
        libc::sigaltstack(&new, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
fn ensure_signal_stack() {}

/// A callback of `Behaviour` (matches `Host.Invoke`).
#[derive(Debug, Clone, Copy)]
pub enum Callback {
    Start = 0,
    FixedUpdate = 1,
    OnTrigger = 2,
    OnCollision = 3,
    OnInteract = 4,
    OnMessage = 5,
    OnDestroy = 6,
}

impl Callback {
    /// The C# method's name, for diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::FixedUpdate => "FixedUpdate",
            Self::OnTrigger => "OnTrigger",
            Self::OnCollision => "OnCollision",
            Self::OnInteract => "OnInteract",
            Self::OnMessage => "OnMessage",
            Self::OnDestroy => "OnDestroy",
        }
    }
}

fn pd(s: impl AsRef<std::ffi::OsStr>) -> Result<PdCString, String> {
    PdCString::from_os_str(s).map_err(|e| e.to_string())
}

impl Runtime {
    /// Starts .NET (once per process; later calls join it) and loads
    /// `assembly` (next to its `.runtimeconfig.json`).
    pub fn load(assembly: &Path, project_root: &Path) -> Result<Runtime, String> {
        let (hostfxr_path, dotnet_root) = find_hostfxr().ok_or_else(|| {
            format!(
                "C# scripts need the .NET runtime {} or newer, which was not found. Install it \
                 from {} (or set DOTNET_ROOT to where it is)",
                copperframe_assets::csharp::DOTNET_VERSION,
                copperframe_assets::csharp::DOTNET_URL
            )
        })?;
        ensure_signal_stack();
        let hostfxr = Hostfxr::load_from_path(&hostfxr_path)
            .map_err(|e| format!("could not load {}: {e}", hostfxr_path.display()))?;
        let config = assembly.with_extension("runtimeconfig.json");
        let context = hostfxr
            .initialize_for_runtime_config_with_dotnet_root(pd(&config)?, pd(&dotnet_root)?)
            .map_err(|e| format!("could not start .NET for {}: {e}", config.display()))?;
        let loader = context
            .get_delegate_loader_for_assembly(pd(assembly)?)
            .map_err(|e| format!("could not load {}: {e}", assembly.display()))?;
        let ty = pd(HOST_TYPE)?;
        macro_rules! get {
            ($name:literal, $f:ty) => {{
                let f = loader
                    .get_function_with_unmanaged_callers_only::<$f>(&ty, &pd($name)?)
                    .map_err(|e| {
                        format!(
                            "{}: entry point {} not found: {e}",
                            assembly.display(),
                            $name
                        )
                    })?;
                *f
            }};
        }
        let init: InitFn = get!("Init", fn(*const c_void, *const u8, i32) -> i32);
        let rt = Runtime {
            describe: get!("Describe", fn(*mut i32) -> *mut u8),
            free: get!("Free", fn(*mut u8)),
            create: get!(
                "Create",
                fn(i32, *const u8, i32, *mut *mut u8, *mut i32) -> i64
            ),
            release: get!("Release", fn(i64)),
            prompt: get!("Prompt", fn(i64, *mut i32) -> *mut u8),
            invoke: get!("Invoke", fn(i64, *mut InvokeArgs) -> i32),
            _loader: loader,
        };
        let root = std::path::absolute(project_root).unwrap_or_else(|_| project_root.to_path_buf());
        let root = root.to_string_lossy().into_owned();
        let version = init(ctx_call as *const c_void, root.as_ptr(), root.len() as i32);
        if version != ABI_VERSION {
            return Err(format!(
                "{} was built for another engine version (entry points v{version}, this engine \
                 needs v{ABI_VERSION}); build it again",
                assembly.display()
            ));
        }
        Ok(rt)
    }

    /// Copies and frees a string the runtime allocated.
    fn take(&self, p: *mut u8, len: i32) -> Option<String> {
        if p.is_null() {
            return None;
        }
        // SAFETY: the runtime returned `len` bytes at `p`, allocated for us.
        let s = unsafe { std::slice::from_raw_parts(p, len.max(0) as usize) };
        let s = String::from_utf8_lossy(s).into_owned();
        (self.free)(p);
        Some(s)
    }

    /// The behaviours of the assembly as JSON (see `Host.Describe`).
    pub fn describe(&self) -> String {
        ensure_signal_stack();
        let mut len = 0;
        let p = (self.describe)(&mut len);
        self.take(p, len).unwrap_or_default()
    }

    /// A new instance of kind number `kind` with the scene's values.
    pub fn create(&self, kind: usize, params_json: &str) -> Result<i64, String> {
        ensure_signal_stack();
        let mut err = std::ptr::null_mut();
        let mut err_len = 0;
        let h = (self.create)(
            kind as i32,
            params_json.as_ptr(),
            params_json.len() as i32,
            &mut err,
            &mut err_len,
        );
        if h == 0 {
            Err(self
                .take(err, err_len)
                .unwrap_or_else(|| "could not create it".into()))
        } else {
            Ok(h)
        }
    }

    pub fn release(&self, handle: i64) {
        ensure_signal_stack();
        (self.release)(handle);
    }

    pub fn prompt(&self, handle: i64) -> Option<String> {
        ensure_signal_stack();
        let mut len = 0;
        let p = (self.prompt)(handle, &mut len);
        self.take(p, len)
    }

    /// Runs a callback; `ctx` calls made by it go to `dispatch`. An error is
    /// JSON `{message, file, line}` (see `Host.Invoke`).
    #[allow(clippy::too_many_arguments)]
    pub fn invoke(
        &self,
        handle: i64,
        callback: Callback,
        me: i64,
        other: i64,
        flag: bool,
        text: &str,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), String> {
        ensure_signal_stack();
        let mut state = CallState {
            dispatch,
            strings: Vec::new(),
        };
        let mut args = InvokeArgs {
            callback: callback as i32,
            flag: i32::from(flag),
            state: (&mut state as *mut CallState<'_, '_>).cast(),
            me,
            other,
            text: text.as_ptr(),
            text_len: text.len() as i32,
            error_len: 0,
            error: std::ptr::null_mut(),
        };
        let r = (self.invoke)(handle, &mut args);
        let (err, err_len) = (args.error, args.error_len);
        if r == 0 {
            Ok(())
        } else {
            Err(self
                .take(err, err_len)
                .unwrap_or_else(|| r#"{"message":"the callback failed"}"#.into()))
        }
    }
}

/// `hostfxr` of the newest installed .NET and the install's root folder.
fn find_hostfxr() -> Option<(PathBuf, PathBuf)> {
    let lib = if cfg!(windows) {
        "hostfxr.dll"
    } else if cfg!(target_os = "macos") {
        "libhostfxr.dylib"
    } else {
        "libhostfxr.so"
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(r) = std::env::var_os("DOTNET_ROOT") {
        roots.push(r.into());
    }
    // The folder of `dotnet` on PATH (often a link into the install).
    let exe = if cfg!(windows) {
        "dotnet.exe"
    } else {
        "dotnet"
    };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(exe);
            if candidate.is_file()
                && let Ok(real) = std::fs::canonicalize(&candidate)
                && let Some(parent) = real.parent()
            {
                roots.push(parent.to_path_buf());
            }
        }
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432"] {
            if let Some(p) = std::env::var_os(var) {
                roots.push(PathBuf::from(p).join("dotnet"));
            }
        }
    } else {
        if let Ok(s) = std::fs::read_to_string("/etc/dotnet/install_location") {
            roots.push(PathBuf::from(s.trim()));
        }
        for p in [
            "/usr/local/share/dotnet",
            "/usr/share/dotnet",
            "/usr/lib/dotnet",
            "/usr/lib64/dotnet",
            "/opt/dotnet",
        ] {
            roots.push(p.into());
        }
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join(".dotnet"));
        }
    }
    for root in roots {
        let fxr = root.join("host").join("fxr");
        let Ok(entries) = std::fs::read_dir(&fxr) else {
            continue;
        };
        let newest = entries
            .flatten()
            .filter(|e| e.path().join(lib).is_file())
            .max_by_key(|e| version_key(&e.file_name().to_string_lossy()));
        if let Some(dir) = newest {
            return Some((dir.path().join(lib), root));
        }
    }
    None
}

/// `10.0.14` sorts after `9.0.3`; preview suffixes sort before the release.
fn version_key(v: &str) -> (Vec<u64>, bool) {
    let (nums, pre) = match v.split_once('-') {
        Some((n, _)) => (n, false),
        None => (v, true),
    };
    (
        nums.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
        pre,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_runtimes_sort_last() {
        let mut v = vec!["10.0.14", "9.0.3", "10.0.2", "10.0.14-rc.1"];
        v.sort_by_key(|s| version_key(s));
        assert_eq!(v, ["9.0.3", "10.0.2", "10.0.14-rc.1", "10.0.14"]);
    }

    #[test]
    fn values_cross_the_boundary_unchanged() {
        let mut keep = Vec::new();
        for v in [
            ScriptValue::Nil,
            ScriptValue::Bool(true),
            ScriptValue::Int(-7),
            ScriptValue::Num(2.5),
            ScriptValue::Str("héllo".into()),
        ] {
            let n = NativeValue::write(&v, &mut keep);
            // SAFETY: the string bytes live in `keep`.
            assert_eq!(unsafe { n.read() }, v);
        }
    }
}
