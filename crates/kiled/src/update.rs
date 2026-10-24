//! Kiled updates itself (DESIGN.md §13). It reads `releases/latest.toml` on `main`; when that
//! names a newer version, it downloads the DMG beside it, checks the checksum, the version,
//! the Developer ID team and Apple's notarization, and keeps the app from it until Kiled
//! quits. Quitting then replaces the running bundle, so the next launch is the new version.

use std::{
    ffi::OsStr,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use gpui::{App, AppContext as _, Global, MenuItem, actions};
use serde::Deserialize;

use crate::{
    settings::Settings,
    update_window::{self, Shown},
    workspace,
};

actions!(update, [CheckForUpdates, RestartToUpdate]);

/// Raw from `main`, where the release commit puts the DMG and this file together.
const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/timwuhaotian/kiled/main/releases/latest.toml";
const DMG_URL: &str = "https://github.com/timwuhaotian/kiled/raw/main/releases/Kiled-arm64.dmg";
/// After launch, so the check never competes with reopening the session.
const FIRST_CHECK: Duration = Duration::from_secs(5);
/// An editor stays open for days; one wakeup in hours keeps idle CPU at zero (A13).
const CHECK_EVERY: Duration = Duration::from_hours(6);
/// How often a download's size is read, and the update window redraws its bar.
pub const POLL: Duration = Duration::from_millis(100);

/// What a check is doing, for the update window to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Checking,
    /// The DMG's bytes so far, and its size once the server has said it.
    Downloading {
        version: String,
        got: u64,
        total: Option<u64>,
    },
    Verifying {
        version: String,
    },
}

/// A check's current step: written by its thread, read by the update window.
#[derive(Debug, Default)]
pub struct Progress(Mutex<Step>);

impl Progress {
    pub fn step(&self) -> Step {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set(&self, step: Step) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = step;
    }
}

/// `releases/latest.toml`: the release's version and its DMG's SHA-256.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: String,
    pub sha256: String,
}

impl Manifest {
    /// # Errors
    /// If `text` isn't a manifest.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|err| format!("latest.toml: {err}"))
    }
}

/// `major.minor.patch`, compared as numbers.
fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.trim().split('.').map(|part| part.parse().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// Whether `candidate` is a later version than `current`; anything unreadable is not.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!(
        (parse_version(candidate), parse_version(current)),
        (Some(candidate), Some(current)) if candidate > current
    )
}

/// The `.app` that `exe` runs in, if Kiled may replace it: not a bare binary (`cargo run`),
/// not the copy inside the DMG, and not one Gatekeeper translocated to a read-only place.
fn replaceable_bundle(exe: &Path) -> Result<PathBuf, &'static str> {
    let bundle = exe
        .parent()
        .filter(|dir| dir.ends_with("Contents/MacOS"))
        .and_then(|dir| dir.parent()?.parent())
        .filter(|bundle| bundle.extension().is_some_and(|ext| ext == "app"))
        .ok_or("Kiled isn't running from Kiled.app")?;
    let path = bundle.to_string_lossy();
    if path.starts_with("/Volumes/") || path.contains("/AppTranslocation/") {
        return Err("Kiled is running from the disk image: drag it to Applications first");
    }
    Ok(bundle.to_owned())
}

/// Runs `program`; a failure is its stderr, else how it ended.
fn run(program: &str, args: &[&OsStr]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    Err(failure(program, output.status, &output.stderr))
}

fn failure(program: &str, status: ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let name = Path::new(program).file_name().unwrap_or_default().display();
    match stderr.trim() {
        "" => format!("{name}: {status}"),
        why => format!("{name}: {why}"),
    }
}

/// The body size the last response in a `curl -D` dump gives; redirects come before it.
fn content_length(headers: &str) -> Option<u64> {
    let last = headers
        .rsplit_once("\nHTTP/")
        .map_or(headers, |(_, last)| last);
    last.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .filter(|&size| size > 0)
}

