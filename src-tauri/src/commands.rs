use reqwest::blocking::Client;
use std::{path::Path, process::Command, time::Duration, sync::{Arc, Mutex}, collections::HashSet};
use tauri::{AppHandle, State, Manager};

use crate::extractor::{detect_archive_format, ArchiveFormat};
use crate::process::{
    is_process_in_directory, normalize_process_path, query_running_executable_paths,
    sanitize_install_slug, spawn_mod_watcher_thread,
};

use crate::state::{
    emit_installation_event, emit_installation_progress_event, emit_mod_process_status_event,
    extract_credit_names, first_image_url, image_urls, load_state, save_state, state_to_view,
    BaseZipValidation, ClubModEnvelope, ClubModsResponse, InstallResult, InstalledMod,
    LauncherRuntimeState, LauncherStateView, SupportedMod, SupportedModCredits,
    UpdateLauncherConfigRequest, EXPECTED_DDLC_SHA256,
};
use crate::utils::{
    cleanup_failed_installation_target, compute_sha256_chunked, copy_file_secure,
    create_dir_all_safe, debug_log, ensure_file_exists, now_epoch_millis,
    path_exists, sanitize_slug_for_filename, to_absolute_path,
};

#[tauri::command]
pub fn get_launcher_state(app: AppHandle) -> Result<LauncherStateView, String> {
    let state = load_state(&app)?;
    Ok(state_to_view(&state))
}

#[tauri::command]
pub fn update_launcher_config(
    app: AppHandle,
    request: UpdateLauncherConfigRequest,
) -> Result<LauncherStateView, String> {
    let mut state = load_state(&app)?;

    if let Some(raw_manifest_url) = request.manifest_url {
        let trimmed = raw_manifest_url.trim().to_owned();
        if trimmed.is_empty() {
            state.manifest_url = None;
        } else {
            crate::state::validate_manifest_url(&trimmed)?;
            state.manifest_url = Some(trimmed);
        }
    }

    if let Some(raw_install_dir) = request.global_install_dir {
        let trimmed = raw_install_dir.trim().to_owned();
        let resolved = if trimmed.is_empty() {
            crate::state::default_install_dir()
        } else {
            to_absolute_path(Path::new(&trimmed))?
        };

        crate::state::ensure_install_dir_allowed(&resolved)?;
        create_dir_all_safe(&resolved)?;
        state.global_install_dir = Some(resolved.to_string_lossy().into_owned());
    }

    if let Some(raw_zip_path) = request.cached_ddlc_zip_path {
        let trimmed = raw_zip_path.trim().to_owned();
        if trimmed.is_empty() {
            state.cached_ddlc_zip_path = None;
        } else {
            let absolute = to_absolute_path(Path::new(&trimmed))?;
            ensure_file_exists(&absolute, "archivo base de DDLC")?;
            detect_archive_format(&absolute)?;
            state.cached_ddlc_zip_path = Some(absolute.to_string_lossy().into_owned());
        }
    }

    save_state(&app, &state)?;
    Ok(state_to_view(&state))
}

#[tauri::command]
pub async fn validate_vanilla_zip(
    app: AppHandle,
    path: String,
) -> Result<BaseZipValidation, String> {
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || validate_vanilla_zip_impl(&app_handle, &path))
        .await
        .map_err(|err| format!("Error en tarea de validación: {err}"))?
}

fn validate_vanilla_zip_impl(
    _app: &AppHandle,
    raw_path: &str,
) -> Result<BaseZipValidation, String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err("Ruta del archivo base vacía.".to_owned());
    }

    let absolute = to_absolute_path(Path::new(trimmed))?;
    ensure_file_exists(&absolute, "ZIP base original")?;

    let archive_format = detect_archive_format(&absolute)?;
    let (expected_sha256, is_valid, warning) = match archive_format {
        ArchiveFormat::Zip => {
            let hash = compute_sha256_chunked(&absolute)?;
            let valid = hash.eq_ignore_ascii_case(EXPECTED_DDLC_SHA256);
            let warn = if valid {
                None
            } else {
                Some(
                    "Verificación fallida: el hash SHA-256 no coincide con el juego oficial."
                        .to_owned(),
                )
            };
            (hash, valid, warn)
        }
        ArchiveFormat::Rar => (
            "N/A (RAR)".to_owned(),
            true,
            Some(
                "Formato RAR detectado: no existe verificación para validación segura.".to_owned(),
            ),
        ),
    };

    Ok(BaseZipValidation {
        path: absolute.to_string_lossy().into_owned(),
        computed_sha256: expected_sha256,
        expected_sha256: EXPECTED_DDLC_SHA256.to_owned(),
        is_valid,
        warning,
    })
}

