//! Crash reports, log capture and bug-report drafts.
//!
//! [`install`] sets a panic hook that writes a plain-text crash report into
//! `<data dir>/crash_reports/` and keeps the newest [`KEEP_REPORTS`]. Log
//! lines passed to [`record_log`] are kept in memory (the tail goes into
//! every report) and, once installed, appended to `<data dir>/logs/<app>.log`.
//!
//! Nothing here sends anything. Engine tools offer the user a pre-filled
//! `mailto:` draft ([`mailto_url`]) that their own mail program sends, so no
//! credentials ship in the binaries. Games made with the engine install
//! without an address and only keep local reports.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where engine tools (editor, CLI, sandbox) send bug and crash reports.
pub const REPORT_EMAIL: &str = "zacharyaars+copperframeengine@gmail.com";

/// Set by the editor on the game processes it starts for Play, so their
/// crash reports land where the editor looks for them.
pub const CRASH_DIR_ENV: &str = "COPPERFRAME_CRASH_DIR";

/// Crash reports kept per data folder; older ones are deleted.
pub const KEEP_REPORTS: usize = 20;

/// Log lines kept in memory for reports.
pub const LOG_TAIL_LINES: usize = 200;

/// Lines of the log tail included in an email draft.
const MAIL_LOG_LINES: usize = 25;

/// Longest `mailto:` URL handed to the mail program. Some Windows mail
/// clients drop links above roughly 2 000 characters.
pub const MAX_MAILTO_LEN: usize = 1900;

/// The log file stops growing past this size (the in-memory tail does not).
const MAX_LOG_FILE_BYTES: u64 = 16 * 1024 * 1024;

const REPORTS_DIR: &str = "crash_reports";
const LOGS_DIR: &str = "logs";
const SEEN_SUFFIX: &str = ".seen.txt";

struct Reporter {
    app: String,
    data_dir: Option<PathBuf>,
    email: Option<String>,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();
static LOG_TAIL: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
/// `None` until the first line is logged after [`install`]; tools that never
/// log (the CLI) leave no empty log files behind.
static LOG_FILE: Mutex<Option<Option<LogFile>>> = Mutex::new(None);
static CONTEXT: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

struct LogFile {
    file: fs::File,
    written: u64,
}

/// The per-user data folder of the engine tools:
/// `%APPDATA%\Copperframe Engine\copperframe-engine\data` on Windows,
/// `~/.local/share/copperframe-engine` on Linux. `None` without a home folder.
pub fn engine_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "Copperframe Engine", "copperframe-engine")
        .map(|d| d.data_dir().to_path_buf())
}

/// Installs the panic hook and opens the log file. `data_dir` holds
/// `crash_reports/` and `logs/`; `None` keeps everything in memory (the
/// panic is still printed). [`CRASH_DIR_ENV`], when set, replaces
/// `data_dir`. `email` is the address offered for reports; pass `None` for
/// games. Only the first call has an effect.
pub fn install(app: &str, data_dir: Option<PathBuf>, email: Option<&str>) {
    let data_dir = std::env::var_os(CRASH_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or(data_dir);
    let reporter = Reporter {
        app: app.to_string(),
        data_dir,
        email: email.map(str::to_string),
    };
    if REPORTER.set(reporter).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The default hook prints the message (and a backtrace if asked).
        previous(info);
        let location = info
            .location()
            .map_or_else(|| "unknown".to_string(), |l| l.to_string());
        let message = panic_message(info.payload());
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let text = crash_report_text(&message, &location, &thread, &backtrace);
        match save_crash_report(&text) {
            Some(path) => {
                eprintln!("A crash report was saved to {}", path.display());
                if let Some(email) = REPORTER.get().and_then(|r| r.email.as_deref()) {
                    eprintln!(
                        "Please email it to {email} (the editor offers to do this the next time it starts)."
                    );
                }
            }
            None => eprintln!("(no crash report saved: no writable data folder)"),
        }
    }));
}

/// The data folder given to [`install`] (or [`CRASH_DIR_ENV`]).
pub fn data_dir() -> Option<PathBuf> {
    REPORTER.get().and_then(|r| r.data_dir.clone())
}

/// The folder crash reports are written to, once [`install`] ran with one.
pub fn reports_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join(REPORTS_DIR))
}

