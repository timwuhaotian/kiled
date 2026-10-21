//! Files on disk: reading text (refusing what isn't), saving so that a crash or a full disk
//! never leaves a half-written file, and listing a folder for the sidebar and Quick Open
//! (DESIGN.md §4.4).

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::sys;

/// Larger files are not opened.
pub const OPEN_LIMIT: u64 = 256 * 1024 * 1024;

/// How many symlinks a path may pass through, as the kernel allows (`MAXSYMLINKS`).
const MAX_LINKS: usize = 32;

/// Bytes of a file's name kept in its temp file's, which must stay under 255.
const TEMP_NAME_PART: usize = 100;

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
/// The file is replaced whole: written beside it, flushed to disk, then renamed over it, so
/// a crash or a full disk never leaves it half-written. The new file gets the old one's
/// permissions, group, ACL and extended attributes, and a symlink is followed, staying a
/// symlink. Where that can't be done (a hard-linked file, one someone else owns or whose
/// group we can't give, a folder we can't write) the file is written in place, and put back
/// as it was if that fails part way. A file we may not write is refused.
///
/// # Errors
/// If the file can't be written. The file on disk is then as it was.
pub fn save(path: &Path, contents: &str) -> io::Result<Stamp> {
    let path = follow_links(path)?;
    let existing = match fs::metadata(&path) {
        Ok(meta) => Some(meta),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    if let Some(meta) = &existing {
        if !meta.is_file() {
            return Err(io::Error::other("it isn't a regular file"));
        }
        // Renaming over a file needs no right to write it, so that is asked first: its
        // mode, an ACL or a lock may forbid it.
        if let Err(err) = OpenOptions::new().write(true).open(&path) {
            return Err(match err.kind() {
                io::ErrorKind::PermissionDenied => io::Error::new(err.kind(), "it is read-only"),
                _ => err,
            });
        }
    }
    let replaceable = existing.as_ref().is_none_or(|meta| meta.nlink() == 1);
    if !(replaceable && replace(&path, contents, existing.as_ref())?) {
        overwrite(&path, contents)?;
    }
    stamp(&path)
}

/// Where `path` leads through its symlinks; the last may name a file not made yet.
fn follow_links(path: &Path) -> io::Result<PathBuf> {
    let mut path = path.to_owned();
    for _ in 0..MAX_LINKS {
        match fs::read_link(&path) {
            Ok(target) => {
                let dir = path.parent().unwrap_or_else(|| Path::new("/"));
                path = dir.join(target);
            }
            // Not a link, or nothing there: this is the file.
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::InvalidInput | io::ErrorKind::NotFound
                ) =>
            {
                return Ok(path);
            }
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::from_raw_os_error(libc::ELOOP))
}

/// Writes beside `path` and renames over it. `Ok(false)`: the new file can't be made the
/// old one's equal (owner, group, ACL, attributes), or the folder or the old file forbid
/// it; nothing was changed.
fn replace(path: &Path, contents: &str, existing: Option<&fs::Metadata>) -> io::Result<bool> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = dir.join(temp_name(path));
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
        Ok(file) => file,
        Err(err) if matches!(err.kind(), io::ErrorKind::PermissionDenied) => return Ok(false),
        Err(err) if err.raw_os_error() == Some(libc::EROFS) => return Ok(false),
        Err(err) => return Err(err),
    };
    let written = (|| {
        let original = existing.map(|_| File::open(path)).transpose()?;
        if let (Some(meta), Some(original)) = (existing, &original) {
            let made = file.metadata()?;
            if made.uid() != meta.uid() {
                return Ok(false);
            }
            // A new file takes its folder's group; giving it the old one's may be refused.
            if made.gid() != meta.gid()
                && std::os::unix::fs::fchown(&file, None, Some(meta.gid())).is_err()
            {
                return Ok(false);
            }
            if sys::copy_attributes(original, &file).is_err() {
                return Ok(false);
            }
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        match fs::rename(&temp, path) {
            Ok(()) => {}
            // Replacing deletes the old file, which an ACL may forbid where writing it isn't.
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => return Ok(false),
            Err(err) => return Err(err),
        }
        // The ACL comes last: one that forbids deleting would have kept the temp file.
        if let Some(original) = &original
            && let Err(err) = sys::copy_acl(original, &file)
        {
            log::warn!("{}: its ACL wasn't kept: {err}", path.display());
        }
        Ok(true)
    })();
    if !matches!(written, Ok(true)) {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// A hidden name beside `path`, ours alone, and short enough whatever its name.
fn temp_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy());
    let mut end = name.len().min(TEMP_NAME_PART);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    format!(".{}.kiled-{}-{nanos}", &name[..end], std::process::id())
}