#[tauri::command]
pub async fn finalize_oobe_setup(
    app: AppHandle,
    original_zip_path: String,
    global_install_dir: String,
) -> Result<LauncherStateView, String> {
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        finalize_oobe_setup_impl(&app_handle, &original_zip_path, global_install_dir)
    })
    .await
    .map_err(|err| format!("Debug: error en tarea OOBE: {err}"))?
}

fn finalize_oobe_setup_impl(
    app: &AppHandle,
    original_zip_path: &str,
    global_install_dir: String,
) -> Result<LauncherStateView, String> {
    let source_original_zip = to_absolute_path(Path::new(original_zip_path.trim()))?;
    ensure_file_exists(&source_original_zip, "archivo base original")?;

    match detect_archive_format(&source_original_zip)? {
        ArchiveFormat::Zip => {}
        ArchiveFormat::Rar => {
            return Err("Se requiere el archivo original en formato .zip.".to_owned())
        }
    }

    let dest_dir = crate::state::oobe_dir_path(app)?;
    let isolated_zip = dest_dir.join(crate::state::OOBE_ORIGINAL_ARCHIVE_NAME);
    copy_file_secure(&source_original_zip, &isolated_zip)?;
    let copied_hash = compute_sha256_chunked(&isolated_zip)?;
    if !copied_hash.eq_ignore_ascii_case(EXPECTED_DDLC_SHA256) {
        return Err(format!("La copia local del ZIP original quedó corrupta."));
    }

    let mut state = load_state(app)?;
    let target_install_dir = to_absolute_path(Path::new(global_install_dir.trim()))?;
    create_dir_all_safe(&target_install_dir)?;

    state.global_install_dir = Some(target_install_dir.to_string_lossy().into_owned());
    state.cached_ddlc_zip_path = Some(isolated_zip.to_string_lossy().into_owned());
    state.oobe_completed = true;

    save_state(app, &state)?;
    Ok(state_to_view(&state))
}

#[tauri::command]
pub async fn fetch_supported_mods(app: AppHandle) -> Result<Vec<SupportedMod>, String> {
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || fetch_supported_mods_impl(&app_handle))
        .await
        .map_err(|err| format!("Error en tarea de consulta remota: {err}"))?
}

fn fetch_supported_mods_impl(_app: &AppHandle) -> Result<Vec<SupportedMod>, String> {
    let client = build_http_client()?;
    let remote_mods = fetch_remote_mods(&client)?;

    Ok(build_supported_mods(&remote_mods))
}

#[tauri::command]
pub fn cancel_installation(app: AppHandle, slug: String) -> Result<(), String> {
    let sanitized_slug = sanitize_install_slug(slug)?;
    let runtime_state = app.state::<LauncherRuntimeState>();
    if let Ok(mut cancelled) = runtime_state.cancelled_installations.lock() {
        cancelled.insert(sanitized_slug.clone());
    }
    emit_installation_progress_event(
        &app,
        &sanitized_slug,
        0,
        "Cancelling installation...",
        "running",
        None,
        None,
        None,
        None,
        None,
    );
    Ok(())
}

