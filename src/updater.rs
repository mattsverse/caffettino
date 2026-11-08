//! Self-updates from Caffettino's own GitHub releases, fetched with the system `curl`.
//! A release ships the notarized universal app as `Caffettino-<version>-universal.zip`;
//! installing swaps it in for the running bundle once it passes Gatekeeper and carries
//! the running app's Developer ID team. Anything else (a local build, a translocated copy) can only
//! point at the release page.

use serde::Deserialize;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const LATEST_RELEASE: &str = "https://api.github.com/repos/mattsverse/caffettino/releases/latest";
/// Release assets must come from the project's own release downloads.
const DOWNLOADS: &str = "https://github.com/mattsverse/caffettino/releases/download/";

/// A stable `major.minor.patch` version, like the release tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u64, u64, u64);

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text
            .strip_prefix('v')
            .unwrap_or(text)
            .split('.')
            .map(|part| {
                // Signs and leading zeros aren't versions (release.py rejects them too).
                let canonical = !part.is_empty()
                    && part.bytes().all(|byte| byte.is_ascii_digit())
                    && (part == "0" || !part.starts_with('0'));
                canonical.then(|| part.parse().ok()).flatten()
            });
        let version = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is major.minor.patch")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release page, for when the update can't be installed in place.
    pub url: String,
    /// The app archive's download URL, when the release ships one.
    asset: Option<String>,
}

/// The latest published release, when it's newer than the running build.
/// GitHub's "latest" never returns drafts or prereleases.
pub fn check() -> io::Result<Option<Release>> {
    let body = curl(&["--max-time", "30", LATEST_RELEASE])?;
    parse_latest(&body, current_version())
}

fn parse_latest(body: &str, current: Version) -> io::Result<Option<Release>> {
    let latest: Latest = serde_json::from_str(body).map_err(io::Error::other)?;
    let version = Version::parse(&latest.tag_name).ok_or_else(|| {
        io::Error::other(format!("release tag {} is not a version", latest.tag_name))
    })?;
    if version <= current {
        return Ok(None);
    }
    let name = asset_name(version);
    let asset = latest
        .assets
        .into_iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.browser_download_url)
        .filter(|url| url.starts_with(DOWNLOADS));
    Ok(Some(Release {
        version,
        url: latest.html_url,
        asset,
    }))
}

fn asset_name(version: Version) -> String {
    format!("Caffettino-{version}-universal.zip")
}

/// Whether `install` can swap the release in for the running app, rather than the
/// user downloading it. Only a Developer ID-signed app can verify the update's team.
pub fn can_install(release: &Release) -> bool {
    // A dev build would replace its own target directory with a release.
    !cfg!(debug_assertions)
        && release.asset.is_some()
        && install_target().is_ok_and(|app| team_id(&app).is_some())
}

/// Downloads the release, checks it, and swaps it in for the running app, which keeps
/// running the old build until it restarts. Returns the app to relaunch.
pub fn install(release: &Release) -> io::Result<PathBuf> {
    let url = release.asset.as_deref().ok_or_else(|| {
        io::Error::other(format!("release {} has no app download", release.version))
    })?;
    let target = install_target()?;
    let team = team_id(&target)
        .ok_or_else(|| io::Error::other("the running app has no Developer ID signature"))?;
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("the app has no parent folder"))?;
    // Stage next to the app so the swap is a rename within one volume.
    let staging = parent.join(".caffettino-update");
    fs::remove_dir_all(&staging).ok();
    fs::create_dir(&staging).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("can't write to {}: {error}", parent.display()),
        )
    })?;
    let result = unpack(url, &staging)
        .and_then(|fresh| verify(&fresh, &team, release.version).map(|()| fresh))
        .and_then(|fresh| swap(&fresh, &staging, &target));
    fs::remove_dir_all(&staging).ok();
    result.map(|()| target)
}

