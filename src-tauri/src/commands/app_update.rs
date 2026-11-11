//! Desktop (Tauri) self-update commands.
//!
//! The download is driven from Rust through `tauri-plugin-updater` — not from
//! a renderer callback — so its progress lives in the shared
//! [`crate::update::state`] handle and survives the settings page unmounting,
//! a tab switch, or a reload. The renderer is a pure subscriber: it kicks off
//! `perform_app_update`, then reflects whatever the `app_update_state`
//! event/snapshot reports, exactly like the standalone-server path.
//!
//! These mirror the server-mode axum handlers in
//! `web::handlers::app_update` (same command names, so the transport layer
//! routes `perform_app_update` / `restart_app` / `app_update_state` to the
//! right place per runtime), but drive the platform updater instead of the
//! in-place tarball swap.
//!
//! Every platform downloads first and switches over only when the user asks
//! for the restart. macOS and Linux put the new version in place at the end of
//! the download. Windows keeps the downloaded installer until the restart and
//! runs it then, because running it is what ends the app.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tauri_plugin_updater::UpdaterExt;

use crate::app_error::AppCommandError;
use crate::update::desktop_auth;

const DEVICE_SECRET: &str = "onehall-device-update-token-v1";

fn require_local_window(window: &tauri::WebviewWindow) -> Result<(), AppCommandError> {
    let url = window
        .url()
        .map_err(|_| AppCommandError::permission_denied("Unable to verify desktop window"))?;
    desktop_auth::validate_window(window.label(), &url).map_err(AppCommandError::permission_denied)
}

fn desktop_policy(app: &tauri::AppHandle) -> Result<url::Url, AppCommandError> {
    let denied =
        || AppCommandError::permission_denied("OneHall signed desktop updates are not configured");
    let policy: serde_json::Value =
        serde_json::from_str(include_str!("../../../onehall.release.json"))
            .map_err(|_| denied())?;
    let own: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.hall.conf.json")).map_err(|_| denied())?;
    let base: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.conf.json")).map_err(|_| denied())?;
    let runtime = app.config().plugins.0.get("updater").ok_or_else(denied)?;
    desktop_auth::validate_policy(
        &policy,
        &own["plugins"]["updater"],
        &base["plugins"]["updater"],
        runtime,
    )
    .map_err(AppCommandError::permission_denied)?;
    url::Url::parse(runtime["endpoints"][0].as_str().ok_or_else(denied)?).map_err(|_| denied())
}

#[derive(serde::Serialize)]
pub struct CredentialStatus {
    configured: bool,
}

async fn stored_credential() -> Result<Option<String>, AppCommandError> {
    tauri::async_runtime::spawn_blocking(|| crate::keyring_store::get_secret(DEVICE_SECRET))
        .await
        .map_err(|_| AppCommandError::task_execution_failed("Unable to access update credential"))?
        .map_err(|_| AppCommandError::task_execution_failed("Unable to access update credential"))
}

#[tauri::command]
pub async fn app_update_credential_status(
    window: tauri::WebviewWindow,
) -> Result<CredentialStatus, AppCommandError> {
    require_local_window(&window)?;
    let token = stored_credential().await?;
    if let Some(ref token) = token {
        desktop_auth::validate_token(token).map_err(AppCommandError::invalid_input)?;
    }
    Ok(CredentialStatus {
        configured: token.is_some(),
    })
}

#[tauri::command]
pub async fn import_app_update_credential(
    window: tauri::WebviewWindow,
    path: String,
) -> Result<CredentialStatus, AppCommandError> {
    require_local_window(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let token = desktop_auth::read_credential_file(std::path::Path::new(&path))
            .map_err(AppCommandError::invalid_input)?;
        crate::keyring_store::set_secret(DEVICE_SECRET, &token).map_err(|_| {
            AppCommandError::task_execution_failed("Unable to store update credential")
        })
    })
    .await
    .map_err(|_| AppCommandError::task_execution_failed("Unable to import update credential"))??;
    Ok(CredentialStatus { configured: true })
}

