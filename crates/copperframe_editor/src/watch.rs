//! Watching the project's `assets/`, `scenes/` and `scripts/` folders for
//! changes made by other programs (a paint program saving a texture,
//! Blender exporting a model, a text editor saving a Lua script, a tool
//! editing a scene).
//!
//! A background thread compares a snapshot of every file's size and
//! modification time once per [`POLL`]. A change is reported only once the
//! files have stopped changing for one interval, so a file that is still
//! being written is not imported half-finished. Polling works the same on
//! every platform and on network drives.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, SystemTime};

/// How often the folders are checked.
pub const POLL: Duration = Duration::from_millis(1000);

/// File size and modification time.
type Stamp = (u64, Option<SystemTime>);
type Snapshot = BTreeMap<PathBuf, Stamp>;

/// What changed since the last report, as paths relative to the project
/// with `/` separators.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub removed: Vec<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.removed.is_empty()
    }

    /// Every changed path.
    pub fn all(&self) -> impl Iterator<Item = &String> {
        self.added.iter().chain(&self.modified).chain(&self.removed)
    }
}

/// Takes snapshots of `folders` (relative to `root`) and turns them into
/// settled [`Changes`]. The thread in [`Watcher`] drives it; tests drive it
/// directly.
pub struct Scanner {
    root: PathBuf,
    folders: Vec<&'static str>,
    /// What the last report (or the start) saw.
    reported: Snapshot,
    /// The previous poll, to tell whether files are still changing.
    previous: Snapshot,
}

impl Scanner {
    pub fn new(root: &Path, folders: &[&'static str]) -> Self {
        let mut s = Self {
            root: root.to_path_buf(),
            folders: folders.to_vec(),
            reported: Snapshot::new(),
            previous: Snapshot::new(),
        };
        s.reported = s.snapshot();
        s.previous = s.reported.clone();
        s
    }

    fn snapshot(&self) -> Snapshot {
        let mut out = Snapshot::new();
        for folder in &self.folders {
            walk(&self.root.join(folder), &mut out);
        }
        out
    }

    /// One poll: `Some` when files changed since the last report and have
    /// been stable since the previous poll.
    pub fn poll(&mut self) -> Option<Changes> {
        let now = self.snapshot();
        let settled = now == self.previous;
        self.previous = now.clone();
        if !settled || now == self.reported {
            return None;
        }
        let rel = |p: &Path| {
            p.strip_prefix(&self.root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        };
        let mut changes = Changes::default();
        for (path, stamp) in &now {
            match self.reported.get(path) {
                None => changes.added.push(rel(path)),
                Some(old) if old != stamp => changes.modified.push(rel(path)),
                Some(_) => {}
            }
        }
        for path in self.reported.keys() {
            if !now.contains_key(path) {
                changes.removed.push(rel(path));
            }
        }
        self.reported = now;
        Some(changes)
    }

    /// Forgets changes the editor made itself (saving a scene, writing
    /// sidecars) so they are not reported back.
    pub fn accept_current(&mut self) {
        self.reported = self.snapshot();
        self.previous = self.reported.clone();
    }
}

fn walk(dir: &Path, out: &mut Snapshot) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Editor temp files and hidden files (.DS_Store, editor swap files).
        if name.starts_with('.') || name.ends_with(".tmp") || name.ends_with('~') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, out);
        } else if kind.is_file()
            && let Ok(meta) = entry.metadata()
        {
            out.insert(path, (meta.len(), meta.modified().ok()));
        }
    }
}

enum Control {
    /// The editor wrote files itself; take a fresh baseline.
    Accept,
}

/// The background watcher for one open project. Dropping it stops the
/// thread.
pub struct Watcher {
    rx: Receiver<Changes>,
    control: Sender<Control>,
}

impl Watcher {
    /// Starts watching; `wake` is called after each report so an idle
    /// event loop redraws.
    pub fn start(root: &Path, folders: &[&'static str], wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let (control, control_rx) = channel();
        let mut scanner = Scanner::new(root, folders);
        std::thread::Builder::new()
            .name("copperframe-watch".into())
            .spawn(move || {
                loop {
                    match control_rx.recv_timeout(POLL) {
                        Ok(Control::Accept) => {
                            scanner.accept_current();
                            continue;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if let Some(changes) = scanner.poll() {
                        if tx.send(changes).is_err() {
                            return;
                        }
                        wake();
                    }
                }
            })
            .map_err(|e| tracing::warn!("could not start the folder watcher: {e}"))
            .ok();
        Self { rx, control }
    }

    /// Changes reported since the last call, merged.
    pub fn take(&self) -> Option<Changes> {
        let mut merged: Option<Changes> = None;
        while let Ok(c) = self.rx.try_recv() {
            let m = merged.get_or_insert_with(Changes::default);
            m.added.extend(c.added);
            m.modified.extend(c.modified);
            m.removed.extend(c.removed);
        }
        merged
    }

    /// The editor just wrote into the watched folders.
    pub fn accept_own_changes(&self) {
        let _ = self.control.send(Control::Accept);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_settled_changes_once() {
        let root = std::env::temp_dir().join(format!("cf_watch_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(root.join("assets/old.png"), b"1").unwrap();
        std::fs::write(root.join("assets/gone.png"), b"1").unwrap();
        let mut s = Scanner::new(&root, &["assets", "scenes"]);
        assert_eq!(s.poll(), None, "nothing changed");

        std::fs::write(root.join("assets/textures/new.png"), b"12").unwrap();
        std::fs::write(root.join("assets/old.png"), b"123").unwrap();
        std::fs::remove_file(root.join("assets/gone.png")).unwrap();
        std::fs::write(root.join("assets/.hidden"), b"x").unwrap();
        std::fs::write(root.join("assets/save.tmp"), b"x").unwrap();
        assert_eq!(s.poll(), None, "still changing on the first poll");
        let c = s.poll().expect("settled");
        assert_eq!(c.added, ["assets/textures/new.png"]);
        assert_eq!(c.modified, ["assets/old.png"]);
        assert_eq!(c.removed, ["assets/gone.png"]);
        assert_eq!(s.poll(), None, "reported once");

        // The editor's own writes are not reported back.
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(root.join("scenes/main.scene.json"), b"{}").unwrap();
        s.accept_current();
        assert_eq!(s.poll(), None);
        assert_eq!(s.poll(), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn watcher_thread_delivers_changes() {
        let root = std::env::temp_dir().join(format!("cf_watch_thread_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let (wake_tx, wake_rx) = channel();
        let w = Watcher::start(&root, &["assets"], move || {
            let _ = wake_tx.send(());
        });
        std::fs::write(root.join("assets/a.png"), b"1").unwrap();
        wake_rx
            .recv_timeout(POLL * 6)
            .expect("the watcher reports within a few polls");
        assert_eq!(w.take().unwrap().added, ["assets/a.png"]);
        assert!(w.take().is_none());
        drop(w);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