/// Downloads `url` to `to`, telling `progress` how much has arrived, by the file's size and
/// the headers curl writes before the body.
fn download(url: &str, to: &Path, version: &str, progress: &Progress) -> Result<(), String> {
    const CURL: &str = "/usr/bin/curl";
    let headers = to.with_extension("headers");
    let mut curl = Command::new(CURL)
        .args(["-fsSL", "--max-time", "900", "-D"])
        .arg(&headers)
        .arg("-o")
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{CURL}: {err}"))?;
    let report = || {
        progress.set(Step::Downloading {
            version: version.to_owned(),
            got: fs::metadata(to).map_or(0, |file| file.len()),
            total: fs::read_to_string(&headers)
                .ok()
                .and_then(|headers| content_length(&headers)),
        });
    };
    let status = loop {
        report();
        match curl.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(POLL),
            Err(err) => return Err(format!("{CURL}: {err}")),
        }
    };
    report();
    let _ = fs::remove_file(&headers);
    if status.success() {
        return Ok(());
    }
    let mut stderr = Vec::new();
    if let Some(mut pipe) = curl.stderr.take() {
        let _ = pipe.read_to_end(&mut stderr);
    }
    Err(failure(CURL, status, &stderr))
}

/// The Developer ID team that signed `app`; `None` for an ad-hoc signature (a local build).
fn team_of(app: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2"])
        .arg(app)
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .filter(|team| *team != "not set")
        .map(str::to_owned)
}

/// The version `app`'s Info.plist shows.
fn bundle_version(app: &Path) -> Result<String, String> {
    let plist = app.join("Contents/Info.plist");
    run(
        "/usr/libexec/PlistBuddy",
        &[
            "-c".as_ref(),
            "Print :CFBundleShortVersionString".as_ref(),
            plist.as_os_str(),
        ],
    )
}

/// Checks that `app` is Kiled `version`, signed by `team` and notarized by Apple.
fn verify(app: &Path, version: &str, team: &str) -> Result<(), String> {
    let shown = bundle_version(app)?;
    if shown != version {
        return Err(format!("the download is Kiled {shown}, not {version}"));
    }
    let requirement =
        format!("=anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
    run(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            "-R".as_ref(),
            requirement.as_ref(),
            app.as_os_str(),
        ],
    )
    .map_err(|err| format!("the download isn't signed as Kiled is ({err})"))?;
    run(
        "/usr/sbin/spctl",
        &[
            "--assess".as_ref(),
            "--type".as_ref(),
            "execute".as_ref(),
            app.as_os_str(),
        ],
    )
    .map_err(|err| format!("Apple hasn't notarized the download ({err})"))?;
    Ok(())
}

/// Reads the manifest at `url`.
///
/// # Errors
/// If it can't be fetched or parsed.
pub fn fetch_manifest(url: &str) -> Result<Manifest, String> {
    let text = run(
        "/usr/bin/curl",
        &[
            "-fsSL".as_ref(),
            "--max-time".as_ref(),
            "30".as_ref(),
            url.as_ref(),
        ],
    )?;
    Manifest::parse(&text)
}

/// Downloads the DMG at `dmg_url` into `dir`, checks it against `manifest` and `team`, and
/// returns the app copied out of it. An app staged earlier for the same version is reused.
///
/// # Errors
/// If the download fails or any check does.
pub fn stage(
    manifest: &Manifest,
    dmg_url: &str,
    dir: &Path,
    team: &str,
    progress: &Progress,
) -> Result<PathBuf, String> {
    let version = &manifest.version;
    let verifying = || {
        progress.set(Step::Verifying {
            version: version.clone(),
        });
    };
    let app = dir.join("Kiled.app");
    if app.exists() {
        verifying();
        if verify(&app, version, team).is_ok() {
            return Ok(app);
        }
    }
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let dmg = dir.join("Kiled.dmg");
    download(dmg_url, &dmg, version, progress)?;
    verifying();
    let sum = run(
        "/usr/bin/shasum",
        &["-a".as_ref(), "256".as_ref(), dmg.as_os_str()],
    )?;
    if !sum
        .split_whitespace()
        .next()
        .is_some_and(|sum| sum.eq_ignore_ascii_case(&manifest.sha256))
    {
        return Err("the download doesn't match its checksum".to_owned());
    }
    let mount = dir.join("mnt");
    run(
        "/usr/bin/hdiutil",
        &[
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            "-readonly".as_ref(),
            "-noautoopen".as_ref(),
            "-quiet".as_ref(),
            "-mountpoint".as_ref(),
            mount.as_os_str(),
            dmg.as_os_str(),
        ],
    )?;
    let copied = run(
        "/usr/bin/ditto",
        &[mount.join("Kiled.app").as_os_str(), app.as_os_str()],
    );
    let detach = |force: bool| {
        let mut args = vec!["detach".as_ref(), "-quiet".as_ref(), mount.as_os_str()];
        if force {
            args.push("-force".as_ref());
        }
        run("/usr/bin/hdiutil", &args)
    };
    if detach(false).is_err() {
        let _ = detach(true);
    }
    copied?;
    let _ = fs::remove_file(&dmg);
    verify(&app, version, team).inspect_err(|_| {
        let _ = fs::remove_dir_all(&app);
    })?;
    Ok(app)
}

