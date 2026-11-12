//! Checks GitHub for new releases and installs them in place.
//!
//! This is the only network access RTerm makes. At launch and then once a
//! day, it asks GitHub's public API for the latest release of the repository
//! named in `Cargo.toml`, using the system's `curl`: nothing about you or
//! your machine is sent. The check can be turned off with `check = false` in
//! the `[updates]` section of the configuration.
//!
//! Installing downloads the release's `.zip`, unpacks it next to the running
//! `RTerm.app`, checks that the new binary reports the expected version and
//! swaps the two bundles. The new version starts on the next launch, or
//! right away with "Restart to Update".
//!
//! Only a copy running from an `.app` bundle can update itself: `cargo run`
//! builds never check.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use gpui::{App, AppContext as _, Context, Entity, Global, actions};

actions!(rterm, [CheckForUpdates, InstallUpdate, RestartToUpdate]);

/// Time between two automatic checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Delay before the first check, so that it does not slow down the launch.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// Download address of the macOS `.zip`.
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing to show: not checked yet, or up to date after an automatic check.
    Idle,
    Checking,
    /// Up to date after a check the user asked for.
    UpToDate,
    Available(Release),
    Installing(String),
    /// Installed: takes effect at the next launch.
    Ready(String),
    Failed(String),
}

pub struct Updater {
    pub status: Status,
    /// The running `RTerm.app`, when RTerm runs from a bundle.
    bundle: Option<PathBuf>,
    feed: String,
}

struct GlobalUpdater(Entity<Updater>);
impl Global for GlobalUpdater {}

/// Creates the updater and, if `automatic`, starts the periodic check.
pub fn init(automatic: bool, cx: &mut App) {
    let updater = cx.new(|_| Updater {
        status: Status::Idle,
        bundle: running_bundle(),
        // `RTERM_UPDATE_FEED`: another release feed, used to test updates.
        feed: std::env::var("RTERM_UPDATE_FEED").unwrap_or_else(|_| default_feed()),
    });
    cx.set_global(GlobalUpdater(updater.clone()));

    cx.on_action({
        let updater = updater.clone();
        move |_: &CheckForUpdates, cx| updater.update(cx, |updater, cx| updater.check(true, cx))
    });
    cx.on_action({
        let updater = updater.clone();
        move |_: &InstallUpdate, cx| updater.update(cx, |updater, cx| updater.install(cx))
    });
    cx.on_action({
        let updater = updater.clone();
        move |_: &RestartToUpdate, cx| updater.update(cx, |updater, cx| updater.restart(cx))
    });

    if automatic && updater.read(cx).can_update() {
        updater.update(cx, |_, cx| {
            cx.spawn(async move |this, cx| {
                let mut delay = FIRST_CHECK_DELAY;
                loop {
                    cx.background_executor().timer(delay).await;
                    delay = CHECK_INTERVAL;
                    let checked = this.update(cx, |updater, cx| {
                        // Never interrupt an installation or hide a pending update.
                        if matches!(updater.status, Status::Idle | Status::UpToDate | Status::Failed(_)) {
                            updater.check(false, cx);
                        }
                    });
                    if checked.is_err() {
                        break;
                    }
                }
            })
            .detach();
        });
    }
}

pub fn global(cx: &App) -> Entity<Updater> {
    cx.global::<GlobalUpdater>().0.clone()
}

impl Updater {
    /// Whether this copy can replace itself: it must run from an `.app`.
    pub fn can_update(&self) -> bool {
        self.bundle.is_some()
    }

    /// A short text for the window, or `None` when there is nothing to say.
    pub fn badge(&self) -> Option<String> {
        match &self.status {
            Status::Idle => None,
            Status::Checking => Some("Checking for updates…".into()),
            Status::UpToDate => Some("RTerm is up to date".into()),
            Status::Available(release) => Some(format!("Update to {}", release.version)),
            Status::Installing(version) => Some(format!("Installing {version}…")),
            Status::Ready(_) => Some("Restart to Update".into()),
            Status::Failed(_) => Some("Update failed".into()),
        }
    }

    /// One line for the About window.
    pub fn summary(&self) -> String {
        match &self.status {
            _ if !self.can_update() => crate::platform::pick(
                "Only available in RTerm.app",
                "Download new versions from GitHub",
            )
            .into(),
            Status::Idle => "Not checked yet".into(),
            Status::Checking => "Checking…".into(),
            Status::UpToDate => "Up to date".into(),
            Status::Available(release) => format!("{} available", release.version),
            Status::Installing(version) => format!("Installing {version}…"),
            Status::Ready(version) => format!("{version} installed, restart to use it"),
            Status::Failed(error) => format!("Failed: {error}"),
        }
    }

