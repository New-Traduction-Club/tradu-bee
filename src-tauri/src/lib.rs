use reqwest::blocking::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use unrar::Archive as RarArchive;
use zip::ZipArchive;

const MOD_API_URL: &str = "https://api-new.dokidokispanish.club/mod/all";
const EXPECTED_DDLC_SHA256: &str = "2A3DD7969A06729A32ACE0A6ECE5F2327E29BDF460B8B39E6A8B0875E545632E";
const STATE_DB_FILE_NAME: &str = "launcher_state.db";
const LEGACY_STATE_FILE_NAME: &str = "user_state.json";
const CACHE_DIR_NAME: &str = "cache";
const OOBE_DIR_NAME: &str = "oobe";
const OOBE_ORIGINAL_ARCHIVE_NAME: &str = "ddlc-original.zip";
const RECIPES_MANIFEST_URL: &str = "https://raw.githubusercontent.com/Just3090/random_shit/refs/heads/main/random.json";
const DEFAULT_MANIFEST_URL_HINT: &str = RECIPES_MANIFEST_URL;
const HASH_CHUNK_SIZE: usize = 1024 * 1024;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    Zip,
    Rar,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct LauncherState {
    manifest_url: Option<String>,
    global_install_dir: Option<String>,
    cached_ddlc_zip_path: Option<String>,
    oobe_completed: bool,
    installed_mods: Vec<InstalledMod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstalledMod {
    slug: String,
    install_path: String,
    current_version: Option<String>,
    executable_path: String,
    installed_at_epoch_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherStateView {
    manifest_url: Option<String>,
    global_install_dir: String,
    cached_ddlc_zip_path: Option<String>,
    oobe_completed: bool,
    installed_mods: Vec<InstalledMod>,
    expected_ddlc_sha256: String,
    manifest_url_hint: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateLauncherConfigRequest {
    manifest_url: Option<String>,
    global_install_dir: Option<String>,
    cached_ddlc_zip_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BaseZipValidation {
    path: String,
    computed_sha256: String,
    expected_sha256: String,
    is_valid: bool,
    warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportedMod {
    slug: String,
    name: String,
    download_url: Option<String>,
    downloadable: bool,
    status: String,
    current_version: Option<String>,
    executable: String,
    description_html: String,
    hero_image_url: Option<String>,
    logo_image_url: Option<String>,
    screenshot_urls: Vec<String>,
    genres: Vec<String>,
    credits: SupportedModCredits,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportedModCredits {
    creators: Vec<String>,
    translators: Vec<String>,
    porters: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallResult {
    slug: String,
    install_path: String,
    executable_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallationEvent {
    slug: String,
    status: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallationProgressEvent {
    slug: String,
    progress: u8,
    status: String,
    state: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModProcessStatusEvent {
    slug: String,
    is_running: bool,
    pid: Option<u32>,
}

#[derive(Clone, Default)]
struct LauncherRuntimeState {
    running_processes: Arc<Mutex<HashMap<String, u32>>>,
}

#[derive(Debug, Deserialize, Clone)]
struct ClubModsResponse {
    data: Vec<ClubModEnvelope>,
}

#[derive(Debug, Deserialize, Clone)]
struct ClubModEnvelope {
    resource: ClubModResource,
    #[serde(default)]
    info: Option<ClubModInfo>,
    #[serde(default)]
    credits: ClubCredits,
}

#[derive(Debug, Deserialize, Clone)]
struct ClubModResource {
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    download_pc: String,
    #[serde(default)]
    images: Vec<ClubImage>,
    #[serde(default)]
    genres: Vec<ClubGenre>,
}

#[derive(Debug, Deserialize, Clone)]
struct ClubModInfo {
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ClubImage {
    #[serde(default)]
    url: String,
    #[serde(default)]
    r#type: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ClubGenre {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize, Clone)]
struct ClubCredits {
    #[serde(default)]
    creators: Vec<ClubCreditEntry>,
    #[serde(default)]
    translators: Vec<ClubCreditEntry>,
    #[serde(default)]
    porters: Vec<ClubCreditEntry>,
}

#[derive(Debug, Deserialize, Clone)]
struct ClubCreditEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    user: Option<ClubCreditUser>,
}

#[derive(Debug, Deserialize, Clone)]
struct ClubCreditUser {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecipeManifest {
    #[allow(dead_code)]
    manifest_version: String,
    recipes: HashMap<String, ModRecipe>,
}

#[derive(Debug, Deserialize, Clone)]
struct ModRecipe {
    is_supported: bool,
    #[serde(default = "default_true")]
    downloadable: bool,
    executable: String,
    steps: Vec<RecipeStep>,
}

#[derive(Debug, Deserialize, Clone)]
struct RecipeStep {
    action: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

#[tauri::command]
fn get_launcher_state(app: AppHandle) -> Result<LauncherStateView, String> {
    let state = load_state(&app)?;
    Ok(state_to_view(&state))
}

#[tauri::command]
fn update_launcher_config(
    app: AppHandle,
    request: UpdateLauncherConfigRequest,
) -> Result<LauncherStateView, String> {
    let mut state = load_state(&app)?;

    if let Some(raw_manifest_url) = request.manifest_url {
        let trimmed = raw_manifest_url.trim().to_owned();
        if trimmed.is_empty() {
            state.manifest_url = None;
        } else {
            validate_manifest_url(&trimmed)?;
            state.manifest_url = Some(trimmed);
        }
    }

    if let Some(raw_install_dir) = request.global_install_dir {
        let trimmed = raw_install_dir.trim().to_owned();
        let resolved = if trimmed.is_empty() {
            default_install_dir()
        } else {
            to_absolute_path(Path::new(&trimmed))?
        };

        ensure_install_dir_allowed(&resolved)?;
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
async fn validate_vanilla_zip(
    app: AppHandle,
    path: Option<String>,
) -> Result<BaseZipValidation, String> {
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || validate_vanilla_zip_impl(&app_handle, path))
        .await
        .map_err(|err| format!("Error en tarea de validación: {err}"))?
}

fn validate_vanilla_zip_impl(
    app: &AppHandle,
    path: Option<String>,
) -> Result<BaseZipValidation, String> {
    let candidate_path = if let Some(raw_path) = path {
        let trimmed = raw_path.trim().to_owned();
        if trimmed.is_empty() {
            let state = load_state(app)?;
            state
                .cached_ddlc_zip_path
                .ok_or_else(|| "No hay ZIP base configurado.".to_owned())?
        } else {
            trimmed
        }
    } else {
        let state = load_state(app)?;
        state
            .cached_ddlc_zip_path
            .ok_or_else(|| "No hay ZIP base configurado.".to_owned())?
    };

    let absolute_path = to_absolute_path(Path::new(&candidate_path))?;
    ensure_file_exists(&absolute_path, "archivo base de DDLC")?;
    let archive_format = detect_archive_format(&absolute_path)?;
    let computed = compute_sha256_chunked(&absolute_path)?;
    let (expected_sha256, is_valid, warning) = match archive_format {
        ArchiveFormat::Zip => (
            EXPECTED_DDLC_SHA256.to_owned(),
            computed.eq_ignore_ascii_case(EXPECTED_DDLC_SHA256),
            None,
        ),
        ArchiveFormat::Rar => (
            "N/A (RAR)".to_owned(),
            true,
            Some(
                "Formato RAR detectado: no existe verificación para validación segura."
                    .to_owned(),
            ),
        ),
    };

    Ok(BaseZipValidation {
        path: absolute_path.to_string_lossy().into_owned(),
        computed_sha256: computed,
        expected_sha256,
        is_valid,
        warning,
    })
}

#[tauri::command]
async fn finalize_oobe_setup(
    app: AppHandle,
    original_zip_path: String,
    global_install_dir: Option<String>,
) -> Result<LauncherStateView, String> {
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        finalize_oobe_setup_impl(&app_handle, &original_zip_path, global_install_dir)
    })
        .await
        .map_err(|err| format!("Debug: error en tarea de OOBE: {err}"))?
}

fn finalize_oobe_setup_impl(
    app: &AppHandle,
    original_zip_path: &str,
    global_install_dir: Option<String>,
) -> Result<LauncherStateView, String> {
    let trimmed = original_zip_path.trim();
    if trimmed.is_empty() {
        return Err("Selecciona primero el ZIP original de DDLC para continuar.".to_owned());
    }

    let source_original_zip = to_absolute_path(Path::new(trimmed))?;
    ensure_file_exists(&source_original_zip, "ZIP original de DDLC")?;
    match detect_archive_format(&source_original_zip)? {
        ArchiveFormat::Zip => {}
        ArchiveFormat::Rar => {
            return Err(
                "Se requiere el archivo original en formato .zip.".to_owned(),
            )
        }
    }

    let computed = compute_sha256_chunked(&source_original_zip)?;
    if !computed.eq_ignore_ascii_case(EXPECTED_DDLC_SHA256) {
        return Err(format!(
            "El ZIP original no coincide con la verificación esperada."
        ));
    }

    let oobe_dir = oobe_dir_path(app)?;
    let isolated_zip = oobe_dir.join(OOBE_ORIGINAL_ARCHIVE_NAME);
    copy_file_secure(&source_original_zip, &isolated_zip)?;
    let copied_hash = compute_sha256_chunked(&isolated_zip)?;
    if !copied_hash.eq_ignore_ascii_case(EXPECTED_DDLC_SHA256) {
        return Err(format!(
            "La copia local del ZIP original quedó corrupta."
        ));
    }

    let mut state = load_state(app)?;
    state.cached_ddlc_zip_path = Some(isolated_zip.to_string_lossy().into_owned());
    if state
        .manifest_url
        .as_ref()
        .map(|url| url.trim().is_empty())
        .unwrap_or(true)
    {
        state.manifest_url = Some(RECIPES_MANIFEST_URL.to_owned());
    }

    if let Some(raw_install_dir) = global_install_dir {
        let trimmed_install_dir = raw_install_dir.trim();
        if trimmed_install_dir.is_empty() {
            state.global_install_dir = Some(default_install_dir().to_string_lossy().into_owned());
        } else {
            let resolved_install_dir = to_absolute_path(Path::new(trimmed_install_dir))?;
            ensure_install_dir_allowed(&resolved_install_dir)?;
            create_dir_all_safe(&resolved_install_dir)?;
            state.global_install_dir = Some(resolved_install_dir.to_string_lossy().into_owned());
        }
    }

    state.oobe_completed = true;
    if state
        .global_install_dir
        .as_ref()
        .map(|path| path.trim().is_empty())
        .unwrap_or(true)
    {
        state.global_install_dir = Some(default_install_dir().to_string_lossy().into_owned());
    }

    save_state(app, &state)?;
    Ok(state_to_view(&state))
}

#[tauri::command]
async fn fetch_supported_mods(app: AppHandle) -> Result<Vec<SupportedMod>, String> {
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || fetch_supported_mods_impl(&app_handle))
        .await
        .map_err(|err| format!("Error en tarea de consulta remota: {err}"))?
}

fn fetch_supported_mods_impl(app: &AppHandle) -> Result<Vec<SupportedMod>, String> {
    let state = load_state(app)?;
    let manifest_url = resolve_manifest_url(state.manifest_url.as_deref());

    let client = build_http_client()?;
    let manifest = fetch_recipe_manifest(&client, &manifest_url)?;
    let remote_mods = fetch_remote_mods(&client)?;

    Ok(build_supported_mods(&manifest, &remote_mods))
}

#[tauri::command]
fn execute_installation_recipe(
    app: AppHandle,
    slug: String,
    user_provided_zip_path: Option<String>,
) -> Result<(), String> {
    let sanitized_slug = sanitize_install_slug(slug)?;

    emit_installation_progress_event(
        &app,
        &sanitized_slug,
        0,
        "En cola...",
        "queued",
        None,
    );
    emit_installation_event(
        &app,
        &sanitized_slug,
        "started",
        "Instalación enviada al gestor en segundo plano.",
    );

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let app_for_worker = app_handle.clone();
        let slug_for_worker = sanitized_slug.clone();
        let zip_for_worker = user_provided_zip_path.clone();
        let blocking_result = tauri::async_runtime::spawn_blocking(move || {
            execute_installation_recipe_impl(
                &app_for_worker,
                &slug_for_worker,
                zip_for_worker,
                |progress, status| {
                    emit_installation_progress_event(
                        &app_for_worker,
                        &slug_for_worker,
                        progress,
                        status,
                        "running",
                        None,
                    );
                },
            )
        })
        .await;

        match blocking_result {
            Ok(Ok(result)) => {
                emit_installation_progress_event(
                    &app_handle,
                    &sanitized_slug,
                    100,
                    "Instalación completada.",
                    "success",
                    None,
                );
                emit_installation_event(
                    &app_handle,
                    &sanitized_slug,
                    "success",
                    &format!(
                        "Instalación finalizada: {} -> {}",
                        result.slug, result.executable_path
                    ),
                );
            }
            Ok(Err(err)) => {
                emit_installation_progress_event(
                    &app_handle,
                    &sanitized_slug,
                    100,
                    "Instalación fallida.",
                    "failed",
                    Some(err.clone()),
                );
                emit_installation_event(&app_handle, &sanitized_slug, "failed", &err);
            }
            Err(err) => {
                let message = format!("Error en tarea de instalación: {err}");
                emit_installation_progress_event(
                    &app_handle,
                    &sanitized_slug,
                    100,
                    "Instalación fallida.",
                    "failed",
                    Some(message.clone()),
                );
                emit_installation_event(&app_handle, &sanitized_slug, "failed", &message);
            }
        }
    });

    Ok(())
}

#[tauri::command]
async fn uninstall_mod(app: AppHandle, slug: String) -> Result<(), String> {
    let sanitized_slug = sanitize_install_slug(slug)?;
    let app_handle = app.clone();
    let slug_for_worker = sanitized_slug.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        uninstall_mod_impl(&app_handle, &slug_for_worker)
    })
    .await
    .map_err(|err| format!("Error en tarea de desinstalación: {err}"))?;

    if result.is_ok() {
        emit_installation_event(
            &app,
            &sanitized_slug,
            "uninstalled",
            "Desinstalación completada.",
        );
    }

    result
}

#[tauri::command]
fn get_running_mod_processes(
    app: AppHandle,
    runtime: tauri::State<'_, LauncherRuntimeState>,
) -> Result<Vec<String>, String> {
    let state = load_state(&app)?;
    let running_paths = query_running_executable_paths()?;
    let mut running_slugs = Vec::new();

    for installed in &state.installed_mods {
        let executable_path = to_absolute_path(Path::new(&installed.executable_path))?;
        let normalized_path = normalize_process_path(&executable_path);
        if running_paths.contains(&normalized_path) {
            running_slugs.push(installed.slug.clone());
        }
    }

    if let Ok(mut tracked) = runtime.running_processes.lock() {
        tracked.retain(|slug, _| running_slugs.iter().any(|running_slug| running_slug == slug));
        for slug in &running_slugs {
            tracked.entry(slug.clone()).or_insert(0);
        }
    }

    Ok(running_slugs)
}

#[tauri::command]
fn launch_installed_mod(
    app: AppHandle,
    runtime: tauri::State<'_, LauncherRuntimeState>,
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
    let normalized_executable_path = normalize_process_path(&executable_path);
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
    if running_paths.contains(&normalized_executable_path) {
        {
            let mut running = runtime
                .running_processes
                .lock()
                .map_err(|_| "No se pudo actualizar el estado de procesos activos.".to_owned())?;
            running.insert(sanitized_slug.clone(), 0);
        }
        emit_mod_process_status_event(&app, &sanitized_slug, true, None);
        emit_installation_event(
            &app,
            &sanitized_slug,
            "playing",
            "El juego ya estaba en ejecución.",
        );
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
    emit_installation_event(
        &app,
        &sanitized_slug,
        "playing",
        &format!("Ejecutable iniciado (PID {pid})."),
    );

    let app_for_watch = app.clone();
    let runtime_for_watch = runtime.inner().clone();
    let slug_for_watch = sanitized_slug.clone();
    let executable_for_watch = normalized_executable_path.clone();
    std::thread::spawn(move || {
        let mut started = false;
        for _ in 0..20 {
            let running_paths = query_running_executable_paths();
            if running_paths
                .as_ref()
                .map(|paths| paths.contains(&executable_for_watch))
                .unwrap_or(false)
            {
                started = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(350));
        }

        if !started {
            if let Ok(mut running) = runtime_for_watch.running_processes.lock() {
                running.remove(&slug_for_watch);
            }
            emit_mod_process_status_event(&app_for_watch, &slug_for_watch, false, None);
            emit_installation_event(
                &app_for_watch,
                &slug_for_watch,
                "stopped",
                "No se detectó el proceso en ejecución tras iniciar el juego.",
            );
            return;
        }

        let mut missing_checks = 0u8;
        loop {
            let is_running = query_running_executable_paths()
                .map(|paths| paths.contains(&executable_for_watch))
                .unwrap_or(false);

            if is_running {
                missing_checks = 0;
            } else {
                missing_checks = missing_checks.saturating_add(1);
            }

            if missing_checks >= 3 {
                break;
            }

            std::thread::sleep(Duration::from_millis(1200));
        }

        if let Ok(mut running) = runtime_for_watch.running_processes.lock() {
            running.remove(&slug_for_watch);
        }
        emit_mod_process_status_event(&app_for_watch, &slug_for_watch, false, None);
        emit_installation_event(
            &app_for_watch,
            &slug_for_watch,
            "stopped",
            "Proceso finalizado.",
        );
    });

    Ok(())
}

fn query_running_executable_paths() -> Result<HashSet<String>, String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath } | ForEach-Object { $_.ExecutablePath }",
            ])
            .output()
            .map_err(|err| format!("No se pudo consultar procesos del sistema: {err}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            if stderr.is_empty() {
                return Err("No se pudo consultar procesos del sistema.".to_owned());
            }
            return Err(format!("No se pudo consultar procesos del sistema: {stderr}"));
        }

        let mut running_paths = HashSet::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            running_paths.insert(normalize_process_path(Path::new(trimmed)));
        }

        Ok(running_paths)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(HashSet::new())
    }
}

fn normalize_process_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_lowercase()
        .trim_start_matches(r"\\?\")
        .to_owned()
}

fn sanitize_install_slug(slug: String) -> Result<String, String> {
    let sanitized_slug = slug.trim().to_owned();
    if sanitized_slug.is_empty() {
        return Err("El slug del mod no puede estar vacío.".to_owned());
    }
    if sanitized_slug.contains('/') || sanitized_slug.contains('\\') || sanitized_slug.contains("..")
    {
        return Err("El slug contiene caracteres no válidos para una ruta local.".to_owned());
    }

    Ok(sanitized_slug)
}

fn uninstall_mod_impl(app: &AppHandle, slug: &str) -> Result<(), String> {
    let mut state = load_state(app)?;
    let index = state
        .installed_mods
        .iter()
        .position(|installed| installed.slug == slug)
        .ok_or_else(|| format!("No existe instalación registrada para `{slug}`."))?;

    let installed = state.installed_mods.remove(index);
    let install_path = to_absolute_path(Path::new(&installed.install_path))?;
    if path_exists(&install_path) {
        remove_dir_all_safe(&install_path)?;
    }

    save_state(app, &state)?;
    Ok(())
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
    report_progress(5, "Validando configuración local...");
    let mut state = load_state(app)?;
    let manifest_url = resolve_manifest_url(state.manifest_url.as_deref());
    let install_dir = state
        .global_install_dir
        .clone()
        .ok_or_else(|| "Falta configurar el directorio global de instalación.".to_owned())?;
    let vanilla_zip_path = state
        .cached_ddlc_zip_path
        .clone()
        .ok_or_else(|| "Falta configurar la ruta al ZIP original de DDLC.".to_owned())?;

    let install_root = to_absolute_path(Path::new(&install_dir))?;
    ensure_install_dir_allowed(&install_root)?;
    create_dir_all_safe(&install_root)?;

    let vanilla_zip = to_absolute_path(Path::new(&vanilla_zip_path))?;
    ensure_file_exists(&vanilla_zip, "archivo base de DDLC")?;
    let vanilla_archive_format = detect_archive_format(&vanilla_zip)?;
    report_progress(15, "Validando hash del juego original...");
    let base_hash = compute_sha256_chunked(&vanilla_zip)?;
    if vanilla_archive_format == ArchiveFormat::Zip
        && !base_hash.eq_ignore_ascii_case(EXPECTED_DDLC_SHA256)
    {
        return Err(format!(
            "El ZIP base de DDLC no coincide con la verificación esperada. Hash calculado: {base_hash}"
        ));
    }
    debug_log(format!(
        "Install start slug=`{slug}` install_root=`{}` vanilla_archive=`{}` vanilla_format={vanilla_archive_format:?}",
        install_root.display(),
        vanilla_zip.display()
    ));

    report_progress(30, "Conectando con el servidor...");
    let client = build_http_client()?;
    let manifest = fetch_recipe_manifest(&client, &manifest_url)?;
    let remote_mods = fetch_remote_mods(&client)?;
    let selected_mod = remote_mods
        .into_iter()
        .find(|item| item.resource.slug == slug)
        .ok_or_else(|| format!("No se encontró el mod `{slug}` en la API remota."))?;

    let recipe = manifest
        .recipes
        .get(slug)
        .ok_or_else(|| format!("No existen instrucciones para `{slug}` en el archivo remoto."))?;
    if !recipe.is_supported {
        return Err(format!(
            "El mod `{slug}` está marcado como no soportado en el servidor."
        ));
    }

    report_progress(45, "Preparando archivo del mod...");
    let mod_zip_path = if recipe.downloadable {
        let mod_download_url = selected_mod.resource.download_pc.trim().to_owned();
        if mod_download_url.is_empty() {
            return Err(format!(
                "El mod `{slug}` está marcado como descargable, pero no tiene un link válido."
            ));
        }

        let cache_dir = cache_dir_path(app)?;
        let archive_extension =
            infer_archive_extension_from_url(&mod_download_url).unwrap_or("zip");
        let cache_path = cache_dir.join(format!(
            "{}.{}",
            sanitize_slug_for_filename(slug),
            archive_extension
        ));
        if path_exists(&cache_path) {
            remove_file_safe(&cache_path)?;
        }
        download_to_file(&client, &mod_download_url, &cache_path)?;
        detect_archive_format(&cache_path)?;
        cache_path
    } else {
        let provided = user_provided_zip_path
            .as_ref()
            .map(|path| path.trim())
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                format!(
                    "El mod `{slug}` requiere descarga manual. Selecciona el archivo antes de instalar."
                )
            })?;
        let provided_path = to_absolute_path(Path::new(provided))?;
        ensure_file_exists(&provided_path, "archivo del mod")?;
        detect_archive_format(&provided_path)?;

        provided_path
    };
    debug_log(format!(
        "Install archives slug=`{slug}` base=`{}` mod_archive=`{}` downloadable={}",
        vanilla_zip.display(),
        mod_zip_path.display(),
        recipe.downloadable
    ));

    report_progress(55, "Preparando directorio de instalación...");
    let target_dir = install_root.join(slug);
    if path_exists(&target_dir) {
        remove_dir_all_safe(&target_dir)?;
    }
    create_dir_all_safe(&target_dir)?;

    if let Err(err) = run_recipe_steps(recipe, &target_dir, &vanilla_zip, &mod_zip_path, |progress, status| {
        report_progress(progress, status);
    }) {
        cleanup_failed_installation_target(&target_dir);
        return Err(format!("{err}{}", debug_preserve_note(&target_dir)));
    }

    report_progress(90, "Validando ejecutable final...");
    let executable_path = resolve_recipe_path(&target_dir, recipe.executable.as_str())?;
    if !path_exists(&executable_path) {
        cleanup_failed_installation_target(&target_dir);
        return Err(format!(
            "La instalación terminó, pero no se encontró el ejecutable `{}`.{}",
            executable_path.display(),
            debug_preserve_note(&target_dir)
        ));
    }

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

    report_progress(96, "Registrando instalación...");
    upsert_installed_mod(&mut state.installed_mods, installed_mod);
    save_state(app, &state)?;
    report_progress(100, "Instalación finalizada.");

    Ok(InstallResult {
        slug: slug.to_owned(),
        install_path: target_dir.to_string_lossy().into_owned(),
        executable_path: executable_path.to_string_lossy().into_owned(),
    })
}

