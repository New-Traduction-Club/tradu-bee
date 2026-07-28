use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Manager};

use crate::utils::{create_dir_all_safe, debug_log, fs_path, path_exists, path_is_file};

pub const EXPECTED_DDLC_SHA256: &str = "2A3DD7969A06729A32ACE0A6ECE5F2327E29BDF460B8B39E6A8B0875E545632E";
pub const STATE_DB_FILE_NAME: &str = "launcher_state.db";
pub const LEGACY_STATE_FILE_NAME: &str = "user_state.json";
pub const CACHE_DIR_NAME: &str = "cache";
pub const OOBE_DIR_NAME: &str = "oobe";
pub const RECIPES_MANIFEST_URL: &str = "https://raw.githubusercontent.com/Just3090/random_shit/refs/heads/main/random.json";
pub const DEFAULT_MANIFEST_URL_HINT: &str = RECIPES_MANIFEST_URL;
pub const MOD_API_URL: &str = "https://api-new.dokidokispanish.club/mod/all";
pub const OOBE_ORIGINAL_ARCHIVE_NAME: &str = "ddlc-original.zip";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct LauncherState {
    pub manifest_url: Option<String>,
    pub global_install_dir: Option<String>,
    pub cached_ddlc_zip_path: Option<String>,
    pub oobe_completed: bool,
    pub installed_mods: Vec<InstalledMod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledMod {
    pub slug: String,
    pub install_path: String,
    pub current_version: Option<String>,
    pub executable_path: String,
    pub installed_at_epoch_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherStateView {
    pub manifest_url: Option<String>,
    pub global_install_dir: String,
    pub cached_ddlc_zip_path: Option<String>,
    pub oobe_completed: bool,
    pub installed_mods: Vec<InstalledMod>,
    pub expected_ddlc_sha256: String,
    pub manifest_url_hint: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLauncherConfigRequest {
    pub manifest_url: Option<String>,
    pub global_install_dir: Option<String>,
    pub cached_ddlc_zip_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseZipValidation {
    pub path: String,
    pub computed_sha256: String,
    pub expected_sha256: String,
    pub is_valid: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedMod {
    pub slug: String,
    pub name: String,
    pub download_url: Option<String>,
    pub downloadable: bool,
    pub status: String,
    pub current_version: Option<String>,
    pub executable: String,
    pub description_html: String,
    pub hero_image_url: Option<String>,
    pub logo_image_url: Option<String>,
    pub screenshot_urls: Vec<String>,
    pub genres: Vec<String>,
    pub credits: SupportedModCredits,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedModCredits {
    pub creators: Vec<String>,
    pub translators: Vec<String>,
    pub porters: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub slug: String,
    pub install_path: String,
    pub executable_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationEvent {
    pub slug: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationProgressEvent {
    pub slug: String,
    pub progress: u8,
    pub status: String,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModProcessStatusEvent {
    pub slug: String,
    pub is_running: bool,
    pub pid: Option<u32>,
}

#[derive(Clone, Default)]
pub struct LauncherRuntimeState {
    pub running_processes: Arc<Mutex<HashMap<String, u32>>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClubModsResponse {
    pub data: Vec<ClubModEnvelope>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClubModEnvelope {
    pub resource: ClubModResource,
    #[serde(default)]
    pub info: Option<ClubModInfo>,
    #[serde(default)]
    pub credits: ClubCredits,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClubModResource {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub download_pc: String,
    #[serde(default)]
    pub images: Vec<ClubImage>,
    #[serde(default)]
    pub genres: Vec<ClubGenre>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClubModInfo {
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClubImage {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub r#type: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClubGenre {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct ClubCredits {
    #[serde(default)]
    pub creators: Vec<ClubCreditEntry>,
    #[serde(default)]
    pub translators: Vec<ClubCreditEntry>,
    #[serde(default)]
    pub porters: Vec<ClubCreditEntry>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClubCreditEntry {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub user: Option<ClubCreditUser>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClubCreditUser {
    #[serde(default)]
    pub name: Option<String>,
}

pub fn first_image_url(images: &[ClubImage], image_type: &str) -> Option<String> {
    images
        .iter()
        .find(|img| img.r#type == image_type)
        .map(|img| img.url.clone())
}

pub fn image_urls(images: &[ClubImage], image_type: &str) -> Vec<String> {
    images
        .iter()
        .filter(|img| img.r#type == image_type)
        .map(|img| img.url.clone())
        .collect()
}

pub fn extract_credit_names(entries: &[ClubCreditEntry]) -> Vec<String> {
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

pub fn state_to_view(state: &LauncherState) -> LauncherStateView {
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

pub fn load_state(app: &AppHandle) -> Result<LauncherState, String> {
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

pub fn save_state(app: &AppHandle, state: &LauncherState) -> Result<(), String> {
    let mut connection = open_state_db(app)?;
    migrate_legacy_state_if_needed(app, &mut connection)?;
    persist_state_in_db(&mut connection, state)
}

pub fn open_state_db(app: &AppHandle) -> Result<Connection, String> {
    let db_path = state_db_path(app)?;
    if let Some(parent) = db_path.parent() {
        create_dir_all_safe(parent)?;
    }

    let connection = Connection::open(&db_path).map_err(|err| {
        format!(
            "No se pudo abrir base SQLite `{}`: {err}",
            db_path.display()
        )
    })?;
    initialize_state_db(&connection)?;
    Ok(connection)
}

pub fn initialize_state_db(connection: &Connection) -> Result<(), String> {
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

pub fn migrate_legacy_state_if_needed(
    app: &AppHandle,
    connection: &mut Connection,
) -> Result<(), String> {
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

    let content = match fs::read_to_string(fs_path(&legacy_path)) {
        Ok(c) => c,
        Err(err) => {
            debug_log(format!(
                "No se pudo leer estado legacy `{}`: {err}",
                legacy_path.display()
            ));
            let corrupted_path = legacy_path.with_extension("corrupted");
            let _ = fs::rename(fs_path(&legacy_path), fs_path(&corrupted_path));
            return Ok(());
        }
    };

    let mut legacy_state: LauncherState = match serde_json::from_str(&content) {
        Ok(state) => state,
        Err(err) => {
            debug_log(format!(
                "No se pudo parsear estado `{}` {err}",
                legacy_path.display()
            ));
            let corrupted_path = legacy_path.with_extension("corrupted");
            let _ = fs::rename(fs_path(&legacy_path), fs_path(&corrupted_path));
            return Ok(());
        }
    };

    if legacy_state
        .global_install_dir
        .as_ref()
        .map(|path| path.trim().is_empty())
        .unwrap_or(true)
    {
        legacy_state.global_install_dir =
            Some(default_install_dir().to_string_lossy().into_owned());
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

pub fn read_preference(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM preferences WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| format!("Error al leer preferencia `{key}` en SQLite: {err}"))?;
    Ok(value)
}

pub fn read_bool_preference(connection: &Connection, key: &str) -> Result<Option<bool>, String> {
    let raw = read_preference(connection, key)?;
    let parsed = raw
        .as_deref()
        .map(|value| match value {
            "1" | "true" | "TRUE" | "True" => Ok(true),
            "0" | "false" | "FALSE" | "False" => Ok(false),
            _ => Err(format!(
                "La preferencia booleana `{key}` contiene valor inválido."
            )),
        })
        .transpose()?;
    Ok(parsed)
}

pub fn read_installed_mods(connection: &Connection) -> Result<Vec<InstalledMod>, String> {
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
        installed_mods
            .push(row.map_err(|err| format!("No se pudo mapear fila de instalación: {err}"))?);
    }
    Ok(installed_mods)
}

pub fn persist_state_in_db(connection: &mut Connection, state: &LauncherState) -> Result<(), String> {
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

pub fn set_preference(
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

pub fn state_db_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join(STATE_DB_FILE_NAME))
}

pub fn legacy_state_file_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join(LEGACY_STATE_FILE_NAME))
}

pub fn cache_dir_path(app: &AppHandle) -> Result<PathBuf, String> {
    let cache_dir = app_data_dir(app)?.join(CACHE_DIR_NAME);
    create_dir_all_safe(&cache_dir)?;
    Ok(cache_dir)
}

pub fn oobe_dir_path(app: &AppHandle) -> Result<PathBuf, String> {
    let oobe_dir = app_data_dir(app)?.join(OOBE_DIR_NAME);
    create_dir_all_safe(&oobe_dir)?;
    Ok(oobe_dir)
}

pub fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("No se pudo resolver app_data_dir: {err}"))?;
    create_dir_all_safe(&path)?;
    Ok(path)
}

pub fn default_install_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("TraduBee").join("Mods");
        }
    }

    env::temp_dir().join("TraduBee").join("Mods")
}

pub fn upsert_installed_mod(installed_mods: &mut Vec<InstalledMod>, item: InstalledMod) {
    if let Some(existing) = installed_mods
        .iter_mut()
        .find(|entry| entry.slug == item.slug)
    {
        *existing = item;
    } else {
        installed_mods.push(item);
    }
}

pub fn emit_installation_event(app: &AppHandle, slug: &str, status: &str, message: &str) {
    let _ = app.emit(
        "installation-status",
        InstallationEvent {
            slug: slug.to_owned(),
            status: status.to_owned(),
            message: message.to_owned(),
        },
    );
}

pub fn emit_mod_process_status_event(app: &AppHandle, slug: &str, is_running: bool, pid: Option<u32>) {
    let _ = app.emit(
        "mod-process-status",
        ModProcessStatusEvent {
            slug: slug.to_owned(),
            is_running,
            pid,
        },
    );
}

pub fn emit_installation_progress_event(
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

pub fn validate_manifest_url(url: &str) -> Result<(), String> {
    reqwest::Url::parse(url)
        .map_err(|err| format!("La URL de las instrucciones no es válida (`{url}`): {err}"))?;
    Ok(())
}

pub fn resolve_manifest_url(configured_url: Option<&str>) -> String {
    configured_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| RECIPES_MANIFEST_URL.to_owned())
}

pub fn ensure_install_dir_allowed(path: &Path) -> Result<(), String> {
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
                return Err(
                    "Por seguridad de UAC, selecciona una ruta fuera de Program Files.".to_owned(),
                );
            }
        }
    }

    Ok(())
}
