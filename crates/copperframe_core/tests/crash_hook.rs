//! The panic hook writes a crash report with the panic, the location and the
//! log tail. The panic happens in a child copy of this test binary so the
//! hook does not affect other tests.

use std::path::PathBuf;
use std::process::Command;

use copperframe_core::report;

const CHILD_ENV: &str = "COPPERFRAME_CRASH_HOOK_CHILD";

#[test]
fn panic_writes_a_crash_report() {
    if let Some(dir) = std::env::var_os(CHILD_ENV) {
        report::install("crash_hook_test", Some(PathBuf::from(dir)), None);
        report::record_log("INFO before the crash\n");
        panic!("deliberate test panic");
    }

    let dir = std::env::temp_dir().join(format!("copperframe-crash-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "panic_writes_a_crash_report", "--nocapture"])
        .env(CHILD_ENV, &dir)
        .env_remove(report::CRASH_DIR_ENV)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the child should have panicked");

    let reports = report::pending_reports_in(&dir.join("crash_reports"));
    assert_eq!(reports.len(), 1, "{}", String::from_utf8_lossy(&out.stderr));
    let text = &reports[0].text;
    assert!(text.contains("panic: deliberate test panic"), "{text}");
    assert!(text.contains("crash_hook.rs"), "{text}");
    assert!(text.contains("INFO before the crash"), "{text}");
    assert!(text.contains("backtrace:"), "{text}");
    let log = std::fs::read_to_string(dir.join("logs/crash_hook_test.log")).unwrap();
    assert!(log.contains("INFO before the crash"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("A crash report was saved to"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}