fn run_recipe_steps(
    recipe: &ModRecipe,
    target_dir: &Path,
    vanilla_zip: &Path,
    mod_zip: &Path,
    mut report_progress: impl FnMut(u8, &str),
) -> Result<(), String> {
    let total_steps = recipe.steps.len().max(1);
    for (index, step) in recipe.steps.iter().enumerate() {
        let step_progress = 60 + (((index as f32) / (total_steps as f32)) * 25.0).round() as u8;
        let status = format!(
            "Paso {}/{}: {}",
            index + 1,
            total_steps,
            recipe_action_label(step.action.as_str())
        );
        report_progress(step_progress.min(85), &status);

        match step.action.as_str() {
            "extract_base" => {
                let destination =
                    resolve_recipe_path(target_dir, step.destination.as_deref().unwrap_or("./"))?;
                extract_archive_here(vanilla_zip, &destination)?;
            }
            "extract_mod" => {
                let destination =
                    resolve_recipe_path(target_dir, step.destination.as_deref().unwrap_or("./"))?;
                extract_archive_here(mod_zip, &destination)?;
            }
            "copy_overwrite" => {
                let source_requested = resolve_recipe_path(
                    target_dir,
                    step.source
                        .as_deref()
                        .ok_or_else(|| "Paso copy_overwrite requiere `source`.".to_owned())?,
                )?;
                let source = resolve_copy_source_path(&source_requested)?;
                let destination = resolve_recipe_path(
                    target_dir,
                    step.destination
                        .as_deref()
                        .ok_or_else(|| "Paso copy_overwrite requiere `destination`.".to_owned())?,
                )?;
                recursive_copy(&source, &destination)?;
            }
            "delete_file" => {
                let target = resolve_recipe_path(
                    target_dir,
                    step.target
                        .as_deref()
                        .ok_or_else(|| "Paso delete_file requiere `target`.".to_owned())?,
                )?;
                if path_exists(&target) {
                    if path_is_file(&target) {
                        remove_file_safe(&target)?;
                    } else {
                        return Err(format!(
                            "delete_file esperaba un archivo, pero encontró directorio: {}",
                            target.display()
                        ));
                    }
                }
            }
            "cleanup_temp" => {
                let target = resolve_recipe_path(
                    target_dir,
                    step.target
                        .as_deref()
                        .ok_or_else(|| "Paso cleanup_temp requiere `target`.".to_owned())?,
                )?;
                if path_exists(&target) {
                    if path_is_file(&target) {
                        remove_file_safe(&target)?;
                    } else {
                        remove_dir_all_safe(&target)?;
                    }
                }
            }
            other => {
                return Err(format!(
                    "Las instrucciones contienen una operación no soportada: `{other}`."
                ));
            }
        }
    }

    Ok(())
}

