use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};
use tauri::AppHandle;

use crate::state::{emit_mod_process_status_event, LauncherRuntimeState};

pub fn query_running_executable_paths() -> Result<HashSet<String>, String> {
    #[cfg(target_os = "windows")]
    {
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        let mut running_paths = HashSet::new();
        for process in sys.processes().values() {
            if let Some(exe) = process.exe() {
                running_paths.insert(normalize_process_path(exe));
            }
        }

        Ok(running_paths)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(HashSet::new())
    }
}

pub fn normalize_process_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_lowercase()
        .trim_start_matches(r"\\?\")
        .to_owned()
}

pub fn is_process_in_directory(process_path: &str, normalized_dir_path: &str) -> bool {
    let mut dir_prefix = normalized_dir_path.to_owned();
    if !dir_prefix.ends_with('\\') {
        dir_prefix.push('\\');
    }
    process_path.starts_with(&dir_prefix)
}

pub fn sanitize_install_slug(slug: String) -> Result<String, String> {
    let sanitized_slug = slug.trim().to_owned();
    if sanitized_slug.is_empty() {
        return Err("La ruta del mod no puede estar vacía.".to_owned());
    }
    if sanitized_slug.contains('/')
        || sanitized_slug.contains('\\')
        || sanitized_slug.contains("..")
    {
        return Err("La ruta contiene caracteres no válidos para una ruta local.".to_owned());
    }

    Ok(sanitized_slug)
}

pub fn spawn_mod_watcher_thread(
    app: AppHandle,
    runtime: Arc<LauncherRuntimeState>,
    slug: String,
    normalized_install_path: String,
) {
    std::thread::spawn(move || {
        let mut started = false;
        for _ in 0..20 {
            let running_paths = query_running_executable_paths();
            let is_running = running_paths
                .as_ref()
                .map(|paths| {
                    paths
                        .iter()
                        .any(|path| is_process_in_directory(path, &normalized_install_path))
                })
                .unwrap_or(false);
            if is_running {
                started = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(350));
        }

        if !started {
            if let Ok(mut running) = runtime.running_processes.lock() {
                running.remove(&slug);
            }
            emit_mod_process_status_event(&app, &slug, false, None);
            return;
        }

        let mut missing_checks = 0u8;
        loop {
            let is_running = query_running_executable_paths()
                .map(|paths| {
                    paths
                        .iter()
                        .any(|path| is_process_in_directory(path, &normalized_install_path))
                })
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

        if let Ok(mut running) = runtime.running_processes.lock() {
            running.remove(&slug);
        }
        emit_mod_process_status_event(&app, &slug, false, None);
    });
}