fn unpack(url: &str, staging: &Path) -> io::Result<PathBuf> {
    let archive = staging.join("Caffettino.zip");
    let archive_arg = archive
        .to_str()
        .ok_or_else(|| io::Error::other("the app's folder path isn't UTF-8"))?;
    curl(&["--max-time", "600", "--output", archive_arg, url])?;
    run(Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(&archive)
        .arg(staging))?;
    let fresh = staging.join("Caffettino.app");
    if fresh.is_dir() {
        Ok(fresh)
    } else {
        Err(io::Error::other(
            "the download doesn't contain Caffettino.app",
        ))
    }
}

/// The new app must pass Gatekeeper (signed and notarized), come from the same
/// Developer ID team as the running app, and be the version the release names.
fn verify(app: &Path, team: &str, version: Version) -> io::Result<()> {
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app))?;
    run(Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute"])
        .arg(app))?;
    if team_id(app).as_deref() != Some(team) {
        return Err(io::Error::other(
            "the update isn't signed by Caffettino's developer",
        ));
    }
    let plist = app.join("Contents/Info.plist");
    let bundled = run(Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print CFBundleShortVersionString"])
        .arg(&plist))?;
    if Version::parse(bundled.trim()) != Some(version) {
        return Err(io::Error::other(format!(
            "the download is version {}, not {version}",
            bundled.trim()
        )));
    }
    Ok(())
}

fn swap(fresh: &Path, staging: &Path, target: &Path) -> io::Result<()> {
    // A bundle can't be renamed over, so move the old one aside first.
    let old = staging.join("previous.app");
    fs::rename(target, &old)?;
    if let Err(error) = fs::rename(fresh, target) {
        fs::rename(&old, target).ok();
        return Err(error);
    }
    Ok(())
}

/// The running app bundle.
fn install_target() -> io::Result<PathBuf> {
    let app = std::env::current_exe()?
        .canonicalize()?
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("not running from an app bundle"))?;
    // Gatekeeper runs quarantined apps from a read-only random path; replacing it
    // would leave the real copy untouched.
    if app
        .components()
        .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return Err(io::Error::other(
            "running from a translocated copy; move Caffettino to Applications",
        ));
    }
    Ok(app)
}

/// The Developer ID team that signed `app`; `None` for ad-hoc or unsigned builds.
fn team_id(app: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=2"])
        .arg(app)
        .output()
        .ok()?;
    // codesign prints signature details on stderr.
    parse_team_id(&String::from_utf8_lossy(&output.stderr))
}

fn parse_team_id(details: &str) -> Option<String> {
    details
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|team| !team.is_empty() && *team != "not set")
        .map(str::to_owned)
}

/// Starts `app` once this process exits, as opening it from Finder does. The cup leaves
/// the menu bar first, so the two copies never share it.
pub fn relaunch(app: &Path) -> io::Result<()> {
    restart_command(app, std::process::id()).spawn().map(drop)
}

fn restart_command(app: &Path, pid: u32) -> Command {
    // The PID and path are arguments, never shell text, so any path survives.
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("while kill -0 \"$1\" 2>/dev/null; do sleep 0.1; done; exec /usr/bin/open -n \"$2\"")
        .arg("caffettino-restart")
        .arg(pid.to_string())
        .arg(app);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Outlive this process instead of sharing its process group.
        command.process_group(0);
    }
    command
}

fn curl(args: &[&str]) -> io::Result<String> {
    let user_agent = format!("Caffettino/{}", current_version());
    run(Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--user-agent",
            &user_agent,
            "--header",
            "Accept: application/vnd.github+json",
        ])
        .args(args))
}

/// Runs `command`, returning its stdout, or its stderr as the error.
fn run(command: &mut Command) -> io::Result<String> {
    let output = command.output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let program = Path::new(command.get_program())
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::other(format!("{program}: {}", stderr.trim())))
    }
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[cfg(test)]
pub mod tests {
    use super::*;