fn recipe_action_label(action: &str) -> &'static str {
    match action {
        "extract_base" => "Extrayendo juego base",
        "extract_mod" => "Extrayendo mod",
        "copy_overwrite" => "Copiando archivos del mod",
        "delete_file" => "Eliminando archivo",
        "cleanup_temp" => "Limpiando archivos temporales",
        _ => "Ejecutando acción",
    }
}

fn resolve_recipe_path(root: &Path, recipe_relative_path: &str) -> Result<PathBuf, String> {
    let trimmed = recipe_relative_path.trim();
    if trimmed.is_empty() {
        return Ok(root.to_path_buf());
    }

    let recipe_path = Path::new(trimmed);
    if recipe_path.is_absolute() {
        return Err(format!(
            "Las rutas absolutas no están permitidas en las instrucciones: `{trimmed}`"
        ));
    }

    let mut normalized = PathBuf::new();
    for component in recipe_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Ruta insegura en las instrucciones (usa solo rutas relativas): `{trimmed}`"
                ));
            }
        }
    }

    Ok(root.join(normalized))
}

fn resolve_copy_source_path(source: &Path) -> Result<PathBuf, String> {
    if path_exists(source) {
        return Ok(source.to_path_buf());
    }

    let parent = source.parent().ok_or_else(|| {
        format!(
            "No se pudo acceder a `{}` y no existe carpeta padre para inferencia.",
            source.display()
        )
    })?;
    if !path_exists(parent) {
        return Err(format!(
            "No se pudo acceder a `{}` porque la carpeta padre `{}` no existe.",
            source.display(),
            parent.display()
        ));
    }

    let entries = fs::read_dir(fs_path(parent))
        .map_err(|err| format!("No se pudo leer `{}` para inferencia: {err}", parent.display()))?;
    let mut available_dirs = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "No se pudo inspeccionar contenido de `{}` para inferencia: {err}",
                parent.display()
            )
        })?;
        let file_type = entry
            .file_type()
            .map_err(|err| format!("No se pudo leer tipo de entrada en inferencia: {err}"))?;
        if file_type.is_dir() {
            available_dirs.push(parent.join(entry.file_name()));
        }
    }

    if available_dirs.len() == 1 {
        let inferred = available_dirs[0].clone();
        debug_log(format!(
            "copy_overwrite source missing. requested=`{}` inferred=`{}`",
            source.display(),
            inferred.display()
        ));
        return Ok(inferred);
    }

    let available = if available_dirs.is_empty() {
        "(ninguno)".to_owned()
    } else {
        available_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(format!(
        "No se pudo acceder a `{}`. Directorios disponibles en `{}`: {}.",
        source.display(),
        parent.display(),
        available
    ))
}