#[tauri::command]
pub fn execute_installation_recipe(
    app: AppHandle,
    slug: String,
    user_provided_zip_path: Option<String>,
) -> Result<(), String> {
    let sanitized_slug = sanitize_install_slug(slug)?;

    emit_installation_progress_event(
        &app,
        &sanitized_slug,
        0,
        "Queued...",
        "queued",
        None,
        None,
        None,
        None,
        None,
    );
    emit_installation_event(&app, &sanitized_slug, "started", "Installation started.");

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let app_for_worker = app_handle.clone();
        let slug_for_worker = sanitized_slug.clone();
        let zip_for_worker = user_provided_zip_path.clone();

        let slug_for_spawn = slug_for_worker.clone();
        let app_for_spawn = app_for_worker.clone();
        let blocking_result = tauri::async_runtime::spawn_blocking(move || {
            let slug_ref = slug_for_spawn.clone();
            execute_installation_recipe_impl(
                &app_for_spawn,
                &slug_for_spawn,
                zip_for_worker,
                |progress, status| {
                    emit_installation_progress_event(
                        &app_for_spawn,
                        &slug_ref,
                        progress,
                        status,
                        "running",
                        None,
                        None,
                        None,
                        None,
                        None,
                    );
                },
            )
        })
        .await;

        match blocking_result {
            Ok(Ok(_)) => {
                emit_installation_progress_event(
                    &app_handle,
                    &slug_for_worker,
                    100,
                    "Installation successful.",
                    "success",
                    None,
                    None,
                    None,
                    None,
                    None,
                );
                emit_installation_event(
                    &app_handle,
                    &slug_for_worker,
                    "success",
                    "Installation completed successfully.",
                );
            }
            Ok(Err(err)) => {
                let was_cancelled = if let Ok(cancelled) =
                    app_handle.state::<LauncherRuntimeState>().cancelled_installations.lock()
                {
                    cancelled.contains(&slug_for_worker)
                } else {
                    false
                };

                let (status_msg, state_msg) = if was_cancelled || err == "Installation cancelled by user." {
                    ("Installation cancelled.", "cancelled")
                } else {
                    ("Installation error.", "failed")
                };

                emit_installation_progress_event(
                    &app_handle,
                    &slug_for_worker,
                    0,
                    status_msg,
                    state_msg,
                    Some(err.clone()),
                    None,
                    None,
                    None,
                    None,
                );
                emit_installation_event(
                    &app_handle,
                    &slug_for_worker,
                    "failed",
                    &format!("Installation failed: {err}"),
                );
            }
            Err(join_err) => {
                let err_msg = format!("Critical failure in execution thread: {join_err}");
                emit_installation_progress_event(
                    &app_handle,
                    &slug_for_worker,
                    0,
                    "Panic failure.",
                    "failed",
                    Some(err_msg.clone()),
                    None,
                    None,
                    None,
                    None,
                );
                emit_installation_event(&app_handle, &slug_for_worker, "failed", &err_msg);
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn uninstall_mod(app: AppHandle, slug: String) -> Result<(), String> {
    let sanitized_slug = sanitize_install_slug(slug)?;
    let app_handle = app.clone();
    let slug_for_worker = sanitized_slug.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        uninstall_mod_impl(&app_handle, &slug_for_worker)
    })
    .await
    .map_err(|err| format!("Error en tarea de desinstalación: {err}"))?;

    if result.is_ok() {
        emit_installation_event(&app, &sanitized_slug, "uninstalled", "Uninstall completed.");
    }

    result
}

#[tauri::command]
pub fn get_running_mod_processes(
    app: AppHandle,
    runtime: State<'_, LauncherRuntimeState>,
) -> Result<Vec<String>, String> {
    let state = load_state(&app)?;
    if state.installed_mods.is_empty() {
        if let Ok(mut tracked) = runtime.running_processes.lock() {
            tracked.clear();
        }
        return Ok(Vec::new());
    }

    let running_paths = query_running_executable_paths()?;
    let mut running_slugs = Vec::new();

    for installed in &state.installed_mods {
        let install_path = to_absolute_path(Path::new(&installed.install_path))?;
        let normalized_install_path = normalize_process_path(&install_path);
        let is_running = running_paths
            .iter()
            .any(|path| is_process_in_directory(path, &normalized_install_path));
        if is_running {
            running_slugs.push(installed.slug.clone());
        }
    }

    if let Ok(mut tracked) = runtime.running_processes.lock() {
        tracked.retain(|slug, _| {
            running_slugs
                .iter()
                .any(|running_slug| running_slug == slug)
        });
        for slug in &running_slugs {
            tracked.entry(slug.clone()).or_insert(0);
        }
    }

    Ok(running_slugs)
}

#[tauri::command]
pub fn launch_installed_mod(
    app: AppHandle,
    runtime: State<'_, LauncherRuntimeState>,
    slug: String,
) -> Result<(), String> {
    let sanitized_slug = sanitize_install_slug(slug)?;

    let state = load_state(&app)?;
    let installed = state
        .installed_mods
        .iter()
        .find(|item| item.slug == sanitized_slug)
        .cloned()
        .ok_or_else(|| format!("No existe instalación registrada para `{sanitized_slug}`."))?;

    let executable_path = to_absolute_path(Path::new(&installed.executable_path))?;
    ensure_file_exists(&executable_path, "ejecutable instalado")?;
    let install_path = to_absolute_path(Path::new(&installed.install_path))?;
    if !path_exists(&install_path) {
        return Err(format!(
            "No se encontró la carpeta de instalación para `{sanitized_slug}` en `{}`.",
            install_path.display()
        ));
    }

    let tracked_pid = {
        let running = runtime
            .running_processes
            .lock()
            .map_err(|_| "No se pudo acceder al estado de procesos activos.".to_owned())?;
        running.get(&sanitized_slug).copied()
    };

    let running_paths = query_running_executable_paths()?;
    let normalized_install_path = normalize_process_path(&install_path);
    let is_already_running = running_paths
        .iter()
        .any(|path| is_process_in_directory(path, &normalized_install_path));
    if is_already_running {
        {
            let mut running = runtime
                .running_processes
                .lock()
                .map_err(|_| "No se pudo actualizar el estado de procesos activos.".to_owned())?;
            running.insert(sanitized_slug.clone(), 0);
        }
        emit_mod_process_status_event(&app, &sanitized_slug, true, None);
        return Ok(());
    }

    if tracked_pid.is_some() {
        if let Ok(mut running) = runtime.running_processes.lock() {
            running.remove(&sanitized_slug);
        }
    }

    let child = Command::new(&executable_path)
        .current_dir(&install_path)
        .spawn()
        .map_err(|err| {
            format!(
                "No se pudo iniciar el ejecutable `{}`: {err}",
                executable_path.display()
            )
        })?;

    let pid = child.id();
    {
        let mut running = runtime
            .running_processes
            .lock()
            .map_err(|_| "No se pudo actualizar el estado de procesos activos.".to_owned())?;
        running.insert(sanitized_slug.clone(), pid);
    }

    emit_mod_process_status_event(&app, &sanitized_slug, true, Some(pid));

    spawn_mod_watcher_thread(
        app,
        runtime.inner().clone().into(),
        sanitized_slug,
        normalized_install_path,
    );

    Ok(())
}

fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|err| format!("No se pudo inicializar cliente HTTP: {err}"))
}