#[tauri::command]
pub async fn remove_app_update_credential(
    window: tauri::WebviewWindow,
) -> Result<CredentialStatus, AppCommandError> {
    require_local_window(&window)?;
    tauri::async_runtime::spawn_blocking(|| crate::keyring_store::delete_secret(DEVICE_SECRET))
        .await
        .map_err(|_| AppCommandError::task_execution_failed("Unable to remove update credential"))?
        .map_err(|_| {
            AppCommandError::task_execution_failed("Unable to remove update credential")
        })?;
    Ok(CredentialStatus { configured: false })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateCheck {
    current_version: String,
    update: Option<DesktopUpdateInfo>,
}

#[derive(serde::Serialize)]
pub struct DesktopUpdateInfo {
    version: String,
    body: String,
    date: Option<String>,
}

#[tauri::command]
pub async fn check_app_update(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<DesktopUpdateCheck, AppCommandError> {
    require_local_window(&window)?;
    let update = authenticated_check(&app, Arc::new(AtomicBool::new(false)))
        .await
        .map_err(AppCommandError::task_execution_failed)?;
    Ok(DesktopUpdateCheck {
        current_version: app.package_info().version.to_string(),
        update: update.map(|update| DesktopUpdateInfo {
            version: update.version,
            body: update.body.unwrap_or_default(),
            date: update.raw_json["pub_date"].as_str().map(str::to_owned),
        }),
    })
}

use crate::update::state::{self as update_state, AppUpdateState, AppUpdateStateHandle};
use crate::web::event_bridge::EventEmitter;

/// Current update snapshot, for the renderer to re-sync on mount.
#[tauri::command]
pub fn app_update_state(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppUpdateStateHandle>,
) -> Result<AppUpdateState, AppCommandError> {
    require_local_window(&window)?;
    Ok(update_state::snapshot(state.inner()))
}

/// Begin (or attach to) a download+install of the available update. Returns
/// immediately with the current snapshot; the work runs detached and reports
/// progress via the `app_update_state` event.
#[tauri::command]
pub async fn perform_app_update(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppUpdateStateHandle>,
) -> Result<AppUpdateState, AppCommandError> {
    require_local_window(&window)?;
    desktop_policy(&app)?;
    let handle = state.inner().clone();
    let emitter = EventEmitter::Tauri(app.clone());

    let (started, snap) = update_state::try_begin(&handle, &emitter);
    if !started {
        // A download is already in flight (or staged) — attach to it.
        return Ok(snap);
    }

    tauri::async_runtime::spawn(async move {
        if let Err(message) = run_download(app, handle.clone(), emitter.clone()).await {
            update_state::set_error(&handle, &emitter, message);
        }
    });

    Ok(snap)
}

/// Bound on the pre-download manifest re-check. `tauri-plugin-updater` applies
/// no timeout of its own, and `try_begin` has already flipped the shared state
/// to `Downloading` by the time we get here — so a black-holed network would
/// pin the UI at "downloading, 0 bytes" forever, with `try_begin` refusing
/// every retry until the app is restarted. Only the check is bounded; the
/// download itself must stay unbounded so a slow-but-progressing transfer is
/// never killed mid-stream.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Whether the download stops short of installing and leaves that to the
/// restart, which is the case on Windows. There the install is the restart:
/// the plugin starts the installer and ends the app (see [`updater`]). Run at
/// the end of the download, it closed the app the moment the download
/// finished, without waiting for the user to restart.
///
/// `cfg!` rather than `#[cfg]`, so that both paths compile on every platform.
const INSTALL_AT_RESTART: bool = cfg!(windows);

/// A downloaded update that the restart installs ([`INSTALL_AT_RESTART`]).
struct StagedInstall {
    update: tauri_plugin_updater::Update,
    /// The package, already checked against the update's signature.
    bytes: Vec<u8>,
    /// Set once the app has shut down for the installer; see [`updater`].
    quit: Arc<AtomicBool>,
}

/// The update a download leaves for the restart. `update_state` keeps the
/// two from overlapping: a download stores it before it reports
/// `ReadyToRestart`, and only a restart claimed from `ReadyToRestart` takes it.
static STAGED_INSTALL: Mutex<Option<StagedInstall>> = Mutex::new(None);

fn staged_install() -> MutexGuard<'static, Option<StagedInstall>> {
    STAGED_INSTALL.lock().unwrap_or_else(|p| p.into_inner())
}

/// `app.updater()`, except that the app quits properly before a Windows
/// installer takes over.
///
/// On Windows a successful install never returns: the plugin launches the
/// installer and ends the process with `std::process::exit(0)` right away, so
/// no `RunEvent::Exit` arrives and the quit in `shut_down` never runs. The
/// agents would outlive the app, and with them the codeg-mcp companions they
/// run from the folder the installer is about to write, along with terminals
/// and office watches; the open workspace windows would not be remembered for
/// the relaunch either. The plugin calls this hook just before it launches the
/// installer, in place of its default one, which only did
/// `cleanup_before_exit`. `quit` records that it ran.
async fn updater(
    app: &tauri::AppHandle,
    quit: Arc<AtomicBool>,
) -> Result<(tauri_plugin_updater::Updater, url::Url), String> {
    let origin = desktop_policy(app)
        .map_err(|_| "OneHall signed desktop updates are not configured".to_string())?;
    let token = stored_credential()
        .await
        .map_err(|_| "Unable to access update credential".to_string())?;
    let authorization =
        desktop_auth::authorization_header(token.as_deref()).map_err(str::to_owned)?;
    let mut headers = http::HeaderMap::new();
    if let Some(authorization) = authorization {
        headers.insert(http::header::AUTHORIZATION, authorization);
    }
    let quitting = app.clone();
    let updater = app
        .updater_builder()
        .headers(headers)
        .configure_client(|client| {
            client
                .redirect(reqwest_updater::redirect::Policy::none())
                .danger_accept_invalid_certs(false)
                .danger_accept_invalid_hostnames(false)
        })
        .on_before_exit(move || {
            quit.store(true, Ordering::SeqCst);
            // `install` runs on a blocking thread (see `install_and_quit`),
            // where this is a plain call. On a runtime worker it would not
            // be, and `shut_down`'s `block_on`s would panic.
            tokio::task::block_in_place(|| crate::shut_down(&quitting));
            quitting.cleanup_before_exit();
        })
        .build()
        .map_err(|_| "Unable to initialize authenticated updater".to_string())?;
    Ok((updater, origin))
}

async fn authenticated_check(
    app: &tauri::AppHandle,
    quit: Arc<AtomicBool>,
) -> Result<Option<tauri_plugin_updater::Update>, String> {
    let (updater, origin) = updater(app, quit).await?;
    let update = tokio::time::timeout(CHECK_TIMEOUT, updater.check())
        .await
        .map_err(|_| "Update check timed out".to_string())?
        .map_err(|_| {
            "Authenticated update check failed; verify device access and service availability"
                .to_string()
        })?;
    if let Some(ref update) = update {
        desktop_auth::validate_url(&update.download_url, &origin).map_err(str::to_owned)?;
    }
    Ok(update)
}

/// Re-check and download the update, driving progress into the shared handle,
/// and install it unless that waits for the restart ([`INSTALL_AT_RESTART`]).
/// On success leaves the state at `ReadyToRestart`. Returns the error message
/// (already stringified) so the caller can record it.
async fn run_download(
    app: tauri::AppHandle,
    handle: AppUpdateStateHandle,
    emitter: EventEmitter,
) -> Result<(), String> {
    let quit = Arc::new(AtomicBool::new(false));
    let update = authenticated_check(&app, quit.clone())
        .await?
        .ok_or_else(|| "No update available".to_string())?;
    let version = update.version.clone();

    let pe = Arc::new(update_state::ProgressEmitter::new(
        handle.clone(),
        emitter.clone(),
    ));
    let pe_chunk = pe.clone();
    let mut downloaded: u64 = 0;
    let on_chunk = move |chunk_len: usize, content_len: Option<u64>| {
        downloaded += chunk_len as u64;
        pe_chunk.downloading(downloaded, content_len);
    };
    if INSTALL_AT_RESTART {
        // Nothing gets installed until the restart, so there is no
        // `Installing` phase: the download runs to its end, then the restart
        // button takes over.
        let bytes = update.download(on_chunk, || {}).await.map_err(|_| {
            "Authenticated update download or signature verification failed".to_string()
        })?;
        *staged_install() = Some(StagedInstall {
            update,
            bytes,
            quit,
        });
    } else {
        let pe_finish = pe.clone();
        update
            .download_and_install(on_chunk, move || pe_finish.installing())
            .await
            .map_err(|_| {
                "Authenticated update download or signature verification failed".to_string()
            })?;
    }

    // The desktop app relaunches itself, so there is no supervisor trial or
    // restart-delay metadata to carry (those are server-only).
    update_state::set_ready(&handle, &emitter, Some(version), None, None, None);
    Ok(())
}

/// Relaunch into the freshly-installed bytes. Flips the shared snapshot to
/// `Restarting` first so the UI (and any other window) reflects it, then
/// restarts after a short flush delay. `restart()` never returns.
///
/// On Windows the install happens here instead ([`INSTALL_AT_RESTART`]); see
/// [`install_and_quit`].
#[tauri::command]
pub async fn restart_app(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppUpdateStateHandle>,
) -> Result<(), AppCommandError> {
    require_local_window(&window)?;
    desktop_policy(&app)?;
    let handle = state.inner().clone();
    let emitter = EventEmitter::Tauri(app.clone());
    // Atomically claim the relaunch (flips to `Restarting`) only if an update is
    // genuinely staged — same authority check as the server `restart_impl`.
    // Guards a stale window / direct IPC call from relaunching during
    // idle/error/downloading/installing, and serializes concurrent clicks.
    if !update_state::try_claim_restart(&handle, &emitter) {
        return Err(AppCommandError::invalid_input(
            "No staged update to restart into",
        ));
    }
    if INSTALL_AT_RESTART {
        return install_and_quit(app, handle, emitter).await;
    }
    // Off the main thread on purpose: from here `restart()` goes through
    // `RunEvent::ExitRequested` and `RunEvent::Exit`. At `Exit` the
    // single-instance plugin removes its socket, and on macOS
    // `crate::relaunch` takes the restart over. On the main thread tauri
    // relaunches straight away, without either event, and on macOS (tauri
    // 2.12.2+ relaunches through LaunchServices and waits for the launch) the
    // new instance finds the old one still listening, hands it its arguments
    // and quits — nothing is left running.
    //
    // On macOS the new instance must also not be a plain child of this one.
    // A codeg started at login runs as the job of
    // `~/Library/LaunchAgents/codeg.plist`, and when the job's process exits,
    // launchd sends SIGTERM to everything left in its process group. tauri
    // 2.11 spawned the new instance as such a child, and it died there,
    // still in `exec` while Gatekeeper checked the bundle the update had just
    // put in place: the kernel logged "ASP: Sleep interrupted" and "Security
    // policy would not allow process", nothing came back, and the macOS 27
    // Dock showed codeg as running in the background. `crate::relaunch`
    // launches it through LaunchServices (`open -n`), which makes it a job of
    // its own, from a relauncher in a process group of its own, and only once
    // this process is gone — earlier, the Dock gives it a second icon.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        tracing::info!("[update] relaunching into the installed update");
        app.restart();
    });
    Ok(())
}

