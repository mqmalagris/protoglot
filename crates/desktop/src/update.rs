//! Self-update from GitHub Releases (the Postman-style "update available" flow).
//! `self_update` is blocking, so the UI runs these on a background thread and
//! shows the result. The release archive nests the binary under
//! `protoglot-desktop-<version>-<target>/`, matched via the template below.

use self_update::cargo_crate_version;

const OWNER: &str = "mqmalagris";
const REPO: &str = "protoglot";
const BIN: &str = "protoglot-desktop";
const ARCHIVE_BIN_PATH: &str = "protoglot-desktop-{{ version }}-{{ target }}/{{ bin }}";

#[derive(Debug, Clone)]
pub enum UpdateOutcome {
    UpToDate,
    Available(String), // newer version tag
    Updated(String),   // updated to this version
    Failed(String),
}

/// Check GitHub for a newer release (no download).
pub fn check() -> UpdateOutcome {
    match latest_newer() {
        Ok(Some(v)) => UpdateOutcome::Available(v),
        Ok(None) => UpdateOutcome::UpToDate,
        Err(e) => UpdateOutcome::Failed(e),
    }
}

/// Download + replace this binary with the latest release.
pub fn install() -> UpdateOutcome {
    match do_update() {
        Ok(status) if status.updated() => UpdateOutcome::Updated(status.version().to_string()),
        Ok(_) => UpdateOutcome::UpToDate,
        Err(e) => UpdateOutcome::Failed(e),
    }
}

fn latest_newer() -> Result<Option<String>, String> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(OWNER)
        .repo_name(REPO)
        .build()
        .map_err(|e| e.to_string())?
        .fetch()
        .map_err(|e| e.to_string())?;
    let latest = releases.first().ok_or("no releases found")?;
    let newer = self_update::version::bump_is_greater(cargo_crate_version!(), &latest.version)
        .map_err(|e| e.to_string())?;
    Ok(newer.then(|| latest.version.clone()))
}

fn do_update() -> Result<self_update::Status, String> {
    self_update::backends::github::Update::configure()
        .repo_owner(OWNER)
        .repo_name(REPO)
        .bin_name(BIN)
        .bin_path_in_archive(ARCHIVE_BIN_PATH)
        .current_version(cargo_crate_version!())
        .show_download_progress(false)
        .no_confirm(true) // no stdin prompt — this is a GUI
        .build()
        .map_err(|e| e.to_string())?
        .update()
        .map_err(|e| e.to_string())
}