/// The folder log files are written to, once [`install`] ran with one.
pub fn logs_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join(LOGS_DIR))
}

/// The address reports go to for this app (`None` for games).
pub fn report_email() -> Option<&'static str> {
    REPORTER.get().and_then(|r| r.email.as_deref())
}

/// Adds or replaces a `key: value` line in every report (for example the
/// graphics adapter once it is known).
pub fn set_context(key: &str, value: impl Into<String>) {
    let value = value.into();
    let mut ctx = CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    match ctx.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value,
        None => ctx.push((key.to_string(), value)),
    }
}

/// Opens `<logs>/<app>.log`, keeping the previous run as `<app>.previous.log`.
fn open_log_file() -> Option<LogFile> {
    let reporter = REPORTER.get()?;
    let dir = reporter.data_dir.as_ref()?.join(LOGS_DIR);
    fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.log", reporter.app));
    if path.exists() {
        let _ = fs::rename(&path, dir.join(format!("{}.previous.log", reporter.app)));
    }
    let file = fs::File::create(&path).ok()?;
    Some(LogFile { file, written: 0 })
}

/// Records formatted log output (one or more lines, without colour codes):
/// kept in the in-memory tail and appended to the log file if one is open.
pub fn record_log(text: &str) {
    {
        let mut tail = LOG_TAIL.lock().unwrap_or_else(|e| e.into_inner());
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if tail.len() == LOG_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line.to_string());
        }
    }
    if REPORTER.get().is_none() {
        return;
    }
    let mut guard = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(log) = guard.get_or_insert_with(open_log_file).as_mut() {
        if log.written >= MAX_LOG_FILE_BYTES {
            return;
        }
        let bytes = text.as_bytes();
        if log.file.write_all(bytes).is_ok() {
            log.written += bytes.len() as u64;
            if log.written >= MAX_LOG_FILE_BYTES {
                let _ = log.file.write_all(b"[log file size limit reached]\n");
            }
        }
    }
}

/// The last [`LOG_TAIL_LINES`] log lines, oldest first.
pub fn recent_log() -> Vec<String> {
    LOG_TAIL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(non-text panic payload)".to_string()
    }
}

/// App, version, platform and context lines shared by crash and bug reports.
pub fn system_summary() -> String {
    let app = REPORTER.get().map_or("unknown", |r| r.app.as_str());
    let mut out = String::new();
    let _ = writeln!(out, "app: {app}");
    let _ = writeln!(out, "version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        out,
        "platform: {} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        if cfg!(debug_assertions) {
            "debug build"
        } else {
            "release build"
        }
    );
    let _ = writeln!(out, "time: {}", utc_timestamp(SystemTime::now()));
    for (k, v) in CONTEXT.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let _ = writeln!(out, "{k}: {v}");
    }
    out
}

fn crash_report_text(message: &str, location: &str, thread: &str, backtrace: &str) -> String {
    let mut out = String::from("Copperframe Engine crash report\n\n");
    out.push_str(&system_summary());
    let _ = writeln!(out, "\npanic: {message}");
    let _ = writeln!(out, "at: {location}");
    let _ = writeln!(out, "thread: {thread}");
    let _ = writeln!(out, "\nbacktrace:\n{}", backtrace.trim_end());
    out.push_str("\nrecent log:\n");
    for line in recent_log() {
        let _ = writeln!(out, "{line}");
    }
    out
}

fn save_crash_report(text: &str) -> Option<PathBuf> {
    let dir = reports_dir()?;
    fs::create_dir_all(&dir).ok()?;
    let app = REPORTER.get().map_or("app", |r| r.app.as_str());
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = dir.join(format!("{app}-{secs}-{}.txt", std::process::id()));
    fs::write(&path, text).ok()?;
    prune_reports(&dir, KEEP_REPORTS);
    Some(path)
}

/// Deletes all but the newest `keep` reports in `dir`.
fn prune_reports(dir: &Path, keep: usize) {
    let mut files = report_files(dir, true);
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    for (path, _) in files.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

fn report_files(dir: &Path, include_seen: bool) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let wanted = name.ends_with(".txt") && (include_seen || !name.ends_with(SEEN_SUFFIX));
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            wanted.then(|| (e.path(), meta.modified().unwrap_or(UNIX_EPOCH)))
        })
        .collect()
}

