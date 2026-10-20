//! Files on disk: reading text (refusing what isn't), saving so that a crash or a full disk
//! never leaves a half-written file, and listing a folder for the sidebar and Quick Open
//! (DESIGN.md §4.4).

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Larger files are not opened.
pub const OPEN_LIMIT: u64 = 256 * 1024 * 1024;

/// Why a file wasn't opened.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// Reading failed.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// A NUL byte near the start, as git decides.
    #[error("it looks like a binary file")]
    Binary,
    /// Not valid UTF-8.
    #[error("it isn't UTF-8 text")]
    NotUtf8,
    /// Above [`OPEN_LIMIT`].
    #[error("it is {0} MB, more than the {limit} MB Kiled opens", limit = OPEN_LIMIT >> 20)]
    TooLarge(u64),
}

/// What a file looked like when it was last read or written, to notice changes made
/// elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// Modification time.
    pub modified: Option<SystemTime>,
    /// Size in bytes.
    pub len: u64,
}

/// The file's stamp now.
///
/// # Errors
/// If it can't be read (e.g. it was deleted).
pub fn stamp(path: &Path) -> io::Result<Stamp> {
    let meta = fs::metadata(path)?;
    Ok(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

/// Reads a text file.
///
/// # Errors
/// If it can't be read, is too large, looks binary or isn't UTF-8.
pub fn load(path: &Path) -> Result<(String, Stamp), LoadError> {
    let stamped = stamp(path)?;
    if stamped.len > OPEN_LIMIT {
        return Err(LoadError::TooLarge(stamped.len >> 20));
    }
    let bytes = fs::read(path)?;
    if bytes.iter().take(8000).any(|&byte| byte == 0) {
        return Err(LoadError::Binary);
    }
    let text = String::from_utf8(bytes).map_err(|_| LoadError::NotUtf8)?;
    Ok((text, stamped))
}

/// Writes `contents` to `path` and returns its new stamp.
///
/// A plain file we own, with one link, is replaced whole: written beside it, flushed to
/// disk, then renamed over it, keeping its permissions. A symlink, a hard-linked file or a
/// file someone else owns is written in place instead, so the link, the other names and the
/// owner stay what they were; so is a file in a folder we can't write.
///
/// # Errors
/// If the file can't be written. The file on disk is then as it was, except after an
/// in-place write that failed part way.
pub fn save(path: &Path, contents: &str) -> io::Result<Stamp> {
    let existing = match fs::symlink_metadata(path) {
        Ok(meta) => Some(meta),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let replaceable = existing
        .as_ref()
        .is_none_or(|meta| meta.file_type().is_file() && meta.nlink() == 1);
    if !(replaceable && replace(path, contents, existing.as_ref())?) {
        fs::write(path, contents)?;
    }
    stamp(path)
}

/// Writes beside `path` and renames over it. `Ok(false)`: it can't be done this way (no
/// write access to the folder, or the file's owner isn't us), and nothing was changed.
fn replace(path: &Path, contents: &str, existing: Option<&fs::Metadata>) -> io::Result<bool> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy());
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let temp = dir.join(format!(".{name}.kiled-{}-{nanos}", std::process::id()));
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
        Ok(file) => file,
        Err(err) if matches!(err.kind(), io::ErrorKind::PermissionDenied) => return Ok(false),
        Err(err) if err.raw_os_error() == Some(libc::EROFS) => return Ok(false),
        Err(err) => return Err(err),
    };
    let written = (|| {
        if let Some(meta) = existing {
            if file.metadata()?.uid() != meta.uid() {
                return Ok(false);
            }
            fs::set_permissions(&temp, meta.permissions())?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(true)
    })();
    if !matches!(written, Ok(true)) {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// One entry of a folder, for the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// File name.
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    /// A folder (or a link to one).
    pub is_dir: bool,
}

/// Never shown: git's own folder and Finder's litter.
fn hidden(name: &str) -> bool {
    matches!(name, ".git" | ".DS_Store")
}

/// The folder's entries: folders first, then files, each by name ignoring case.
///
/// # Errors
/// If the folder can't be read.
pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if hidden(&name) {
                return None;
            }
            let path = entry.path();
            let is_dir = fs::metadata(&path).is_ok_and(|meta| meta.is_dir());
            Some(Entry { name, path, is_dir })
        })
        .collect();
    entries.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));
    Ok(entries)
}

/// Every file under `root` that `.gitignore` doesn't exclude, relative to it, sorted; at most
/// `limit` of them.
pub fn list_project(root: &Path, limit: usize) -> Vec<PathBuf> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| !hidden(&entry.file_name().to_string_lossy()))
        .build();
    let mut files: Vec<PathBuf> = walker
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .take(limit)
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kiled-files-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_should_refuse_binary_and_non_utf8_files() {
        let dir = scratch("load");
        fs::write(dir.join("bin"), b"ab\0cd").unwrap();
        fs::write(dir.join("latin1"), b"caf\xe9").unwrap();
        assert!(matches!(load(&dir.join("bin")), Err(LoadError::Binary)));
        assert!(matches!(load(&dir.join("latin1")), Err(LoadError::NotUtf8)));
    }

    #[test]
    fn save_should_replace_a_plain_file_and_keep_its_permissions() {
        let dir = scratch("replace");
        let path = dir.join("a.sh");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let before = fs::metadata(&path).unwrap().ino();
        save(&path, "new").unwrap();
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(
            (
                fs::read_to_string(&path).unwrap(),
                meta.permissions().mode() & 0o777,
                meta.ino() != before,
                fs::read_dir(&dir).unwrap().count()
            ),
            ("new".to_owned(), 0o755, true, 1)
        );
    }

    #[test]
    fn save_should_write_through_a_symlink_and_keep_it() {
        let dir = scratch("symlink");
        let target = dir.join("target.toml");
        let link = dir.join("link.toml");
        fs::write(&target, "old").unwrap();
        symlink(&target, &link).unwrap();
        save(&link, "new").unwrap();
        assert_eq!(
            (
                fs::read_to_string(&target).unwrap(),
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            ),
            ("new".to_owned(), true)
        );
    }

    #[test]
    fn save_should_write_a_hard_linked_file_in_place() {
        let dir = scratch("hardlink");
        let path = dir.join("a");
        fs::write(&path, "old").unwrap();
        fs::hard_link(&path, dir.join("b")).unwrap();
        save(&path, "new").unwrap();
        assert_eq!(fs::read_to_string(dir.join("b")).unwrap(), "new");
    }

    #[test]
    fn save_should_create_a_new_file() {
        let dir = scratch("create");
        save(&dir.join("new.rs"), "fn main() {}\n").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("new.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn list_project_should_follow_gitignore_and_skip_dot_git() {
        let dir = scratch("project");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        fs::write(dir.join("src/main.rs"), "").unwrap();
        fs::write(dir.join("target/debug/out"), "").unwrap();
        fs::write(dir.join(".git/HEAD"), "").unwrap();
        assert_eq!(
            list_project(&dir, 100),
            vec![PathBuf::from(".gitignore"), PathBuf::from("src/main.rs")]
        );
    }

    #[test]
    fn list_dir_should_put_folders_first() {
        let dir = scratch("dir");
        fs::create_dir_all(dir.join("zeta")).unwrap();
        fs::write(dir.join("Alpha.rs"), "").unwrap();
        fs::write(dir.join(".DS_Store"), "").unwrap();
        let names: Vec<String> = list_dir(&dir)
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["zeta", "Alpha.rs"]);
    }
}
