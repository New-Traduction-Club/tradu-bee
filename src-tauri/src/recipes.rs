use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::extractor::extract_archive_here;
use crate::utils::{
    copy_file_overwrite, create_dir_all_safe, debug_log, fs_path, path_exists, remove_dir_all_safe,
};

pub fn install_mod_files_generic(
    target_dir: &Path,
    vanilla_zip: &Path,
    mod_zip: &Path,
    is_cancelled: &dyn Fn() -> bool,
    mut report_progress: impl FnMut(u8, &str),
) -> Result<String, String> {
    if is_cancelled() {
        return Err("Instalación cancelada.".to_owned());
    }

    report_progress(50, "Extrayendo DDLC...");
    let temp_ddlc = target_dir.join("temp_ddlc");
    if path_exists(&temp_ddlc) {
        remove_dir_all_safe(&temp_ddlc)?;
    }
    create_dir_all_safe(&temp_ddlc)?;
    extract_archive_here(vanilla_zip, &temp_ddlc)?;

    if is_cancelled() {
        let _ = remove_dir_all_safe(&temp_ddlc);
        return Err("Instalación cancelada.".to_owned());
    }

    report_progress(70, "Extrayendo el mod...");
    let temp_mod = target_dir.join("temp_mod");
    if path_exists(&temp_mod) {
        remove_dir_all_safe(&temp_mod)?;
    }
    create_dir_all_safe(&temp_mod)?;
    extract_archive_here(mod_zip, &temp_mod)?;

    if is_cancelled() {
        let _ = remove_dir_all_safe(&temp_ddlc);
        let _ = remove_dir_all_safe(&temp_mod);
        return Err("Instalación cancelada.".to_owned());
    }

    report_progress(80, "Copiando archivos...");
    let base_src = resolve_copy_source_path(&temp_ddlc.join("game"))?;
    let base_root = base_src
        .parent()
        .ok_or_else(|| "Error al bsucar la carpeta de origen.".to_owned())?;
    recursive_copy(base_root, target_dir)?;

    if is_cancelled() {
        let _ = remove_dir_all_safe(&temp_ddlc);
        let _ = remove_dir_all_safe(&temp_mod);
        return Err("Instalación cancelada.".to_owned());
    }

    report_progress(85, "Copiando archivos del mod...");
    let mod_src_root = resolve_mod_extraction_root(&temp_mod)?;
    debug_log(format!(
        "Ruta del mod encontrada: {}",
        mod_src_root.display()
    ));

    let has_game_dir =
        path_exists(&mod_src_root.join("game")) || path_exists(&mod_src_root.join("Game"));
    if has_game_dir {
        recursive_copy(&mod_src_root, target_dir)?;
    } else {
        let target_game_dir = target_dir.join("game");
        create_dir_all_safe(&target_game_dir)?;
        recursive_copy(&mod_src_root, &target_game_dir)?;
    }

    report_progress(92, "Limpiando archivos temporales...");
    let _ = remove_dir_all_safe(&temp_ddlc);
    let _ = remove_dir_all_safe(&temp_mod);

    if is_cancelled() {
        return Err("Instalación cancelada.".to_owned());
    }

    report_progress(96, "Buscando el ejecutable...");
    let exe_name = detect_installed_executable(target_dir)?;
    debug_log(format!("Ejecutable del mod: {exe_name}"));

    Ok(exe_name)
}

pub fn resolve_mod_extraction_root(extracted_dir: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(fs_path(extracted_dir)).map_err(|err| {
        format!(
            "Error al leer la carpeta `{}`: {err}",
            extracted_dir.display()
        )
    })?;

    let mut files = Vec::new();
    let mut dirs = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|err| format!("Error {err}"))?;
        let path = entry.path();
        if path.is_file() {
            files.push(path);
        } else if path.is_dir() {
            dirs.push(path);
        }
    }

    if files.is_empty() && dirs.len() == 1 {
        let dir_name = dirs[0]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        if dir_name != "game"
            && dir_name != "lib"
            && dir_name != "renpy"
            && dir_name != "characters"
        {
            return resolve_mod_extraction_root(&dirs[0]);
        }
    }

    Ok(extracted_dir.to_path_buf())
}