    const REPOSITORY: &str = "mattsverse/caffettino";

    /// A release that can be installed, for state tests.
    pub fn release(version: Version, installable: bool) -> Release {
        Release {
            version,
            url: format!("https://github.com/{REPOSITORY}/releases/tag/v{version}"),
            asset: installable.then(|| format!("{DOWNLOADS}v{version}/{}", asset_name(version))),
        }
    }

    fn latest(tag: &str, asset_url: &str) -> String {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/{REPOSITORY}/releases/tag/{tag}"),
            "assets": [
                {"name": "Caffettino-0.2.0-universal.dmg", "browser_download_url": "https://example.com/a.dmg"},
                {"name": "Caffettino-0.2.0-universal.zip", "browser_download_url": asset_url},
            ],
        })
        .to_string()
    }

    #[test]
    fn versions_are_strict_major_minor_patch_and_compare_numerically() {
        assert_eq!(Version::parse("v0.10.2"), Some(Version(0, 10, 2)));
        assert_eq!(Version::parse("1.0.0"), Some(Version(1, 0, 0)));
        for bad in [
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.3-beta",
            "+1.2.3",
            "v",
            "",
            "1..3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert_eq!(Version(1, 2, 3).to_string(), "1.2.3");
        assert!(current_version() >= Version(0, 1, 0));
    }

    #[test]
    fn a_newer_release_offers_its_app_archive() {
        let url = format!("{DOWNLOADS}v0.2.0/Caffettino-0.2.0-universal.zip");
        let release = parse_latest(&latest("v0.2.0", &url), Version(0, 1, 0))
            .unwrap()
            .unwrap();
        assert_eq!(release.version, Version(0, 2, 0));
        assert_eq!(release.asset.as_deref(), Some(url.as_str()));
        assert!(release.url.ends_with("/releases/tag/v0.2.0"));
    }

    #[test]
    fn the_same_or_an_older_release_is_not_an_update() {
        let url = format!("{DOWNLOADS}v0.2.0/Caffettino-0.2.0-universal.zip");
        for current in [Version(0, 2, 0), Version(0, 3, 0)] {
            assert_eq!(
                parse_latest(&latest("v0.2.0", &url), current).unwrap(),
                None
            );
        }
    }

    #[test]
    fn archives_from_elsewhere_can_only_be_downloaded_by_hand() {
        let release = parse_latest(
            &latest(
                "v0.2.0",
                "https://evil.example/Caffettino-0.2.0-universal.zip",
            ),
            Version(0, 1, 0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(release.asset, None);
        assert!(!can_install(&release));
    }

    #[test]
    fn malformed_responses_are_errors() {
        assert!(parse_latest("{}", Version(0, 1, 0)).is_err());
        assert!(parse_latest(&latest("nightly", ""), Version(0, 1, 0)).is_err());
    }

    #[test]
    fn team_ids_come_from_developer_id_signatures_only() {
        let signed = "Executable=/A/Caffettino\nAuthority=Developer ID Application: M (ABCDE12345)\nTeamIdentifier=ABCDE12345\n";
        assert_eq!(parse_team_id(signed).as_deref(), Some("ABCDE12345"));
        assert_eq!(
            parse_team_id("Signature=adhoc\nTeamIdentifier=not set\n"),
            None
        );
        assert_eq!(parse_team_id(""), None);
    }

    #[test]
    fn relaunch_waits_for_exit_and_keeps_paths_out_of_shell_text() {
        let command = restart_command(Path::new("/Apps/Caffé ' $`x.app"), 42);
        let args: Vec<_> = command.get_args().map(std::ffi::OsStr::to_owned).collect();
        assert_eq!(command.get_program(), "/bin/sh");
        assert_eq!(args[3], "42");
        assert_eq!(args[4], "/Apps/Caffé ' $`x.app");
        assert!(!args[1].to_string_lossy().contains("Caff"));
    }
}