/// Replaces `bundle` with `staged`: copied in beside it, then two renames, so `bundle` is never
/// missing or half-written, and a failure leaves the old app in place. The running app keeps
/// the code it loaded.
///
/// # Errors
/// If the folder can't be written or a rename fails.
pub fn install(staged: &Path, bundle: &Path) -> Result<(), String> {
    let (Some(parent), Some(name)) = (bundle.parent(), bundle.file_name()) else {
        return Err(format!("{}: not an app", bundle.display()));
    };
    let name = name.to_string_lossy();
    let new = parent.join(format!(".{name}.new"));
    let old = parent.join(format!(".{name}.old"));
    for leftover in [&new, &old] {
        let _ = fs::remove_dir_all(leftover);
    }
    run("/usr/bin/ditto", &[staged.as_os_str(), new.as_os_str()])?;
    if let Err(err) = fs::rename(bundle, &old) {
        let _ = fs::remove_dir_all(&new);
        return Err(format!("{}: {err}", bundle.display()));
    }
    if let Err(err) = fs::rename(&new, bundle) {
        let _ = fs::rename(&old, bundle);
        let _ = fs::remove_dir_all(&new);
        return Err(format!("{}: {err}", bundle.display()));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// Whether Kiled can create files in `dir`, which replacing the bundle needs.
fn writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".kiled-update-{}", std::process::id()));
    fs::create_dir(&probe)
        .and_then(|()| fs::remove_dir(&probe))
        .map_err(|err| format!("Kiled can't replace itself in {} ({err})", dir.display()))
}

/// `~/Library/Caches/dev.kil.editor/update`.
fn cache_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Caches/dev.kil.editor/update"))
        .ok_or_else(|| "no home directory".to_owned())
}

/// A newer Kiled, checked and waiting for Kiled to quit.
#[derive(Clone)]
struct Ready {
    version: String,
    app: PathBuf,
    bundle: PathBuf,
}

enum Found {
    UpToDate,
    Ready(Ready),
}

/// One check, start to end; blocks, so it runs on a background thread.
fn find(current: &str, progress: &Progress) -> Result<Found, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let bundle = replaceable_bundle(&exe)?;
    let team = team_of(&bundle).ok_or(
        "this copy of Kiled isn't signed with a Developer ID, so it doesn't update itself",
    )?;
    let manifest = fetch_manifest(MANIFEST_URL)?;
    if !is_newer(&manifest.version, current) {
        return Ok(Found::UpToDate);
    }
    writable(bundle.parent().unwrap_or(&bundle))?;
    let app = stage(&manifest, DMG_URL, &cache_dir()?, &team, progress)?;
    Ok(Found::Ready(Ready {
        version: manifest.version,
        app,
        bundle,
    }))
}

#[derive(Default)]
struct Updater {
    /// The check under way.
    checking: Option<Arc<Progress>>,
    /// Someone chose Check for Updates, and is told the outcome.
    asked: bool,
    ready: Option<Ready>,
    /// Restart to Update: open the new app once this one has quit.
    reopen: bool,
}

impl Global for Updater {}

/// Starts the checks and installs a staged update whenever Kiled quits.
pub fn init(cx: &mut App) {
    cx.set_global(Updater::default());
    cx.on_action(|_: &CheckForUpdates, cx| check_now(cx));
    cx.on_action(|_: &RestartToUpdate, cx| restart(cx));
    cx.on_app_quit(|cx| {
        install_on_quit(cx);
        async {}
    })
    .detach();
    cx.spawn(async move |cx| {
        let mut wait = FIRST_CHECK;
        loop {
            cx.background_executor().timer(wait).await;
            wait = CHECK_EVERY;
            let checked = cx.update(|cx| {
                if cx.global::<Settings>().auto_update {
                    check(false, cx);
                }
            });
            if checked.is_err() {
                return;
            }
        }
    })
    .detach();
}

/// The Kiled menu's update item: Check for Updates…, or Restart to Update once one is ready.
pub fn menu_item(cx: &App) -> MenuItem {
    match cx
        .try_global::<Updater>()
        .and_then(|updater| updater.ready.as_ref())
    {
        Some(ready) => MenuItem::action(
            format!("Restart to Update to {}", ready.version),
            RestartToUpdate,
        ),
        None => MenuItem::action("Check for Updates…", CheckForUpdates),
    }
}