fn sanitize_archive_entry_path(entry_path: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    for component in entry_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Entrada de archivo comprimido con ruta insegura: `{}`",
                    entry_path.display()
                ));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err("Entrada de archivo comprimido con ruta vacía.".to_owned());
    }

    Ok(normalized)
}

fn extract_archive_here(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let archive_format = detect_archive_format(archive_path)?;
    debug_log(format!(
        "extract_archive_here format={archive_format:?} source=`{}` destination=`{}` destination_fs=`{}`",
        archive_path.display(),
        destination.display(),
        fs_path(destination).display()
    ));

    match archive_format {
        ArchiveFormat::Zip => extract_zip_archive(archive_path, destination),
        ArchiveFormat::Rar => extract_rar_archive(archive_path, destination),
    }
}

fn extract_rar_archive(archive_path: &Path, destination: &Path) -> Result<(), String> {
    ensure_file_exists(archive_path, "archivo RAR de entrada")?;
    create_dir_all_safe(destination)?;
    debug_log(format!(
        "RAR open source=`{}` destination=`{}` destination_fs=`{}`",
        archive_path.display(),
        destination.display(),
        fs_path(destination).display()
    ));

    let mut archive = RarArchive::new(archive_path)
        .open_for_processing()
        .map_err(|err| format!("No se pudo abrir RAR `{}`: {err}", archive_path.display()))?;

    while let Some(header) = archive.read_header().map_err(|err| {
        format!(
            "No se pudo leer cabecera en RAR `{}`: {err}",
            archive_path.display()
        )
    })? {
        let raw_entry_path = header.entry().filename.clone();
        let safe_entry_path = sanitize_archive_entry_path(&raw_entry_path)?;
        let entry_name = safe_entry_path.to_string_lossy().into_owned();
        let is_file = header.entry().is_file();
        let preview_output_path = destination.join(&safe_entry_path);
        let preview_output_len = preview_output_path.to_string_lossy().len();
        debug_log(format!(
            "RAR entry raw=`{}` safe=`{entry_name}` is_file={is_file} preview_output=`{}` preview_len={preview_output_len}",
            raw_entry_path.display(),
            preview_output_path.display()
        ));

        archive = if is_file {
            if let Some(parent) = preview_output_path.parent() {
                create_dir_all_safe(parent)?;
            }

            let (contents, next_archive) = header.read().map_err(|err| {
                format!(
                    "No se pudo leer contenido de entrada `{entry_name}` en `{}`: {err}",
                    archive_path.display()
                )
            })?;

            let mut output_file = File::create(fs_path(&preview_output_path)).map_err(|err| {
                format!(
                    "No se pudo crear archivo destino para entrada `{entry_name}` en `{}` (preview_output=`{}` | preview_len={preview_output_len}): {err}",
                    destination.display(),
                    preview_output_path.display()
                )
            })?;
            output_file.write_all(&contents).map_err(|err| {
                format!(
                    "No se pudo escribir archivo destino para entrada `{entry_name}` en `{}` (bytes={}): {err}",
                    preview_output_path.display(),
                    contents.len()
                )
            })?;
            debug_log(format!(
                "RAR entry wrote `{}` bytes={} to `{}`",
                entry_name,
                contents.len(),
                preview_output_path.display()
            ));
            next_archive
        } else {
            header.skip().map_err(|err| {
                format!(
                    "No se pudo procesar entrada no-archivo `{entry_name}` en `{}`: {err}",
                    archive_path.display(),
                )
            })?
        };
    }

    Ok(())
}

