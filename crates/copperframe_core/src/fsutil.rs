//! File helpers shared by every project file format.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `path` with `suffix` appended to its file name (`scene.json` → `scene.json.bak`).
pub fn sibling_path(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} has no file name", path.display())))?;
    let mut name = name.to_os_string();
    name.push(suffix);
    Ok(path.with_file_name(name))
}

/// Writes `contents` atomically: a temporary file next to `path` is written
/// and flushed, the previous file (if any) is copied to `<name>.bak`, then
/// the temporary file is renamed over `path`. A crash leaves either the old
/// or the new file in place, never a partial one.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    use io::Write as _;
    let tmp = sibling_path(path, ".tmp")?;
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    if path.exists() {
        fs::copy(path, sibling_path(path, ".bak")?)?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_keeps_backup() {
        let dir = std::env::temp_dir().join(format!("psx_fsutil_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.json");
        atomic_write(&file, b"one").unwrap();
        atomic_write(&file, b"two").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        assert_eq!(fs::read(dir.join("a.json.bak")).unwrap(), b"one");
        assert!(!dir.join("a.json.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
