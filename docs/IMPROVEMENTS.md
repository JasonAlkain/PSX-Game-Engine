# Improvement list

Started 2026-11-03 after all six milestones and the Copperframe rename were
merged. Each item says what is wrong today, what changes, and its state.
Keep this list current: tick items as they merge and add new findings at the
end of their section.

## Crash and bug reporting

1. **Crashes leave no trace.** A panic prints to stderr only. On Windows the
   editor is usually started from Explorer or the itch.io app, so the console
   closes with the message in it. *Change:* a panic hook in every engine
   binary writes a crash report (app, version, OS, panic message and
   location, backtrace, the last log lines) to the per-user data folder
   (`%APPDATA%\Copperframe Engine\…\crash_reports` on Windows). The newest 20
   are kept. **Done (crash reporting PR).**
2. **No way to send a report.** *Change:* the editor checks for unseen crash
   reports when it starts and after a Play session exits with an error, and
   asks before doing anything. "Email report" opens a pre-filled draft to
   `zacharyaars+copperframeengine@gmail.com` in the user's own mail program
   (a `mailto:` link); nothing is sent without the user pressing Send there.
   Help → "Report a bug…" does the same for problems that are not crashes.
   The CLI prints the report path and the address. **Done (crash reporting
   PR).**
3. **No credentials in the binary.** The repository is public and builds
   are shipped on itch.io, so SMTP passwords or API keys in the engine would
   leak. Reports therefore go through the user's mail client. A server that
   receives reports directly (for example a small HTTPS endpoint or a hosted
   crash service) is a later choice for the project owner; it needs hosting,
   a privacy note and an opt-in setting. **Open (owner decision).**
4. **Games made with the engine must not email the engine author.** Exported
   games write crash reports to the game's own data folder and never offer
   the engine address. **Done (crash reporting PR).**
5. **Logs disappear with the console.** *Change:* engine binaries also write
   their log to `logs/<app>.log` in the data folder (the previous run is kept
   as `<app>.previous.log`), so a bug report can attach it. **Done (crash
   reporting PR).**
6. **Release backtraces have no file or line.** *Change:* release builds
   keep line tables (`debug = "line-tables-only"`) and the itch.io bundle
   ships the `.pdb` files next to the executables, so crash reports from
   Windows players point at source lines. **Done (crash reporting PR).**
7. **No bug report template on GitHub.** *Change:* `.github/ISSUE_TEMPLATE`
   bug and feature forms that ask for the crash report and log.
   **Done (crash reporting PR).**

## Stability

8. **A crashed editor locks the project.** The editor lock is a file with a
   pid. After a crash it stays behind and the editor and `copperframe apply`
   refuse the project until the user deletes it or passes `--steal-lock`,
   which itch.io users cannot do. *Change:* the editor holds an OS file lock
   on the lock file while open; the OS releases it when the process dies, so
   a stale lock is detected and replaced automatically. **Done (editor
   responsiveness PR).**
9. **Startup errors are invisible on Windows.** If the GPU or project fails
   to open, the error is printed to a console that closes at once. The log
   file (item 5) now records it. A native message box is a follow-up.
   **Partly done.**

## Editor responsiveness

10. **The editor redraws continuously while idle.** `ControlFlow::Poll`
    renders every frame even when nothing changes, which keeps a CPU core
    and the GPU busy and makes laptops hot. *Change:* redraw on input, on
    egui's repaint requests, and continuously only while something moves
    (camera fly, animation preview, splash, gizmo drag); poll at 10 Hz while
    a game runs or a background job reports progress. The status bar shows
    "idle" while waiting. **Done (editor responsiveness PR).**

## Housekeeping

11. **CI keeps running superseded commits.** *Change:* cancel in-progress
    runs for the same branch and add job timeouts. **Done (housekeeping PR).**
12. **Dependencies drift silently.** *Change:* Dependabot for GitHub
    Actions and Cargo, weekly, grouped so wgpu/winit/egui move together.
    **Done (housekeeping PR).**

## Later (not started)

- Native error dialog for startup failures on Windows (needs a dialog crate
  or the Windows API behind a reviewed `unsafe` boundary).
- Load imported models on a background thread so large glTF files do not
  stall the editor for a frame.
- Server-backed crash upload (item 3), only if the owner wants it.
- Run the Windows 11 checklist in `docs/PLATFORMS.md` on real hardware.