fn extract_zip_archive(zip_path: &Path, destination: &Path) -> Result<(), String> {
    ensure_file_exists(zip_path, "ZIP de entrada")?;
    create_dir_all_safe(destination)?;

    let file = File::open(fs_path(zip_path))
        .map_err(|err| format!("No se pudo abrir ZIP `{}`: {err}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|err| format!("No se pudo leer ZIP `{}`: {err}", zip_path.display()))?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|err| {
            format!(
                "No se pudo leer la entrada #{index} del ZIP `{}`: {err}",
                zip_path.display()
            )
        })?;

        let enclosed = entry.enclosed_name().ok_or_else(|| {
            format!(
                "ZIP inválido por ruta insegura en la entrada `{}`.",
                entry.name()
            )
        })?;
        let output_path = destination.join(enclosed);

        if entry.is_dir() {
            create_dir_all_safe(&output_path)?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            create_dir_all_safe(parent)?;
        }

        let mut output_file = File::create(fs_path(&output_path)).map_err(|err| {
            format!(
                "No se pudo crear el archivo extraído `{}`: {err}",
                output_path.display()
            )
        })?;
        io::copy(&mut entry, &mut output_file).map_err(|err| {
            format!(
                "Error al extraer `{}` hacia `{}`: {err}",
                entry.name(),
                output_path.display()
            )
        })?;
    }

    Ok(())
}

fn recursive_copy(source: &Path, destination: &Path) -> Result<(), String> {
    let source_meta = fs::metadata(fs_path(source))
        .map_err(|err| format!("No se pudo acceder a `{}`: {err}", source.display()))?;

    if source_meta.is_file() {
        copy_file_overwrite(source, destination)?;
        return Ok(());
    }

    create_dir_all_safe(destination)?;
    let entries = fs::read_dir(fs_path(source))
        .map_err(|err| format!("No se pudo leer el directorio `{}`: {err}", source.display()))?;

    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "No se pudo recorrer una entrada dentro de `{}`: {err}",
                source.display()
            )
        })?;
        let entry_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let entry_meta = entry
            .file_type()
            .map_err(|err| format!("No se pudo leer tipo de archivo: {err}"))?;

        if entry_meta.is_dir() {
            recursive_copy(&entry_path, &destination_path)?;
        } else if entry_meta.is_file() {
            copy_file_overwrite(&entry_path, &destination_path)?;
        }
    }

    Ok(())
}