fn fetch_remote_mods(client: &Client) -> Result<Vec<ClubModEnvelope>, String> {
    let response = client
        .get(crate::state::MOD_API_URL)
        .send()
        .map_err(|err| format!("No se pudo consultar API de Spanish Club: {err}"))?
        .error_for_status()
        .map_err(|err| format!("API de Spanish Club devolvió error HTTP: {err}"))?;

    let payload: ClubModsResponse = response
        .json()
        .map_err(|err| format!("No se pudo leer respuesta de API de Spanish Club: {err}"))?;
    Ok(payload.data)
}

fn build_supported_mods(remote_mods: &[ClubModEnvelope]) -> Vec<SupportedMod> {
    let mut mods = remote_mods
        .iter()
        .filter_map(|entry| {
            let slug = entry.resource.slug.trim();
            let download_url = entry.resource.download_pc.trim();
            if slug.is_empty() {
                return None;
            }

            let lower_url = download_url.to_lowercase();
            let is_gdrive = lower_url.contains("drive.google.com") || lower_url.contains("docs.google.com");
            let is_mediafire = lower_url.contains("mediafire.com");
            let downloadable = is_gdrive || is_mediafire;

            let creators = extract_credit_names(&entry.credits.creators);
            let translators = extract_credit_names(&entry.credits.translators);
            let porters = extract_credit_names(&entry.credits.porters);

            Some(SupportedMod {
                slug: slug.to_owned(),
                name: entry.resource.name.clone(),
                download_url: if download_url.is_empty() {
                    None
                } else {
                    Some(download_url.to_owned())
                },
                downloadable,
                status: entry.resource.status.clone(),
                current_version: entry.info.as_ref().and_then(|info| info.updated_at.clone()),
                executable: "DDLC.exe".to_owned(),
                description_html: entry.resource.description.clone(),
                hero_image_url: first_image_url(&entry.resource.images, "main"),
                logo_image_url: first_image_url(&entry.resource.images, "logo"),
                screenshot_urls: image_urls(&entry.resource.images, "screenshot"),
                genres: entry
                    .resource
                    .genres
                    .iter()
                    .map(|genre| genre.name.clone())
                    .collect(),
                credits: SupportedModCredits {
                    creators,
                    translators,
                    porters,
                },
            })
        })
        .collect::<Vec<_>>();

    mods.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    mods
}

