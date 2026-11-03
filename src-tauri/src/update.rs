//! Updates of the app itself, through `tauri-plugin-updater`.
//!
//! The release workflow attaches `latest.json` (version, notes, and for each
//! platform the package URL and its minisign signature) to every GitHub
//! release; the app reads the one of the latest published release (drafts are
//! not visible), so publishing a release by hand is what ships it. Packages
//! are verified against the public key in `tauri.conf.json` before anything
//! is installed: macOS replaces the `.app` bundle, Linux rewrites the AppImage
//! or installs the `.deb`/`.rpm` through pkexec (the system asks for the
//! administrator password). The new version runs after a restart, which the
//! UI asks for (it may interrupt transfers).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::error::{AppError, Result};
use crate::log::{Channel, Logger};

/// Where the app looks for updates (also in `tauri.conf.json`, which the
/// plugin reads): logged with each check.
pub const ENDPOINT: &str = "https://github.com/pegperego/glassferry/releases/latest/download/latest.json";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    /// Release notes (the version's section of CHANGELOG.md, Markdown).
    pub notes: Option<String>,
    /// Publication date, RFC 3339.
    pub date: Option<String>,
    /// How it would be installed: `app`, `appimage`, `deb`, `rpm`; `None`
    /// when this build cannot install it (development build, unknown
    /// package): the UI links the release page instead.
    pub installer: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// The update found by the last check.
#[derive(Default)]
pub struct UpdateState {
    pending: Mutex<Option<Update>>,
    installing: AtomicBool,
}

/// The package kind this running app was installed from, if it can update
/// itself. Development builds never install: the "bundle" would be the
/// cargo target directory.
fn installer() -> Option<&'static str> {
    use tauri::utils::config::BundleType;
    if cfg!(debug_assertions) {
        return None;
    }
    match tauri::utils::platform::bundle_type()? {
        BundleType::App => Some("app"),
        BundleType::AppImage => Some("appimage"),
        BundleType::Deb => Some("deb"),
        BundleType::Rpm => Some("rpm"),
        _ => None,
    }
}

fn info(update: &Update) -> UpdateInfo {
    UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone().filter(|b| !b.trim().is_empty()),
        date: update
            .date
            .and_then(|d| chrono::DateTime::from_timestamp(d.unix_timestamp(), 0))
            .map(|d| d.to_rfc3339()),
        installer: installer(),
    }
}

impl UpdateState {
    /// Asks the endpoint for a version newer than this one.
    pub async fn check(&self, app: &AppHandle, logger: &std::sync::Arc<Logger>) -> Result<Option<UpdateInfo>> {
        let mut op = logger.start(None, Channel::Local, "update-check", ENDPOINT);
        op.equivalent(format!("curl -L {ENDPOINT}"));
        let found = async {
            let updater = app.updater_builder().build().map_err(|e| AppError::Other(e.to_string()))?;
            updater.check().await.map_err(|e| AppError::Other(format!("controllo degli aggiornamenti: {e}")))
        }
        .await;
        let found = op.result(found, |update| {
            Some(match update {
                Some(update) => format!("disponibile la {} (installata la {})", update.version, update.current_version),
                None => format!("nessun aggiornamento (installata la {})", app.package_info().version),
            })
        })?;
        let result = found.as_ref().map(info);
        *self.pending.lock().unwrap() = found;
        Ok(result)
    }

    /// Downloads, verifies and installs the update found by the last check;
    /// returns its version. Progress goes to the UI as `update-progress`.
    pub async fn install(&self, app: &AppHandle, logger: &std::sync::Arc<Logger>) -> Result<String> {
        let Some(kind) = installer() else {
            return Err(AppError::Other("questa build non si aggiorna da sola: scarica la nuova versione dalla pagina della release".into()));
        };
        let update = self
            .pending
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AppError::Other("nessun aggiornamento da installare: controlla di nuovo".into()))?;
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(AppError::Other("installazione già in corso".into()));
        }
        let result = self.download_and_install(app, logger, &update, kind).await;
        self.installing.store(false, Ordering::SeqCst);
        result?;
        Ok(update.version)
    }

    async fn download_and_install(&self, app: &AppHandle, logger: &std::sync::Arc<Logger>, update: &Update, kind: &str) -> Result<()> {
        let mut op = logger.start(None, Channel::Local, "update-download", update.download_url.as_str());
        op.equivalent(format!("curl -LO {}", update.download_url));
        let mut downloaded: u64 = 0;
        let mut last = Instant::now() - PROGRESS_INTERVAL;
        let bytes = update
            .download(
                |chunk, total| {
                    downloaded += chunk as u64;
                    if last.elapsed() >= PROGRESS_INTERVAL || total == Some(downloaded) {
                        last = Instant::now();
                        let _ = app.emit("update-progress", UpdateProgress { downloaded, total });
                    }
                },
                || {},
            )
            .await
            // `download` checks the signature too.
            .map_err(|e| AppError::Other(format!("download dell'aggiornamento: {e}")));
        let bytes = op.result(bytes, |b| Some(format!("{} byte, firma verificata", b.len())))?;

        let op = logger.start(None, Channel::Local, "update-install", format!("{} {}", update.version, kind));
        let update = update.clone();
        // Blocking: pkexec waits for the administrator password on Linux.
        let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
            .and_then(|r| r.map_err(|e| AppError::Other(format!("installazione dell'aggiornamento: {e}"))));
        op.result(installed, |_| Some("si attiva al riavvio".into()))
    }
}