pub fn detect_installed_executable(target_dir: &Path) -> Result<String, String> {
    let entries = std::fs::read_dir(fs_path(target_dir))
        .map_err(|err| format!("Error al leer la carpeta: {err}"))?;

    let mut exe_files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|err| format!("Error: {err}"))?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext.to_ascii_lowercase() == "exe" {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        let name_lower = name.to_lowercase();
                        if name_lower != "python.exe" && name_lower != "pythonw.exe" {
                            exe_files.push(name.to_owned());
                        }
                    }
                }
            }
        }
    }

    let custom_64_exes: Vec<&String> = exe_files
        .iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            lower != "ddlc.exe" && !lower.ends_with("-32.exe") && !lower.ends_with("32.exe")
        })
        .collect();

    if let Some(&exe_name) = custom_64_exes.first() {
        return Ok(exe_name.clone());
    }

    let custom_any_exes: Vec<&String> = exe_files
        .iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            lower != "ddlc.exe"
        })
        .collect();

    if let Some(&exe_name) = custom_any_exes.first() {
        return Ok(exe_name.clone());
    }

    Ok("DDLC.exe".to_owned())
}

#[allow(dead_code)]
pub fn resolve_recipe_path(root: &Path, recipe_relative_path: &str) -> Result<PathBuf, String> {
    let trimmed = recipe_relative_path.trim();
    if trimmed.is_empty() {
        return Ok(root.to_path_buf());
    }

    let recipe_path = Path::new(trimmed);
    if recipe_path.is_absolute() {
        return Err(format!("Error: `{trimmed}`"));
    }

    let mut normalized = PathBuf::new();
    for component in recipe_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("Error: `{trimmed}`"));
            }
        }
    }

    Ok(root.join(normalized))
}

pub fn resolve_copy_source_path(source: &Path) -> Result<PathBuf, String> {
    if path_exists(source) {
        return Ok(source.to_path_buf());
    }

    let parent = source
        .parent()
        .ok_or_else(|| format!("Error al resolver la ruta: `{}`", source.display()))?;
    if !path_exists(parent) {
        return Err(format!("La carepta `{}` no existe.", parent.display()));
    }

    let entries = fs::read_dir(fs_path(parent))
        .map_err(|err| format!("Error al leer `{}`: {err}", parent.display()))?;
    let mut available_dirs = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|err| format!("Error al leer: {err}"))?;
        let file_type = entry
            .file_type()
            .map_err(|err| format!("Error al leer: {err}"))?;
        if file_type.is_dir() {
            available_dirs.push(parent.join(entry.file_name()));
        }
    }

    if available_dirs.len() == 1 {
        let inferred = available_dirs[0].clone();
        if let Some(file_name) = source.file_name() {
            let path_with_suffix = inferred.join(file_name);
            if path_exists(&path_with_suffix) {
                return Ok(path_with_suffix);
            }
        }
        return Ok(inferred);
    }

    let available = if available_dirs.is_empty() {
        "(none)".to_owned()
    } else {
        available_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(format!(
        "Error al buscar carpeta origen. Subdcarpetas: {available}"
    ))
}

pub fn recursive_copy(source: &Path, destination: &Path) -> Result<(), String> {
    let source_meta = fs::metadata(fs_path(source))
        .map_err(|err| format!("Error `{}`: {err}", source.display()))?;

    if source_meta.is_file() {
        copy_file_overwrite(source, destination)?;
        return Ok(());
    }

    create_dir_all_safe(destination)?;
    let entries = fs::read_dir(fs_path(source))
        .map_err(|err| format!("Error `{}`: {err}", source.display()))?;

    for entry in entries {
        let entry = entry.map_err(|err| format!("Error al copiar: {err}"))?;
        let entry_path = entry.path();
        let relative_path = entry_path
            .strip_prefix(fs_path(source))
            .map_err(|err| format!("Error al buscar ruta relativa: {err}"))?;
        let target_path = destination.join(relative_path);

        recursive_copy(&entry_path, &target_path)?;
    }

    Ok(())
}