    /// What clicking the badge does, depending on the status.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        match &self.status {
            Status::Available(_) => self.install(cx),
            Status::Ready(_) => self.restart(cx),
            Status::UpToDate | Status::Failed(_) => {
                self.status = Status::Idle;
                cx.notify();
            }
            Status::Idle | Status::Checking | Status::Installing(_) => {}
        }
    }

    /// Asks GitHub for the latest release. `manual`: the user asked, so
    /// "up to date" is worth saying.
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if !self.can_update() {
            if manual {
                self.status = Status::Failed("updates only work in the RTerm.app bundle".into());
                cx.notify();
            }
            return;
        }
        if matches!(self.status, Status::Checking | Status::Installing(_) | Status::Ready(_)) {
            return;
        }
        self.status = Status::Checking;
        cx.notify();

        let feed = self.feed.clone();
        let task = cx.background_executor().spawn(async move { latest_release(&feed) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |updater, cx| {
                updater.status = match result {
                    Ok(release) if is_newer(&release.version, crate::VERSION) => Status::Available(release),
                    Ok(_) if manual => Status::UpToDate,
                    Ok(_) => Status::Idle,
                    Err(error) if manual => Status::Failed(format!("{error:#}")),
                    // A failed automatic check (offline…) is not worth a message.
                    Err(_) => Status::Idle,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Downloads and installs the available release.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let (Status::Available(release), Some(bundle)) = (&self.status, &self.bundle) else {
            return;
        };
        let (release, bundle) = (release.clone(), bundle.clone());
        self.status = Status::Installing(release.version.clone());
        cx.notify();

        let task = cx.background_executor().spawn({
            let release = release.clone();
            async move { install(&release, &bundle) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |updater, cx| {
                updater.status = match result {
                    Ok(()) => Status::Ready(release.version),
                    Err(error) => Status::Failed(format!("{error:#}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Quits and launches the installed version as soon as this one is gone.
    pub fn restart(&self, cx: &mut App) {
        let (Status::Ready(_), Some(bundle)) = (&self.status, &self.bundle) else {
            return;
        };
        let relaunch = Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; open "$2""#)
            .arg("sh")
            .arg(std::process::id().to_string())
            .arg(bundle)
            .spawn();
        if relaunch.is_ok() {
            cx.quit();
        }
    }
}

/// `https://github.com/owner/name` from `Cargo.toml` → the API address of
/// its latest release.
fn default_feed() -> String {
    let repository = env!("CARGO_PKG_REPOSITORY").trim_end_matches('/');
    let slug = repository.trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{slug}/releases/latest")
}

/// The `.app` that contains the running binary, if any.
fn running_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bundle = executable.ancestors().nth(3)?;
    let is_bundle = bundle.extension().is_some_and(|extension| extension == "app")
        && executable.parent()?.ends_with("Contents/MacOS");
    is_bundle.then(|| bundle.to_path_buf())
}

fn curl() -> Command {
    let mut command = Command::new("/usr/bin/curl");
    command.args(["--fail", "--silent", "--show-error", "--location", "--max-time", "300"]);
    command
}

fn run(command: &mut Command, what: &str) -> Result<Vec<u8>> {
    let output = command.output().with_context(|| format!("{what}: could not run the command"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        bail!("{what}: {}", message.trim());
    }
    Ok(output.stdout)
}

fn latest_release(feed: &str) -> Result<Release> {
    let body = run(
        curl().args(["--max-time", "20", "-H", "Accept: application/vnd.github+json", feed]),
        "Could not reach GitHub",
    )?;
    parse_release(&body)
}

/// Reads GitHub's answer: the tag and the macOS `.zip` among the assets.
fn parse_release(body: &[u8]) -> Result<Release> {
    let json: serde_json::Value = serde_json::from_slice(body).context("Unexpected answer from GitHub")?;
    let tag = json["tag_name"].as_str().context("The release has no tag")?;
    let version = tag.trim_start_matches('v').to_owned();
    let url = json["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|asset| asset["name"].as_str().is_some_and(|name| name.ends_with("-macos-universal.zip")))
        .find_map(|asset| asset["browser_download_url"].as_str())
        .with_context(|| format!("Release {version} has no macOS download"))?
        .to_owned();
    Ok(Release { version, url })
}

/// `1.2.3` → (1, 2, 3). A suffix such as `-dev` is ignored.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// Downloads `release` and puts it in place of `bundle`.
fn install(release: &Release, bundle: &Path) -> Result<()> {
    let parent = bundle.parent().context("RTerm.app has no parent folder")?;
    let name = bundle.file_name().context("RTerm.app has no name")?.to_string_lossy().into_owned();
    // Everything happens next to the bundle, on the same disk, so that the
    // final swap is two renames.
    let staging = parent.join(format!(".{name}.update"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).with_context(|| {
        format!("Cannot write to {}: move RTerm.app to a folder you own", parent.display())
    })?;
    let result = (|| {
        let archive = staging.join("RTerm.zip");
        run(curl().arg("--output").arg(&archive).arg(&release.url), "Download failed")?;
        // ditto keeps the signature and attributes intact.
        run(
            Command::new("/usr/bin/ditto").arg("-x").arg("-k").arg(&archive).arg(&staging),
            "Could not unpack the update",
        )?;
        let new_bundle = staging.join("RTerm.app");
        let reported = run(
            Command::new(new_bundle.join("Contents/MacOS/rterm")).arg("--version"),
            "The downloaded RTerm does not start",
        )?;
        let reported = String::from_utf8_lossy(&reported).trim().to_owned();
        ensure!(
            reported == format!("RTerm {}", release.version),
            "The download reports “{reported}” instead of RTerm {}",
            release.version
        );

        let old = staging.join("previous.app");
        std::fs::rename(bundle, &old).context("Could not move the current version aside")?;
        if let Err(error) = std::fs::rename(&new_bundle, bundle) {
            // Put the running version back.
            let _ = std::fs::rename(&old, bundle);
            return Err(error).context("Could not put the new version in place");
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.9.99"));
        assert!(is_newer("0.1.1", "0.1.0-dev"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn release_answer_is_parsed() {
        let body = br#"{
            "tag_name": "v0.2.3",
            "assets": [
                {"name": "RTerm-0.2.3-macos-universal.dmg", "browser_download_url": "https://x/a.dmg"},
                {"name": "RTerm-0.2.3-macos-universal.zip", "browser_download_url": "https://x/a.zip"}
            ]
        }"#;
        let release = parse_release(body).unwrap();
        assert_eq!(release, Release { version: "0.2.3".into(), url: "https://x/a.zip".into() });

        let no_zip = br#"{"tag_name": "v0.2.3", "assets": []}"#;
        assert!(parse_release(no_zip).is_err());
    }

    /// Installs a fake release over a fake bundle, as the real update does.
    /// Updates only exist for the macOS app (it needs `ditto`).
    #[cfg(target_os = "macos")]
    #[test]
    fn install_swaps_the_bundle() {
        let root = std::env::temp_dir().join(format!("rterm-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fake_bundle = |parent: &Path, version: &str| {
            let macos = parent.join("RTerm.app/Contents/MacOS");
            std::fs::create_dir_all(&macos).unwrap();
            let binary = macos.join("rterm");
            std::fs::write(&binary, format!("#!/bin/sh\necho 'RTerm {version}'\n")).unwrap();
            Command::new("chmod").arg("755").arg(&binary).status().unwrap();
            parent.join("RTerm.app")
        };
        let installed = fake_bundle(&root.join("Applications"), "0.1.0");
        let new = fake_bundle(&root.join("build"), "0.2.0");
        let archive = root.join("RTerm-0.2.0-macos-universal.zip");
        let zipped = Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&new)
            .arg(&archive)
            .status()
            .unwrap();
        assert!(zipped.success());

        let version_of = |bundle: &Path| {
            let output = Command::new(bundle.join("Contents/MacOS/rterm")).output().unwrap();
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let release = |version: &str| Release {
            version: version.into(),
            url: format!("file://{}", archive.display()),
        };

        // A download that reports another version is refused, nothing moves.
        assert!(install(&release("0.3.0"), &installed).is_err());
        assert_eq!(version_of(&installed), "RTerm 0.1.0");

        install(&release("0.2.0"), &installed).unwrap();
        assert_eq!(version_of(&installed), "RTerm 0.2.0");
        // No leftovers next to the bundle.
        let entries: Vec<_> = std::fs::read_dir(root.join("Applications")).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_feed_follows_the_repository() {
        assert!(default_feed().starts_with("https://api.github.com/repos/"));
        assert!(default_feed().ends_with("/releases/latest"));
    }
}