/// The restart on Windows: runs the installer the download left, which ends
/// the app and, once it has installed the new version, starts it (the plugin
/// passes it `/R`). Returns only if that failed, with the state at `Error`.
async fn install_and_quit(
    app: tauri::AppHandle,
    handle: AppUpdateStateHandle,
    emitter: EventEmitter,
) -> Result<(), AppCommandError> {
    let Some(StagedInstall {
        update,
        bytes,
        quit,
    }) = staged_install().take()
    else {
        let message = "No downloaded update to install";
        update_state::set_error(&handle, &emitter, message);
        return Err(AppCommandError::invalid_input(message));
    };
    // Lets the windows show `Restarting` before the app shuts down.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    tracing::info!("[update] starting the installer");
    // On a blocking thread: it writes the installer out and shuts the app
    // down, and if the installer starts it ends the process right there.
    let outcome = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    let message = match outcome {
        Ok(Ok(())) => "The installer did not take over".to_string(),
        Ok(Err(_)) => "Update installation failed".to_string(),
        Err(_) => "Update installation task failed".to_string(),
    };
    if quit.load(Ordering::SeqCst) {
        // The installer failed to start after the app had already shut down
        // for it: its agents, terminals and web service are gone. Come back up
        // as the version still installed rather than carry on half torn down.
        tracing::error!("[update] the installer did not start: {message}; relaunching");
        app.restart();
    }
    tracing::error!("[update] the update could not be installed: {message}");
    update_state::set_error(&handle, &emitter, message.clone());
    Err(AppCommandError::task_execution_failed(message))
}