fn copy_file_overwrite(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        create_dir_all_safe(parent)?;
    }

    fs::copy(fs_path(source), fs_path(destination)).map_err(|err| {
        format!(
            "No se pudo copiar `{}` hacia `{}`: {err}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn copy_file_secure(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        create_dir_all_safe(parent)?;
    }

    let tmp_destination = destination.with_extension("tmp");
    if path_exists(&tmp_destination) {
        remove_file_safe(&tmp_destination)?;
    }

    fs::copy(fs_path(source), fs_path(&tmp_destination)).map_err(|err| {
        format!(
            "No se pudo copiar `{}` hacia `{}`: {err}",
            source.display(),
            tmp_destination.display()
        )
    })?;

    if path_exists(destination) {
        remove_file_safe(destination)?;
    }
    fs::rename(fs_path(&tmp_destination), fs_path(destination)).map_err(|err| {
        format!(
            "No se pudo finalizar copia segura hacia `{}`: {err}",
            destination.display()
        )
    })?;

    Ok(())
}

fn download_to_file(client: &Client, url: &str, target_path: &Path) -> Result<(), String> {
    if let Some(parent) = target_path.parent() {
        create_dir_all_safe(parent)?;
    }

    let mut response = client
        .get(url)
        .send()
        .map_err(|err| format!("No se pudo descargar `{url}`: {err}"))?
        .error_for_status()
        .map_err(|err| format!("La descarga de `{url}` devolvió error HTTP: {err}"))?;

    let mut file = File::create(fs_path(target_path)).map_err(|err| {
        format!(
            "No se pudo crear el archivo cacheado `{}`: {err}",
            target_path.display()
        )
    })?;
    io::copy(&mut response, &mut file).map_err(|err| {
        format!(
            "No se pudo escribir el archivo descargado `{}`: {err}",
            target_path.display()
        )
    })?;

    Ok(())
}

fn compute_sha256_chunked(path: &Path) -> Result<String, String> {
    let mut file = File::open(fs_path(path))
        .map_err(|err| format!("No se pudo abrir `{}` para hash: {err}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK_SIZE];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("Error al leer `{}` para hash: {err}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!("{:X}", hasher.finalize()))
}

fn detect_archive_format(path: &Path) -> Result<ArchiveFormat, String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .ok_or_else(|| {
            format!(
                "El archivo `{}` no tiene extensión. Se requiere .zip o .rar.",
                path.display()
            )
        })?;

    match extension.as_str() {
        "zip" => Ok(ArchiveFormat::Zip),
        "rar" => Ok(ArchiveFormat::Rar),
        _ => Err(format!(
            "Formato no soportado en `{}`. Solo se permiten .zip o .rar.",
            path.display()
        )),
    }
}

fn infer_archive_extension_from_url(url: &str) -> Option<&'static str> {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment.split('?').next().unwrap_or(without_fragment);
    let extension = Path::new(without_query)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())?;

    match extension.as_str() {
        "zip" => Some("zip"),
        "rar" => Some("rar"),
        _ => None,
    }
}

fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|err| format!("No se pudo inicializar cliente HTTP: {err}"))
}

fn fetch_remote_mods(client: &Client) -> Result<Vec<ClubModEnvelope>, String> {
    let response = client
        .get(MOD_API_URL)
        .send()
        .map_err(|err| format!("No se pudo consultar API remota de mods: {err}"))?
        .error_for_status()
        .map_err(|err| format!("API remota de mods devolvió error HTTP: {err}"))?;

    let payload: ClubModsResponse = response
        .json()
        .map_err(|err| format!("No se pudo parsear respuesta de API remota de mods: {err}"))?;
    Ok(payload.data)
}

fn fetch_recipe_manifest(client: &Client, manifest_url: &str) -> Result<RecipeManifest, String> {
    let response = client
        .get(manifest_url)
        .send()
        .map_err(|err| format!("No se pudieron descargar las instrucciones (`{manifest_url}`): {err}"))?
        .error_for_status()
        .map_err(|err| format!("El servidor devolvió error HTTP (`{manifest_url}`): {err}"))?;

    response
        .json::<RecipeManifest>()
        .map_err(|err| format!("No se pudieron parsear las instrucciones (`{manifest_url}`): {err}"))
}

fn build_supported_mods(manifest: &RecipeManifest, remote_mods: &[ClubModEnvelope]) -> Vec<SupportedMod> {
    let mut mods = remote_mods
        .iter()
        .filter_map(|entry| {
            let slug = entry.resource.slug.trim();
            let download_url = entry.resource.download_pc.trim();
            if slug.is_empty() {
                return None;
            }

            let recipe = manifest.recipes.get(slug)?;
            if !recipe.is_supported {
                return None;
            }
            if recipe.downloadable && download_url.is_empty() {
                return None;
            }

            Some(SupportedMod {
                slug: slug.to_owned(),
                name: entry.resource.name.clone(),
                download_url: if download_url.is_empty() {
                    None
                } else {
                    Some(download_url.to_owned())
                },
                downloadable: recipe.downloadable,
                status: entry.resource.status.clone(),
                current_version: entry
                    .info
                    .as_ref()
                    .and_then(|info| info.updated_at.clone()),
                executable: recipe.executable.clone(),
                description_html: entry.resource.description.clone(),
                hero_image_url: first_image_url(&entry.resource.images, "main"),
                logo_image_url: first_image_url(&entry.resource.images, "logo"),
                screenshot_urls: image_urls(&entry.resource.images, "screenshot"),
                genres: entry
                    .resource
                    .genres
                    .iter()
                    .filter_map(|genre| {
                        let name = genre.name.trim();
                        if name.is_empty() {
                            None
                        } else {
                            Some(name.to_owned())
                        }
                    })
                    .collect(),
                credits: SupportedModCredits {
                    creators: extract_credit_names(&entry.credits.creators),
                    translators: extract_credit_names(&entry.credits.translators),
                    porters: extract_credit_names(&entry.credits.porters),
                },
            })
        })
        .collect::<Vec<_>>();

    mods.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    mods
}

fn first_image_url(images: &[ClubImage], image_type: &str) -> Option<String> {
    images
        .iter()
        .find(|image| image.r#type.eq_ignore_ascii_case(image_type))
        .and_then(|image| {
            let url = image.url.trim();
            if url.is_empty() {
                None
            } else {
                Some(url.to_owned())
            }
        })
}

fn image_urls(images: &[ClubImage], image_type: &str) -> Vec<String> {
    images
        .iter()
        .filter(|image| image.r#type.eq_ignore_ascii_case(image_type))
        .filter_map(|image| {
            let url = image.url.trim();
            if url.is_empty() {
                None
            } else {
                Some(url.to_owned())
            }
        })
        .collect()
}

fn extract_credit_names(entries: &[ClubCreditEntry]) -> Vec<String> {
    let mut names = Vec::new();
    for entry in entries {
        let candidate = entry
            .user
            .as_ref()
            .and_then(|user| user.name.as_ref())
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                entry
                    .name
                    .as_ref()
                    .map(|name| name.trim())
                    .filter(|name| !name.is_empty())
            });

        if let Some(name) = candidate {
            names.push(name.to_owned());
        }
    }
    names
}