/// Check for Updates…: a check whose progress and outcome the update window shows.
pub fn check_now(cx: &mut App) {
    check(true, cx);
}

/// Restart to Update: quits as ⌘Q does, asking about unsaved files, then installs and opens
/// the new Kiled.
pub fn restart(cx: &mut App) {
    if cx.global::<Updater>().ready.is_some() {
        workspace::quit_to_update(cx);
    }
}

/// A Restart to Update quit is going ahead: once this process is gone, the new Kiled opens.
pub fn reopen_on_quit(cx: &mut App) {
    cx.global_mut::<Updater>().reopen = true;
}

fn check(asked: bool, cx: &mut App) {
    let updater = cx.global_mut::<Updater>();
    if let Some(ready) = &updater.ready {
        if asked {
            let shown = Shown::Ready(ready.version.clone());
            update_window::show(shown, cx);
        }
        return;
    }
    updater.asked |= asked;
    let progress = match updater.checking.clone() {
        Some(progress) => progress,
        None => start(cx),
    };
    if asked {
        update_window::show(Shown::Working(progress), cx);
    }
}

/// Starts a check on a background thread; `checked` gets its outcome.
fn start(cx: &mut App) -> Arc<Progress> {
    let progress = Arc::new(Progress::default());
    cx.global_mut::<Updater>().checking = Some(progress.clone());
    let found = cx.background_spawn({
        let progress = progress.clone();
        async move { find(env!("CARGO_PKG_VERSION"), &progress) }
    });
    cx.spawn(async move |cx| {
        let found = found.await;
        let _ = cx.update(|cx| checked(found, cx));
    })
    .detach();
    progress
}

fn checked(found: Result<Found, String>, cx: &mut App) {
    let updater = cx.global_mut::<Updater>();
    updater.checking = None;
    let asked = std::mem::take(&mut updater.asked);
    let shown = match found {
        Ok(Found::Ready(ready)) => {
            log::info!("Kiled {} is ready to install", ready.version);
            let version = ready.version.clone();
            updater.ready = Some(ready);
            cx.set_menus(crate::menus(cx));
            Shown::Ready(version)
        }
        Ok(Found::UpToDate) => Shown::UpToDate,
        Err(err) => {
            log::warn!("update check: {err}");
            Shown::Failed(err)
        }
    };
    if asked {
        update_window::show(shown, cx);
    }
}