struct CancelGuard {
    state: Arc<Mutex<HashSet<String>>>,
    slug: String,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Ok(mut cancelled) = self.state.lock() {
            cancelled.remove(&self.slug);
        }
    }
}

fn execute_installation_recipe_impl<F>(
    app: &AppHandle,
    slug: &str,
    user_provided_zip_path: Option<String>,
    mut report_progress: F,
) -> Result<InstallResult, String>
where
    F: FnMut(u8, &str),
{
    let runtime_state = app.state::<LauncherRuntimeState>();
    let is_cancelled = || {
        if let Ok(cancelled) = runtime_state.cancelled_installations.lock() {
            cancelled.contains(slug)
        } else {
            false
        }
    };

    let _guard = CancelGuard {
        state: runtime_state.cancelled_installations.clone(),
        slug: slug.to_owned(),
    };

    if is_cancelled() {
        return Err("Installation cancelled by user.".to_owned());
    }

    let state = load_state(app)?;
    let global_install_root = state
        .global_install_dir
        .as_ref()
        .ok_or_else(|| "El directorio de instalación no está configurado.".to_owned())?;
    let vanilla_zip_path = state
        .cached_ddlc_zip_path
        .as_ref()
        .ok_or_else(|| "El archivo original de DDLC no está configurado.".to_owned())?;

    let install_root = to_absolute_path(Path::new(global_install_root))?;
    let vanilla_zip = to_absolute_path(Path::new(vanilla_zip_path))?;
    ensure_file_exists(&vanilla_zip, "ZIP base original")?;

    report_progress(5, "Verifying base game integrity...");
    let base_hash = compute_sha256_chunked(&vanilla_zip)?;
    if detect_archive_format(&vanilla_zip)? == ArchiveFormat::Zip
        && !base_hash.eq_ignore_ascii_case(EXPECTED_DDLC_SHA256)
    {
        return Err(format!(
            "The base DDLC ZIP does not match the expected hash. Calculated hash: {base_hash}"
        ));
    }
    debug_log(format!(
        "Install start slug=`{slug}` install_root=`{}` vanilla_archive=`{}`",
        install_root.display(),
        vanilla_zip.display()
    ));

    if is_cancelled() {
        return Err("Installation cancelled by user.".to_owned());
    }

    report_progress(30, "Connecting to server...");
    let client = build_http_client()?;
    let remote_mods = fetch_remote_mods(&client)?;
    let selected_mod = remote_mods
        .into_iter()
        .find(|item| item.resource.slug == slug)
        .ok_or_else(|| format!("Mod `{slug}` not found in remote API."))?;

    let mod_download_url = selected_mod.resource.download_pc.trim().to_owned();
    let lower_url = mod_download_url.to_lowercase();
    let is_gdrive = lower_url.contains("drive.google.com") || lower_url.contains("docs.google.com");
    let is_mediafire = lower_url.contains("mediafire.com");
    let downloadable = is_gdrive || is_mediafire;

    report_progress(45, "Preparing mod archive...");
    let mod_zip_path = if downloadable && user_provided_zip_path.is_none() {
        if mod_download_url.is_empty() {
            return Err(format!(
                "Mod `{slug}` is marked as downloadable but has no valid download URL."
            ));
        }

        let cache_dir = crate::state::cache_dir_path(app)?;
        let cache_path = crate::downloader::download_to_file(
            &client,
            &mod_download_url,
            &cache_dir,
            &sanitize_slug_for_filename(slug),
            &is_cancelled,
            |downloaded, total, speed, eta| {
                let progress = if total > 0 {
                    ((downloaded as f64 / total as f64) * 45.0).round() as u8
                } else {
                    40
                };
                crate::state::emit_installation_progress_event(
                    app,
                    slug,
                    progress,
                    "Downloading mod...",
                    "running",
                    None,
                    Some(speed),
                    Some(eta),
                    Some(downloaded),
                    Some(total),
                );
            },
        )?;
        cache_path
    } else {
        let provided = user_provided_zip_path
            .as_ref()
            .map(|path| path.trim())
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                format!(
                    "Mod `{slug}` requires manual download. Please select the file before installing."
                )
            })?;
        let provided_path = to_absolute_path(Path::new(provided))?;
        ensure_file_exists(&provided_path, "mod file")?;
        detect_archive_format(&provided_path)?;

        provided_path
    };
    debug_log(format!(
        "Install archives slug=`{slug}` base=`{}` mod_archive=`{}` downloadable={}",
        vanilla_zip.display(),
        mod_zip_path.display(),
        downloadable
    ));

    report_progress(48, "Preparing installation directory...");
    let target_dir = install_root.join(slug);
    if path_exists(&target_dir) {
        crate::utils::remove_dir_all_safe(&target_dir)?;
    }
    create_dir_all_safe(&target_dir)?;

    let executable_name = match crate::recipes::install_mod_files_generic(
        &target_dir,
        &vanilla_zip,
        &mod_zip_path,
        &is_cancelled,
        |progress, status| {
            report_progress(progress, status);
        },
    ) {
        Ok(exe) => exe,
        Err(err) => {
            cleanup_failed_installation_target(&target_dir);
            return Err(err);
        }
    };

    let executable_path = target_dir.join(&executable_name);

    let installed_mod = InstalledMod {
        slug: slug.to_owned(),
        install_path: target_dir.to_string_lossy().into_owned(),
        current_version: selected_mod
            .info
            .as_ref()
            .and_then(|info| info.updated_at.clone()),
        executable_path: executable_path.to_string_lossy().into_owned(),
        installed_at_epoch_ms: now_epoch_millis(),
    };

    report_progress(98, "Registering installation...");
    let mut current_state = load_state(app)?;
    crate::state::upsert_installed_mod(&mut current_state.installed_mods, installed_mod);
    save_state(app, &current_state)?;
    report_progress(100, "Installation finished.");

    Ok(InstallResult {
        slug: slug.to_owned(),
        install_path: target_dir.to_string_lossy().into_owned(),
        executable_path: executable_path.to_string_lossy().into_owned(),
    })
}

fn uninstall_mod_impl(app: &AppHandle, slug: &str) -> Result<(), String> {
    let mut state = load_state(app)?;
    let index = state
        .installed_mods
        .iter()
        .position(|entry| entry.slug == slug)
        .ok_or_else(|| format!("El mod `{slug}` no está registrado como instalado."))?;

    let installed = &state.installed_mods[index];
    let install_path = to_absolute_path(Path::new(&installed.install_path))?;

    debug_log(format!(
        "Uninstall start slug=`{slug}` install_path=`{}`",
        install_path.display()
    ));

    if path_exists(&install_path) {
        crate::utils::remove_dir_all_safe(&install_path)?;
    }

    state.installed_mods.remove(index);
    save_state(app, &state)?;

    Ok(())
}
