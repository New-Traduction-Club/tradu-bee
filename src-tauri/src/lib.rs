mod commands;
mod downloader;
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
            commands::cancel_installation,
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

    #[test]
    fn test_resolve_mod_extraction_root() {
        let temp_dir =
            std::env::temp_dir().join(format!("tradu_bee_ext_test_{}", now_epoch_millis()));
        let _ = fs::create_dir_all(&temp_dir);

        let assert_paths_equal = |left: PathBuf, right: PathBuf| {
            let clean_left = left
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .replace('/', "\\");
            let clean_right = right
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .replace('/', "\\");
            assert_eq!(clean_left.to_lowercase(), clean_right.to_lowercase());
        };

        // Case Flat layout, contains files directly in root
        let flat_root = temp_dir.join("flat");
        let _ = fs::create_dir_all(&flat_root);
        let _ = fs::File::create(flat_root.join("scripts.rpy"));
        let _ = fs::create_dir_all(flat_root.join("game"));
        assert_paths_equal(
            super::recipes::resolve_mod_extraction_root(&flat_root).unwrap(),
            flat_root,
        );

        // Case Nested layout, contains 0 files and 1 directory
        let nested_root = temp_dir.join("nested");
        let _ = fs::create_dir_all(&nested_root);
        let inner_folder = nested_root.join("MyAwesomeMod-1.0");
        let _ = fs::create_dir_all(&inner_folder);
        let _ = fs::File::create(inner_folder.join("options.rpy"));
        assert_paths_equal(
            super::recipes::resolve_mod_extraction_root(&nested_root).unwrap(),
            inner_folder,
        );

        // Case Double nested layout
        let dbl_nested_root = temp_dir.join("dbl_nested");
        let _ = fs::create_dir_all(&dbl_nested_root);
        let layer1 = dbl_nested_root.join("Archive");
        let layer2 = layer1.join("ModFiles");
        let _ = fs::create_dir_all(&layer2);
        let _ = fs::File::create(layer2.join("scripts.rpa"));
        assert_paths_equal(
            super::recipes::resolve_mod_extraction_root(&dbl_nested_root).unwrap(),
            layer2,
        );

        // Case Structural renpy folders, stops walk
        let struct_root = temp_dir.join("struct_test");
        let _ = fs::create_dir_all(&struct_root);
        let game_folder = struct_root.join("game");
        let _ = fs::create_dir_all(&game_folder);
        let _ = fs::File::create(game_folder.join("scripts.rpa"));
        assert_paths_equal(
            super::recipes::resolve_mod_extraction_root(&struct_root).unwrap(),
            struct_root,
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_detect_installed_executable() {
        let temp_dir =
            std::env::temp_dir().join(format!("tradu_bee_exe_test_{}", now_epoch_millis()));
        let _ = fs::create_dir_all(&temp_dir);

        // Standard DDLC and python exes
        let _ = fs::File::create(temp_dir.join("DDLC.exe"));
        let _ = fs::File::create(temp_dir.join("python.exe"));
        let _ = fs::File::create(temp_dir.join("pythonw.exe"));

        // With no custom exes, falls back to DDLC.exe
        assert_eq!(
            super::recipes::detect_installed_executable(&temp_dir).unwrap(),
            "DDLC.exe"
        );

        // With custom exes, prioritizes non-32 ones
        let _ = fs::File::create(temp_dir.join("LaunchMod-32.exe"));
        let _ = fs::File::create(temp_dir.join("LaunchMod.exe"));
        assert_eq!(
            super::recipes::detect_installed_executable(&temp_dir).unwrap(),
            "LaunchMod.exe"
        );

        // Fallback to any custom, even 32-bit if no 64-bit exists
        let temp_dir2 =
            std::env::temp_dir().join(format!("tradu_bee_exe_test2_{}", now_epoch_millis()));
        let _ = fs::create_dir_all(&temp_dir2);
        let _ = fs::File::create(temp_dir2.join("DDLC.exe"));
        let _ = fs::File::create(temp_dir2.join("ModLauncher-32.exe"));
        assert_eq!(
            super::recipes::detect_installed_executable(&temp_dir2).unwrap(),
            "ModLauncher-32.exe"
        );

        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_dir_all(&temp_dir2);
    }
}