fn install_on_quit(cx: &mut App) {
    let Some(updater) = cx.try_global::<Updater>() else {
        return;
    };
    let Some(ready) = updater.ready.clone() else {
        return;
    };
    let reopen = updater.reopen;
    // The bundle may have been replaced by hand since the update was staged (a maintainer
    // installing a release); a newer app there stays, and the stale download goes.
    let on_disk = bundle_version(&ready.bundle).ok();
    let stale = on_disk
        .as_deref()
        .is_some_and(|on_disk| !is_newer(&ready.version, on_disk));
    if stale {
        log::info!(
            "Kiled {} is installed already; not replacing it with {}",
            on_disk.unwrap_or_default(),
            ready.version
        );
    } else if let Err(err) = install(&ready.app, &ready.bundle) {
        log::error!("installing Kiled {}: {err}", ready.version);
        return;
    }
    if let Some(dir) = ready.app.parent() {
        let _ = fs::remove_dir_all(dir);
    }
    if reopen {
        // Once this process is gone, so macOS opens the new bundle and not this one again.
        let spawned = Command::new("/bin/sh")
            .args([
                "-c",
                r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$2""#,
                "sh",
            ])
            .arg(std::process::id().to_string())
            .arg(&ready.bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Err(err) = spawned {
            log::error!("reopening Kiled: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_should_compare_numbers_not_text() {
        assert_eq!(
            [
                is_newer("0.0.10", "0.0.9"),
                is_newer("0.1.0", "0.0.99"),
                is_newer("0.0.5", "0.0.5"),
                is_newer("0.0.4", "0.0.5"),
                is_newer("0.0.x", "0.0.5"),
                is_newer("0.0.6.1", "0.0.5"),
            ],
            [true, true, false, false, false, false]
        );
    }

    #[test]
    fn manifest_should_parse_and_reject_unknown_keys() {
        assert_eq!(
            Manifest::parse("version = \"0.0.6\"\nsha256 = \"ab\"\n"),
            Ok(Manifest {
                version: "0.0.6".into(),
                sha256: "ab".into()
            })
        );
        assert!(Manifest::parse("version = \"0.0.6\"\nurl = \"x\"\n").is_err());
    }

    #[test]
    fn only_an_installed_bundle_should_be_replaceable() {
        let bundle = |exe: &str| replaceable_bundle(Path::new(exe)).ok();
        assert_eq!(
            [
                bundle("/Applications/Kiled.app/Contents/MacOS/kiled"),
                bundle("/Users/me/code/kiled/target/release/kiled"),
                bundle("/Volumes/Kiled/Kiled.app/Contents/MacOS/kiled"),
                bundle(
                    "/private/var/folders/x/AppTranslocation/1/d/Kiled.app/Contents/MacOS/kiled"
                ),
            ],
            [
                Some(PathBuf::from("/Applications/Kiled.app")),
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn content_length_should_be_the_last_responses() {
        let redirected = "HTTP/2 302 \r\ncontent-length: 0\r\n\r\nHTTP/2 200 \r\nContent-Length: 4570610\r\n\r\n";
        assert_eq!(
            [
                content_length(redirected),
                content_length("HTTP/2 302 \r\ncontent-length: 0\r\n\r\n"),
                content_length("HTTP/2 302 \r\ncontent-length: 9\r\n\r\nHTTP/2 200 \r\n"),
                content_length("Content-Length: 94\r\nAccept-ranges: bytes\r\n"),
            ],
            [Some(4_570_610), None, None, Some(94)]
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kiled-update-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_app(at: &Path, version: &str) {
        fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        fs::write(at.join("Contents/MacOS/kiled"), version).unwrap();
    }

    #[test]
    fn install_should_swap_the_bundle_and_leave_nothing_beside_it() {
        let dir = scratch("install");
        let (bundle, staged) = (dir.join("Apps/Kiled.app"), dir.join("cache/Kiled.app"));
        fake_app(&bundle, "old");
        fake_app(&staged, "new");
        install(&staged, &bundle).unwrap();
        let mut left: Vec<_> = fs::read_dir(dir.join("Apps"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(
            (
                fs::read_to_string(bundle.join("Contents/MacOS/kiled")).unwrap(),
                left
            ),
            (
                "new".to_owned(),
                vec![std::ffi::OsString::from("Kiled.app")]
            )
        );
    }

    #[test]
    fn install_should_keep_the_old_app_when_the_new_one_is_missing() {
        let dir = scratch("missing");
        let bundle = dir.join("Kiled.app");
        fake_app(&bundle, "old");
        assert!(install(&dir.join("nowhere/Kiled.app"), &bundle).is_err());
        assert_eq!(
            fs::read_to_string(bundle.join("Contents/MacOS/kiled")).unwrap(),
            "old"
        );
    }

    #[test]
    fn download_should_report_every_byte_of_a_known_size() {
        let dmg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../releases/Kiled-arm64.dmg");
        let size = fs::metadata(&dmg).unwrap().len();
        let to = scratch("download").join("Kiled.dmg");
        let progress = Progress::default();
        let url = format!("file://{}", fs::canonicalize(&dmg).unwrap().display());
        download(&url, &to, "9.9.9", &progress).unwrap();
        assert_eq!(
            (progress.step(), to.with_extension("headers").exists()),
            (
                Step::Downloading {
                    version: "9.9.9".into(),
                    got: size,
                    total: Some(size),
                },
                false
            )
        );
        let missing = download("file:///nowhere/Kiled.dmg", &to, "9.9.9", &progress);
        assert!(missing.is_err_and(|err| err.starts_with("curl: ")));
    }

    /// The release on `main` end to end, from the repository instead of GitHub: the manifest
    /// matches the DMG, and the app in it passes every check an update makes.
    #[test]
    fn the_committed_release_should_pass_the_checks_an_update_makes() {
        let releases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../releases");
        let manifest =
            Manifest::parse(&fs::read_to_string(releases.join("latest.toml")).unwrap()).unwrap();
        let dmg = fs::canonicalize(releases.join("Kiled-arm64.dmg")).unwrap();
        let dir = scratch("stage");
        let url = format!("file://{}", dmg.display());
        let progress = Progress::default();
        let staged = stage(&manifest, &url, &dir, "43VLF3KTFZ", &progress).unwrap();
        assert!(staged.join("Contents/MacOS/kiled").is_file());
        assert_eq!(
            progress.step(),
            Step::Verifying {
                version: manifest.version.clone()
            }
        );
        assert!(verify(&staged, &manifest.version, "AAAAAAAAAA").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
