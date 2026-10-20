//! Finding a language server. An app opened from the Dock gets launchd's bare `PATH`, while
//! servers live where shell profiles look (Homebrew, npm, rustup, nvm), so the login
//! shell's `PATH` is asked for once.

use std::{
    ffi::{OsStr, OsString},
    io::Read,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use crate::{language::Server, sys};

/// A shell profile slower than this is given up on; the app's own `PATH` is used instead.
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
const MARK: &str = "__KILED_PATH__";

/// The `PATH` an interactive login shell sets, or `None` if it couldn't be read in time.
#[must_use]
pub fn login_path() -> Option<OsString> {
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    let mut child = Command::new(&shell)
        .args([
            "-l",
            "-i",
            "-c",
            &format!("printf '\\n{MARK}%s{MARK}\\n' \"$PATH\""),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (done, output) = mpsc::channel();
    let _ = thread::Builder::new()
        .name("login-path".into())
        .spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            let _ = done.send(text);
        });
    let text = output.recv_timeout(SHELL_TIMEOUT);
    if text.is_err() {
        // Not reaped yet, so the group is still the shell's.
        let _ = sys::kill_group(child.id(), libc::SIGKILL);
    }
    let _ = child.wait();
    let text = text.ok()?;
    let start = text.find(MARK)? + MARK.len();
    let end = start + text[start..].find(MARK)?;
    Some(OsString::from(&text[start..end]))
}

/// The `PATH` servers are looked up on and started with: the login shell's, then the app's,
/// then where developer tools usually live.
#[must_use]
pub fn server_path(login: Option<&OsStr>) -> OsString {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let usual = [
        home.as_ref().map(|home| home.join(".cargo/bin")),
        Some(PathBuf::from("/opt/homebrew/bin")),
        Some(PathBuf::from("/usr/local/bin")),
        home.as_ref().map(|home| home.join(".local/bin")),
        home.as_ref().map(|home| home.join(".bun/bin")),
        home.as_ref().map(|home| home.join(".volta/bin")),
    ];
    let mut dirs: Vec<PathBuf> = Vec::new();
    let own = std::env::var_os("PATH");
    let sources = login
        .into_iter()
        .chain(own.as_deref())
        .flat_map(std::env::split_paths)
        .chain(usual.into_iter().flatten());
    for dir in sources {
        if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// What `program` runs: itself if it is a path (`~` allowed), else the first executable of
/// that name on `path`.
#[must_use]
pub fn find_program(program: &str, path: &OsStr) -> Option<PathBuf> {
    if program.contains('/') {
        let expanded = match program.strip_prefix("~/") {
            Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
            None => PathBuf::from(program),
        };
        return is_executable(&expanded).then_some(expanded);
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

/// The program that runs `server` for a project at `root`: the project's own npm install
/// first, then `PATH`, passing over an npm package older than the server needs.
#[must_use]
pub fn find_server(server: &Server, root: &Path, path: &OsStr) -> Option<PathBuf> {
    let local = server
        .npm
        .map(|_| root.join("node_modules/.bin").join(server.command));
    local
        .into_iter()
        .chain(std::env::split_paths(path).map(|dir| dir.join(server.command)))
        .filter(|candidate| is_executable(candidate))
        .find(|candidate| match server.npm {
            Some((package, minimum)) if minimum > 0 => {
                npm_major(candidate, package).is_some_and(|major| major >= minimum)
            }
            _ => true,
        })
}

/// The major version of the npm `package` a program (or the link to it) belongs to.
fn npm_major(program: &Path, package: &str) -> Option<u32> {
    let real = std::fs::canonicalize(program).ok()?;
    let dir = real.ancestors().find(|dir| {
        dir.file_name().is_some_and(|name| name == package)
            && dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "node_modules")
    })?;
    let manifest = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    manifest["version"]
        .as_str()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_program_should_search_the_path_in_order() {
        assert_eq!(
            find_program("sh", OsStr::new("/nonexistent:/bin:/usr/bin")),
            Some(PathBuf::from("/bin/sh"))
        );
        assert_eq!(
            find_program("no-such-server-kiled", OsStr::new("/bin")),
            None
        );
    }

    #[test]
    fn find_server_should_prefer_the_projects_own_new_enough_install() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("kiled-npm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let install = |root: &Path, version: &str| {
            let package = root.join("node_modules/typescript");
            std::fs::create_dir_all(package.join("bin")).unwrap();
            std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
            std::fs::write(
                package.join("package.json"),
                format!(r#"{{"version": "{version}"}}"#),
            )
            .unwrap();
            let tsc = package.join("bin/tsc");
            std::fs::write(&tsc, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&tsc, std::fs::Permissions::from_mode(0o755)).unwrap();
            symlink("../typescript/bin/tsc", root.join("node_modules/.bin/tsc")).unwrap();
        };
        let old = dir.join("old");
        let new = dir.join("new");
        install(&old, "5.9.3");
        install(&new, "7.0.2");
        let typescript = crate::language::SERVERS
            .iter()
            .find(|server| server.name == "typescript")
            .unwrap();
        let global = new.join("node_modules/.bin");
        assert_eq!(
            (
                find_server(typescript, &old, global.as_os_str()),
                find_server(typescript, &old, OsStr::new("/bin")),
            ),
            (Some(global.join("tsc")), None)
        );
    }

    #[test]
    fn server_path_should_put_the_login_path_first_without_repeats() {
        let path = server_path(Some(OsStr::new("/a:/b:/a")));
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(&dirs[..2], &[PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(
            dirs.iter().filter(|dir| **dir == Path::new("/a")).count(),
            1
        );
    }
}
