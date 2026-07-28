mod commands;
mod extractor;
mod process;
mod recipes;
mod state;
mod utils;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(state::LauncherRuntimeState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_launcher_state,
            commands::update_launcher_config,
            commands::validate_vanilla_zip,
            commands::finalize_oobe_setup,
            commands::fetch_supported_mods,
            commands::execute_installation_recipe,
            commands::uninstall_mod,
            commands::launch_installed_mod,
            commands::get_running_mod_processes
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::extractor::sanitize_archive_entry_path;
    use super::recipes::resolve_copy_source_path;
    use super::recipes::resolve_recipe_path;
    use super::utils::now_epoch_millis;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_resolve_recipe_path() {
        let root = Path::new("C:\\games\\ddlc");
        assert_eq!(
            resolve_recipe_path(root, "game/scripts.rpy").unwrap(),
            PathBuf::from("C:\\games\\ddlc\\game\\scripts.rpy")
        );
        assert_eq!(
            resolve_recipe_path(root, "./game/scripts.rpy").unwrap(),
            PathBuf::from("C:\\games\\ddlc\\game\\scripts.rpy")
        );
        assert!(resolve_recipe_path(root, "../game").is_err());
        assert!(resolve_recipe_path(root, "/absolute/path").is_err());
    }

    #[test]
    fn test_sanitize_archive_entry_path() {
        assert_eq!(
            sanitize_archive_entry_path(Path::new("game/script.rpyc")).unwrap(),
            PathBuf::from("game/script.rpyc")
        );
        assert!(sanitize_archive_entry_path(Path::new("../script.rpyc")).is_err());
        assert!(sanitize_archive_entry_path(Path::new("/script.rpyc")).is_err());
        assert!(sanitize_archive_entry_path(Path::new("")).is_err());
    }

    #[test]
    fn test_resolve_copy_source_path() {
        let mut temp_root = std::env::temp_dir();
        temp_root.push(format!("tradu_bee_test_{}", now_epoch_millis()));
        let _ = fs::create_dir_all(&temp_root);

        // Case 1: Source exists directly
        let case1_root = temp_root.join("case1");
        let source_direct = case1_root.join("direct_folder");
        let _ = fs::create_dir_all(&source_direct);
        assert_eq!(
            resolve_copy_source_path(&source_direct).unwrap(),
            source_direct
        );

        // Case 2: Source does not exist, but there's a nested folder with suffix
        let case2_root = temp_root.join("case2");
        let _ = fs::create_dir_all(&case2_root);
        let requested_source = case2_root.join("game");
        let nested_mod_dir = case2_root.join("DokiDokiMod-1.0");
        let actual_game_dir = nested_mod_dir.join("game");
        let _ = fs::create_dir_all(&actual_game_dir);

        assert_eq!(
            resolve_copy_source_path(&requested_source).unwrap(),
            actual_game_dir
        );

        let _ = fs::remove_dir_all(&temp_root);
    }
}
