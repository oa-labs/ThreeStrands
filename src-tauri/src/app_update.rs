//! Checks GitHub Releases for a newer ThreeStrands and, where this install can
//! replace itself, installs it on request. The check and install run here
//! rather than through the updater plugin's JavaScript API so the webview
//! never gains permission to download or install arbitrary updates: it can
//! only ask "is there an update?" and "install the one you found".
//!
//! The update manifest URL lives in `tauri.conf.json` (`plugins.updater`).
//! A future beta channel would pass its own manifest URL through
//! `updater_builder().endpoints(..)` instead of changing that default.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

const RELEASES_URL: &str = "https://github.com/oa-labs/ThreeStrands/releases";
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// How this particular install can take the update it was offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallMode {
    /// The updater can replace this install and relaunch it.
    InPlace,
    /// macOS is running the app from a disk image or a translocated copy,
    /// which cannot be replaced. The user must move it to Applications.
    MoveToApplications,
    /// Packages owned by the system package manager (deb, rpm) are updated by
    /// downloading the new package, not by replacing files underneath it.
    Download,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub version: String,
    pub current_version: String,
    pub release_url: String,
    pub install_mode: InstallMode,
}

/// The update the last check found, held so `install_app_update` installs
/// exactly what the user was shown rather than re-resolving the manifest.
#[derive(Default)]
pub struct PendingUpdate(tokio::sync::Mutex<Option<Update>>);

fn install_mode(bundle: Option<BundleType>, executable: &Path) -> InstallMode {
    if cfg!(target_os = "macos") {
        let path = executable.to_string_lossy();
        if path.starts_with("/Volumes/") || path.contains("/AppTranslocation/") {
            InstallMode::MoveToApplications
        } else {
            InstallMode::InPlace
        }
    } else if bundle == Some(BundleType::AppImage) {
        InstallMode::InPlace
    } else {
        InstallMode::Download
    }
}

fn release_url(version: &str) -> String {
    format!("{RELEASES_URL}/tag/v{version}")
}

fn current_install_mode() -> InstallMode {
    let executable = std::env::current_exe().unwrap_or_default();
    install_mode(tauri::utils::platform::bundle_type(), &executable)
}

#[tauri::command]
pub async fn check_for_app_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<Option<AvailableUpdate>, String> {
    // Development builds are not released and would only ever find the
    // published version, so they never contact the update server.
    if cfg!(debug_assertions) {
        return Ok(None);
    }
    let update = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|error| format!("Unable to prepare the update check: {error}"))?
        .check()
        .await
        .map_err(|error| format!("Unable to check for updates: {error}"))?;
    let available = update.as_ref().map(|update| AvailableUpdate {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        release_url: release_url(&update.version),
        install_mode: current_install_mode(),
    });
    *pending.0.lock().await = update;
    Ok(available)
}

#[tauri::command]
pub async fn install_app_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<(), String> {
    if current_install_mode() != InstallMode::InPlace {
        return Err("This copy of ThreeStrands cannot update itself. Download the new version instead.".into());
    }
    let update = pending
        .0
        .lock()
        .await
        .clone()
        .ok_or("No update is ready to install. Check for updates again.")?;
    log::info!(target: "threestrands", "installing ThreeStrands {}", update.version);
    // The plugin verifies the download against the public key in
    // tauri.conf.json before replacing anything.
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| format!("Unable to install the update: {error}"))?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_url_points_at_the_tagged_release() {
        assert_eq!(
            release_url("0.72.0"),
            "https://github.com/oa-labs/ThreeStrands/releases/tag/v0.72.0"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_installs_in_place_unless_running_from_a_disk_image_or_translocation() {
        let mode = |path: &str| install_mode(Some(BundleType::App), Path::new(path));
        assert_eq!(
            mode("/Applications/ThreeStrands.app/Contents/MacOS/threestrands"),
            InstallMode::InPlace
        );
        assert_eq!(
            mode("/Users/someone/Applications/ThreeStrands.app/Contents/MacOS/threestrands"),
            InstallMode::InPlace
        );
        assert_eq!(
            mode("/Volumes/ThreeStrands/ThreeStrands.app/Contents/MacOS/threestrands"),
            InstallMode::MoveToApplications
        );
        assert_eq!(
            mode("/private/var/folders/x/AppTranslocation/ABC/d/ThreeStrands.app/Contents/MacOS/threestrands"),
            InstallMode::MoveToApplications
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_installs_only_appimages_in_place() {
        let path = Path::new("/home/someone/ThreeStrands.AppImage");
        assert_eq!(install_mode(Some(BundleType::AppImage), path), InstallMode::InPlace);
        assert_eq!(install_mode(Some(BundleType::Deb), path), InstallMode::Download);
        assert_eq!(install_mode(Some(BundleType::Rpm), path), InstallMode::Download);
        assert_eq!(install_mode(None, path), InstallMode::Download);
    }
}