/// A saved crash report the user has not acted on yet.
#[derive(Debug, Clone, PartialEq)]
pub struct CrashReport {
    pub path: PathBuf,
    pub text: String,
}

/// Unseen crash reports in `dir`, newest first.
pub fn pending_reports_in(dir: &Path) -> Vec<CrashReport> {
    let mut files = report_files(dir, false);
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files
        .into_iter()
        .filter_map(|(path, _)| {
            let text = fs::read_to_string(&path).ok()?;
            Some(CrashReport { path, text })
        })
        .collect()
}

/// Unseen crash reports of the installed data folder, newest first.
pub fn pending_reports() -> Vec<CrashReport> {
    reports_dir().map_or_else(Vec::new, |d| pending_reports_in(&d))
}

/// Marks a report as handled (renamed to `*.seen.txt`; it stays on disk
/// until pruned).
pub fn mark_seen(report: &CrashReport) {
    let name = report
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(stem) = name.strip_suffix(".txt") {
        let _ = fs::rename(
            &report.path,
            report.path.with_file_name(format!("{stem}{SEEN_SUFFIX}")),
        );
    }
}

/// Email draft for a crash report.
pub fn crash_mail(email: &str, report: &CrashReport) -> String {
    let first = report
        .text
        .lines()
        .find_map(|l| l.strip_prefix("panic: "))
        .unwrap_or("crash");
    let subject = format!("Copperframe crash: {}", clip(first, 80));
    let body = format!(
        "Copperframe Engine closed unexpectedly. The report is below.\n\
         What were you doing when it happened?\n\n\n\
         --- crash report ({}) ---\n{}",
        report.path.display(),
        report.text
    );
    mailto_url(email, &subject, &body, Some(&report.path))
}

/// Email draft for a bug the user describes. `include_log` adds the last log
/// lines.
pub fn bug_mail(email: &str, summary: &str, description: &str, include_log: bool) -> String {
    let summary = summary.trim();
    let subject = if summary.is_empty() {
        "Copperframe bug report".to_string()
    } else {
        format!("Copperframe bug: {}", clip(summary, 80))
    };
    mailto_url(email, &subject, &bug_body(description, include_log), None)
}

/// Body of [`bug_mail`], also shown to the user before sending.
pub fn bug_body(description: &str, include_log: bool) -> String {
    let mut body = String::new();
    let description = description.trim();
    body.push_str(if description.is_empty() {
        "(describe what happened and how to make it happen again)"
    } else {
        description
    });
    body.push_str("\n\n--- system ---\n");
    body.push_str(&system_summary());
    if include_log {
        let log = recent_log();
        body.push_str("\n--- recent log ---\n");
        for line in &log[log.len().saturating_sub(MAIL_LOG_LINES)..] {
            body.push_str(line);
            body.push('\n');
        }
    }
    body
}

fn clip(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max_chars) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Builds a `mailto:` URL no longer than [`MAX_MAILTO_LEN`]. A body that
/// does not fit is cut and ends with a note asking to attach `attach` (the
/// full report) instead.
pub fn mailto_url(email: &str, subject: &str, body: &str, attach: Option<&Path>) -> String {
    let head = format!("mailto:{}?subject={}&body=", encode(email), encode(subject));
    let full = encode(body);
    if head.len() + full.len() <= MAX_MAILTO_LEN {
        return head + &full;
    }
    let note = match attach {
        Some(p) => format!(
            "\n\n[cut to fit an email link; please attach the full report: {}]",
            p.display()
        ),
        None => "\n\n[cut to fit an email link]".to_string(),
    };
    let note = encode(&note);
    let budget = MAX_MAILTO_LEN.saturating_sub(head.len() + note.len());
    let mut kept = String::new();
    let mut used = 0;
    for c in body.chars() {
        let mut buf = [0u8; 4];
        let piece = encode(c.encode_utf8(&mut buf));
        if used + piece.len() > budget {
            break;
        }
        used += piece.len();
        kept.push_str(&piece);
    }
    head + &kept + &note
}

/// Percent-encodes everything except RFC 3986 unreserved characters, with
/// line breaks as `%0D%0A` (RFC 6068).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let normalised = s.replace("\r\n", "\n");
    for b in normalised.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b'\n' => out.push_str("%0D%0A"),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Opens a URL (`mailto:`, `https:`) or a folder with the system's default