/// Writes over the file itself, keeping all about it but its contents: without truncating
/// it first, flushed to disk, and with the old contents put back if that fails part way.
fn overwrite(path: &Path, contents: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let mut old = Vec::new();
    file.read_to_end(&mut old)?;
    let written = write_from_start(&file, contents.as_bytes());
    if written.is_err() {
        let _ = write_from_start(&file, &old);
    }
    written
}

fn write_from_start(file: &File, bytes: &[u8]) -> io::Result<()> {
    file.write_all_at(bytes, 0)?;
    file.set_len(bytes.len() as u64)?;
    file.sync_all()
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

    fn run(program: &str, args: &[&str], path: &Path) -> String {
        let out = std::process::Command::new(program)
            .args(args)
            .arg(path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{program} {args:?} failed");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn save_should_keep_the_group_acl_and_extended_attributes() {
        let dir = scratch("metadata");
        let path = dir.join("a.rs");
        fs::write(&path, "old").unwrap();
        // `everyone` (12) is a group every macOS user is in, and not the folder's.
        std::os::unix::fs::chown(&path, None, Some(12)).unwrap();
        run("chmod", &["+a", "staff allow read"], &path);
        run("xattr", &["-w", "user.kiled", "kept"], &path);
        let before = fs::metadata(&path).unwrap().ino();
        save(&path, "new").unwrap();
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(
            (
                fs::read_to_string(&path).unwrap(),
                meta.ino() != before,
                meta.gid(),
                run("ls", &["-le"], &path).contains("staff allow read"),
                run("xattr", &["-p", "user.kiled"], &path),
            ),
            ("new".to_owned(), true, 12, true, "kept\n".to_owned())
        );
    }

    #[test]
    fn save_should_write_in_place_a_file_that_may_not_be_deleted() {
        let dir = scratch("undeletable");
        let path = dir.join("a.rs");
        fs::write(&path, "old").unwrap();
        run("chmod", &["+a", "everyone deny delete"], &path);
        let before = fs::metadata(&path).unwrap().ino();
        let saved = save(&path, "new").map(|_| ());
        let after = (
            fs::read_to_string(&path).unwrap(),
            fs::metadata(&path).unwrap().ino() == before,
            fs::read_dir(&dir).unwrap().count(),
        );
        run("chmod", &["-N"], &path);
        assert_eq!((saved.ok(), after), (Some(()), ("new".to_owned(), true, 1)));
    }

    #[test]
    fn save_should_refuse_a_read_only_file() {
        let dir = scratch("readonly");
        let path = dir.join("a.rs");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let err = save(&path, "new").unwrap_err();
        assert_eq!(
            (err.kind(), fs::read_to_string(&path).unwrap()),
            (io::ErrorKind::PermissionDenied, "old".to_owned())
        );
    }

    #[test]
    fn save_should_replace_the_file_a_symlink_names_and_keep_the_link() {
        let dir = scratch("symlink");
        let target = dir.join("target.toml");
        let link = dir.join("link.toml");
        fs::write(&target, "old").unwrap();
        symlink("target.toml", &link).unwrap();
        let before = fs::metadata(&target).unwrap().ino();
        save(&link, "new").unwrap();
        assert_eq!(
            (
                fs::read_to_string(&target).unwrap(),
                fs::metadata(&target).unwrap().ino() != before,
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                fs::read_dir(&dir).unwrap().count()
            ),
            ("new".to_owned(), true, true, 2)
        );
    }

    #[test]
    fn save_should_create_the_file_a_dangling_symlink_names() {
        let dir = scratch("dangling");
        let link = dir.join("link.toml");
        symlink("gone.toml", &link).unwrap();
        save(&link, "new").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("gone.toml")).unwrap(),
            "new".to_owned()
        );
    }

    #[test]
    fn save_should_write_a_hard_linked_file_in_place_leaving_no_old_tail() {
        let dir = scratch("hardlink");
        let path = dir.join("a");
        fs::write(&path, "older and longer").unwrap();
        fs::hard_link(&path, dir.join("b")).unwrap();
        save(&path, "new").unwrap();
        assert_eq!(fs::read_to_string(dir.join("b")).unwrap(), "new");
    }

    #[test]
    fn save_should_cope_with_the_longest_file_names() {
        let dir = scratch("long");
        let path = dir.join(format!("{}.rs", "n".repeat(250)));
        fs::write(&path, "old").unwrap();
        save(&path, "new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
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