fn state_to_view(state: &LauncherState) -> LauncherStateView {
    LauncherStateView {
        manifest_url: state.manifest_url.clone(),
        global_install_dir: state
            .global_install_dir
            .clone()
            .unwrap_or_else(|| default_install_dir().to_string_lossy().into_owned()),
        cached_ddlc_zip_path: state.cached_ddlc_zip_path.clone(),
        oobe_completed: state.oobe_completed,
        installed_mods: state.installed_mods.clone(),
        expected_ddlc_sha256: EXPECTED_DDLC_SHA256.to_owned(),
        manifest_url_hint: DEFAULT_MANIFEST_URL_HINT.to_owned(),
    }
}

fn load_state(app: &AppHandle) -> Result<LauncherState, String> {
    let mut connection = open_state_db(app)?;
    migrate_legacy_state_if_needed(app, &mut connection)?;

    let mut state = LauncherState {
        manifest_url: read_preference(&connection, "manifest_url")?,
        global_install_dir: read_preference(&connection, "global_install_dir")?,
        cached_ddlc_zip_path: read_preference(&connection, "cached_ddlc_zip_path")?,
        oobe_completed: read_bool_preference(&connection, "oobe_completed")?.unwrap_or(false),
        installed_mods: read_installed_mods(&connection)?,
    };

    if state
        .global_install_dir
        .as_ref()
        .map(|path| path.trim().is_empty())
        .unwrap_or(true)
    {
        state.global_install_dir = Some(default_install_dir().to_string_lossy().into_owned());
    }

    if state
        .manifest_url
        .as_ref()
        .map(|url| url.trim().is_empty())
        .unwrap_or(true)
    {
        state.manifest_url = Some(RECIPES_MANIFEST_URL.to_owned());
    }

    if !state.oobe_completed {
        if let Some(base_path) = state.cached_ddlc_zip_path.as_deref() {
            if path_is_file(Path::new(base_path)) {
                state.oobe_completed = true;
            }
        }
    }

    Ok(state)
}

fn save_state(app: &AppHandle, state: &LauncherState) -> Result<(), String> {
    let mut connection = open_state_db(app)?;
    migrate_legacy_state_if_needed(app, &mut connection)?;
    persist_state_in_db(&mut connection, state)
}

fn open_state_db(app: &AppHandle) -> Result<Connection, String> {
    let db_path = state_db_path(app)?;
    if let Some(parent) = db_path.parent() {
        create_dir_all_safe(parent)?;
    }

    let connection = Connection::open(&db_path)
        .map_err(|err| format!("No se pudo abrir base SQLite `{}`: {err}", db_path.display()))?;
    initialize_state_db(&connection)?;
    Ok(connection)
}

fn initialize_state_db(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS preferences (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS installations (
              slug TEXT PRIMARY KEY,
              install_path TEXT NOT NULL,
              current_version TEXT,
              executable_path TEXT NOT NULL,
              installed_at_epoch_ms INTEGER NOT NULL
            );
            ",
        )
        .map_err(|err| format!("No se pudo inicializar SQLite: {err}"))
}

fn migrate_legacy_state_if_needed(app: &AppHandle, connection: &mut Connection) -> Result<(), String> {
    let preference_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM preferences", [], |row| row.get(0))
        .map_err(|err| format!("No se pudo consultar preferencias en la base de datos: {err}"))?;
    let installation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM installations", [], |row| row.get(0))
        .map_err(|err| format!("No se pudo consultar instalaciones en la base de datos: {err}"))?;
    if preference_count > 0 || installation_count > 0 {
        return Ok(());
    }

    let legacy_path = legacy_state_file_path(app)?;
    if !path_exists(&legacy_path) {
        return Ok(());
    }

    let content = fs::read_to_string(fs_path(&legacy_path)).map_err(|err| {
        format!(
            "No se pudo leer estado legacy `{}`: {err}",
            legacy_path.display()
        )
    })?;
    let mut legacy_state: LauncherState = serde_json::from_str(&content)
        .map_err(|err| format!("No se pudo parsear estado debug user_state.json: {err}"))?;
    if legacy_state
        .global_install_dir
        .as_ref()
        .map(|path| path.trim().is_empty())
        .unwrap_or(true)
    {
        legacy_state.global_install_dir = Some(default_install_dir().to_string_lossy().into_owned());
    }

    persist_state_in_db(connection, &legacy_state)?;
    let migrated_path = legacy_path.with_extension("migrated.json");
    if let Err(err) = fs::rename(fs_path(&legacy_path), fs_path(&migrated_path)) {
        debug_log(format!(
            "No se pudo renombrar estado debug `{}` a `{}` tras migración: {err}",
            legacy_path.display(),
            migrated_path.display()
        ));
    } else {
        debug_log(format!(
            "Migración debug -> SQLite completada. source=`{}` migrated=`{}`",
            legacy_path.display(),
            migrated_path.display()
        ));
    }

    Ok(())
}

fn read_preference(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT value FROM preferences WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| format!("No se pudo leer preferencia `{key}`: {err}"))
}

fn read_bool_preference(connection: &Connection, key: &str) -> Result<Option<bool>, String> {
    let raw_value = read_preference(connection, key)?;
    let parsed = raw_value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| match value {
            "1" | "true" | "TRUE" | "True" => Ok(true),
            "0" | "false" | "FALSE" | "False" => Ok(false),
            _ => Err(format!("La preferencia booleana `{key}` contiene valor inválido.")),
        })
        .transpose()?;
    Ok(parsed)
}

fn read_installed_mods(connection: &Connection) -> Result<Vec<InstalledMod>, String> {
    let mut statement = connection
        .prepare(
            "SELECT slug, install_path, current_version, executable_path, installed_at_epoch_ms
             FROM installations
             ORDER BY installed_at_epoch_ms DESC",
        )
        .map_err(|err| format!("No se pudo preparar consulta de instalaciones: {err}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(InstalledMod {
                slug: row.get(0)?,
                install_path: row.get(1)?,
                current_version: row.get(2)?,
                executable_path: row.get(3)?,
                installed_at_epoch_ms: row.get(4)?,
            })
        })
        .map_err(|err| format!("No se pudo ejecutar consulta de instalaciones: {err}"))?;

    let mut installed_mods = Vec::new();
    for row in rows {
        installed_mods.push(
            row.map_err(|err| format!("No se pudo mapear fila de instalación: {err}"))?,
        );
    }
    Ok(installed_mods)
}

fn persist_state_in_db(connection: &mut Connection, state: &LauncherState) -> Result<(), String> {
    let transaction = connection
        .transaction()
        .map_err(|err| format!("No se pudo abrir transacción SQLite: {err}"))?;

    set_preference(&transaction, "manifest_url", state.manifest_url.as_deref())?;
    set_preference(
        &transaction,
        "global_install_dir",
        state.global_install_dir.as_deref(),
    )?;
    set_preference(
        &transaction,
        "cached_ddlc_zip_path",
        state.cached_ddlc_zip_path.as_deref(),
    )?;
    set_preference(
        &transaction,
        "oobe_completed",
        Some(if state.oobe_completed { "1" } else { "0" }),
    )?;

    transaction
        .execute("DELETE FROM installations", [])
        .map_err(|err| format!("No se pudo limpiar instalaciones en SQLite: {err}"))?;
    for installed in &state.installed_mods {
        transaction
            .execute(
                "INSERT INTO installations (slug, install_path, current_version, executable_path, installed_at_epoch_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    installed.slug,
                    installed.install_path,
                    installed.current_version,
                    installed.executable_path,
                    installed.installed_at_epoch_ms as i64
                ],
            )
            .map_err(|err| format!("No se pudo insertar instalación en SQLite: {err}"))?;
    }

    transaction
        .commit()
        .map_err(|err| format!("No se pudo cerrar transacción SQLite: {err}"))
}