/// handler. Nothing goes through a shell, so the URL is not reinterpreted.
pub fn open_external(target: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "windows") {
        if Path::new(target).is_dir() {
            let mut c = Command::new("explorer.exe");
            c.arg(target);
            c
        } else {
            let mut c = Command::new("rundll32.exe");
            c.arg("url.dll,FileProtocolHandler").arg(target);
            c
        }
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(target);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

/// `YYYY-MM-DD hh:mm:ss UTC`.
pub fn utc_timestamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (H. Hinnant), valid for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("copperframe-report-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn encodes_for_mailto() {
        assert_eq!(encode("a b&c=d\n"), "a%20b%26c%3Dd%0D%0A");
        assert_eq!(encode("é"), "%C3%A9");
        assert_eq!(encode("x\r\ny"), "x%0D%0Ay");
        let url = mailto_url(REPORT_EMAIL, "Hi there", "line", None);
        assert_eq!(
            url,
            "mailto:zacharyaars%2Bcopperframeengine%40gmail.com?subject=Hi%20there&body=line"
        );
    }

    #[test]
    fn long_bodies_are_cut_with_a_note() {
        let body = "é".repeat(5000);
        let path = Path::new("report.txt");
        let url = mailto_url(REPORT_EMAIL, "s", &body, Some(path));
        assert!(url.len() <= MAX_MAILTO_LEN, "{}", url.len());
        assert!(url.ends_with(&encode(&format!(
            "\n\n[cut to fit an email link; please attach the full report: {}]",
            path.display()
        ))));
        // Never splits an encoded character.
        assert!(url.contains("%C3%A9%0D%0A"));
    }

    #[test]
    fn log_tail_is_bounded() {
        for i in 0..LOG_TAIL_LINES + 10 {
            record_log(&format!("line {i}\n"));
        }
        let log = recent_log();
        // Other tests log concurrently, so only check the bound and that the
        // newest of these lines is present.
        assert_eq!(log.len(), LOG_TAIL_LINES);
        assert!(log.contains(&format!("line {}", LOG_TAIL_LINES + 9)));
    }

    #[test]
    fn pending_reports_newest_first_and_seen_skipped() {
        let dir = temp_dir("pending");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "panic: one").unwrap();
        fs::write(&b, "panic: two").unwrap();
        let old = SystemTime::now() - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(old)
            .unwrap();
        fs::write(dir.join("notes.md"), "ignored").unwrap();

        let pending = pending_reports_in(&dir);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].path, b);
        assert!(
            crash_mail(REPORT_EMAIL, &pending[0]).contains("subject=Copperframe%20crash%3A%20two")
        );

        mark_seen(&pending[0]);
        let pending = pending_reports_in(&dir);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, a);
        assert!(dir.join("b.seen.txt").is_file());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prunes_oldest_reports() {
        let dir = temp_dir("prune");
        let now = SystemTime::now();
        for i in 0..5u64 {
            let p = dir.join(format!("r{i}.txt"));
            fs::write(&p, "x").unwrap();
            fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(now - Duration::from_secs(100 - i))
                .unwrap();
        }
        prune_reports(&dir, 3);
        let mut left: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["r2.txt", "r3.txt", "r4.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn crash_report_has_the_essentials() {
        set_context("graphics", "Test Adapter (Vulkan)");
        let text = crash_report_text("boom", "src/x.rs:1:2", "main", "0: frame");
        for needle in [
            "panic: boom",
            "at: src/x.rs:1:2",
            "thread: main",
            "0: frame",
            "graphics: Test Adapter (Vulkan)",
            "version: ",
            "platform: ",
        ] {
            assert!(text.contains(needle), "missing {needle}:\n{text}");
        }
    }

    #[test]
    fn bug_body_includes_system_and_log() {
        record_log("bug-body-marker\n");
        let body = bug_body("  it broke  ", true);
        assert!(body.starts_with("it broke\n"));
        assert!(body.contains("--- system ---"));
        assert!(body.contains("bug-body-marker"));
        assert!(!bug_body("", false).contains("--- recent log ---"));
    }

    #[test]
    fn timestamps() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01 00:00:00 UTC");
        let t = UNIX_EPOCH + Duration::from_secs(1_793_664_000 + 3_723);
        assert_eq!(utc_timestamp(t), "2026-11-03 01:02:03 UTC");
    }
}