fn set_preference(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
    value: Option<&str>,
) -> Result<(), String> {
    if let Some(value) = value {
        transaction
            .execute(
                "INSERT INTO preferences (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|err| format!("No se pudo guardar preferencia `{key}`: {err}"))?;
    } else {
        transaction
            .execute("DELETE FROM preferences WHERE key = ?1", params![key])
            .map_err(|err| format!("No se pudo borrar preferencia `{key}`: {err}"))?;
    }

    Ok(())
}

fn state_db_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join(STATE_DB_FILE_NAME))
}

fn legacy_state_file_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join(LEGACY_STATE_FILE_NAME))
}

fn cache_dir_path(app: &AppHandle) -> Result<PathBuf, String> {
    let cache_dir = app_data_dir(app)?.join(CACHE_DIR_NAME);
    create_dir_all_safe(&cache_dir)?;
    Ok(cache_dir)
}

fn oobe_dir_path(app: &AppHandle) -> Result<PathBuf, String> {
    let oobe_dir = app_data_dir(app)?.join(OOBE_DIR_NAME);
    create_dir_all_safe(&oobe_dir)?;
    Ok(oobe_dir)
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("No se pudo resolver app_data_dir: {err}"))?;
    create_dir_all_safe(&path)?;
    Ok(path)
}

fn to_absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let current_dir = env::current_dir()
            .map_err(|err| format!("No se pudo resolver directorio actual: {err}"))?;
        Ok(current_dir.join(path))
    }
}

fn default_install_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("TraduBee").join("Mods");
        }
    }

    env::temp_dir().join("TraduBee").join("Mods")
}

fn validate_manifest_url(url: &str) -> Result<(), String> {
    reqwest::Url::parse(url)
        .map_err(|err| format!("La URL de las instrucciones no es válida (`{url}`): {err}"))?;
    Ok(())
}

fn resolve_manifest_url(configured_url: Option<&str>) -> String {
    configured_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| RECIPES_MANIFEST_URL.to_owned())
}

fn ensure_install_dir_allowed(path: &Path) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        let raw = path
            .to_string_lossy()
            .replace('/', "\\")
            .to_lowercase()
            .trim_start_matches(r"\\?\")
            .to_owned();

        let disallowed_roots = ["c:\\program files", "c:\\program files (x86)"];
        for root in disallowed_roots {
            if raw == root || raw.starts_with(&format!("{root}\\")) {
                return Err("Por seguridad de UAC, selecciona una ruta fuera de Program Files."
                    .to_owned());
            }
        }
    }

    Ok(())
}

fn ensure_file_exists(path: &Path, label: &str) -> Result<(), String> {
    if !path_is_file(path) {
        return Err(format!("No se encontró {label} en `{}`.", path.display()));
    }
    Ok(())
}

fn upsert_installed_mod(installed_mods: &mut Vec<InstalledMod>, item: InstalledMod) {
    if let Some(existing) = installed_mods.iter_mut().find(|entry| entry.slug == item.slug) {
        *existing = item;
    } else {
        installed_mods.push(item);
    }
}

fn emit_installation_event(app: &AppHandle, slug: &str, status: &str, message: &str) {
    let _ = app.emit(
        "installation-status",
        InstallationEvent {
            slug: slug.to_owned(),
            status: status.to_owned(),
            message: message.to_owned(),
        },
    );
}

fn emit_mod_process_status_event(app: &AppHandle, slug: &str, is_running: bool, pid: Option<u32>) {
    let _ = app.emit(
        "mod-process-status",
        ModProcessStatusEvent {
            slug: slug.to_owned(),
            is_running,
            pid,
        },
    );
}

fn emit_installation_progress_event(
    app: &AppHandle,
    slug: &str,
    progress: u8,
    status: &str,
    state: &str,
    error: Option<String>,
) {
    let _ = app.emit(
        "installation-progress",
        InstallationProgressEvent {
            slug: slug.to_owned(),
            progress: progress.min(100),
            status: status.to_owned(),
            state: state.to_owned(),
            error,
        },
    );
}

fn debug_log(message: impl AsRef<str>) {
    eprintln!("[tradu-bee][debug] {}", message.as_ref());
}

fn now_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn sanitize_slug_for_filename(slug: &str) -> String {
    slug.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn create_dir_all_safe(path: &Path) -> Result<(), String> {
    fs::create_dir_all(fs_path(path))
        .map_err(|err| format!("No se pudo crear directorio `{}`: {err}", path.display()))
}

fn remove_dir_all_safe(path: &Path) -> Result<(), String> {
    fs::remove_dir_all(fs_path(path))
        .map_err(|err| format!("No se pudo eliminar directorio `{}`: {err}", path.display()))
}

fn cleanup_failed_installation_target(target_dir: &Path) {
    if !path_exists(target_dir) {
        return;
    }

    if cfg!(debug_assertions) {
        debug_log(format!(
            "Debug mode activo: se preserva instalación fallida en `{}`",
            target_dir.display()
        ));
        return;
    }

    if let Err(err) = remove_dir_all_safe(target_dir) {
        debug_log(format!(
            "No se pudo limpiar instalación fallida `{}`: {err}",
            target_dir.display()
        ));
    }
}

fn debug_preserve_note(target_dir: &Path) -> String {
    if cfg!(debug_assertions) {
        format!(
            " [debug] Se conservaron archivos en `{}`.",
            target_dir.display()
        )
    } else {
        String::new()
    }
}

fn remove_file_safe(path: &Path) -> Result<(), String> {
    fs::remove_file(fs_path(path))
        .map_err(|err| format!("No se pudo eliminar archivo `{}`: {err}", path.display()))
}

fn path_exists(path: &Path) -> bool {
    fs::metadata(fs_path(path)).is_ok()
}

fn path_is_file(path: &Path) -> bool {
    fs::metadata(fs_path(path))
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn fs_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let normalized = path.to_string_lossy().replace('/', "\\");
        if normalized.starts_with(r"\\?\") {
            return PathBuf::from(normalized);
        }
        if normalized.starts_with(r"\\") {
            return PathBuf::from(format!(
                r"\\?\UNC\{}",
                normalized.trim_start_matches(r"\\")
            ));
        }
        if path.is_absolute() {
            return PathBuf::from(format!(r"\\?\{normalized}"));
        }
    }

    path.to_path_buf()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(LauncherRuntimeState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_launcher_state,
            update_launcher_config,
            validate_vanilla_zip,
            finalize_oobe_setup,
            fetch_supported_mods,
            execute_installation_recipe,
            uninstall_mod,
            launch_installed_mod,
            get_running_mod_processes
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
