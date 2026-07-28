use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::extractor::extract_archive_here;
use crate::utils::{
    copy_file_overwrite, create_dir_all_safe, debug_log, fs_path, path_exists, path_is_file,
    remove_dir_all_safe, remove_file_safe,
};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RecipeStep {
    pub action: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModRecipe {
    pub is_supported: bool,
    pub downloadable: bool,
    pub executable: String,
    pub steps: Vec<RecipeStep>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RecipeManifest {
    pub manifest_version: String,
    pub recipes: std::collections::HashMap<String, ModRecipe>,
}

pub fn run_recipe_steps(
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
            "Step {}/{}: {}",
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
            _ => {
                return Err(format!(
                    "Acción de instrucción no soportada: `{}`",
                    step.action
                ))
            }
        }
    }
    Ok(())
}

pub fn recipe_action_label(action: &str) -> &'static str {
    match action {
        "extract_base" => "Extracting base game",
        "extract_mod" => "Extracting mod",
        "copy_overwrite" => "Installing translation files",
        "delete_file" => "Removing conflicting files",
        "cleanup_temp" => "Cleaning up temporary files",
        _ => "Executing action",
    }
}

pub fn resolve_recipe_path(root: &Path, recipe_relative_path: &str) -> Result<PathBuf, String> {
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

pub fn resolve_copy_source_path(source: &Path) -> Result<PathBuf, String> {
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

    let entries = fs::read_dir(fs_path(parent)).map_err(|err| {
        format!(
            "No se pudo leer `{}` para inferencia: {err}",
            parent.display()
        )
    })?;
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
        if let Some(file_name) = source.file_name() {
            let path_with_suffix = inferred.join(file_name);
            if path_exists(&path_with_suffix) {
                debug_log(format!(
                    "copy_overwrite source missing. requested=`{}` inferred_with_suffix=`{}`",
                    source.display(),
                    path_with_suffix.display()
                ));
                return Ok(path_with_suffix);
            }
        }
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

pub fn recursive_copy(source: &Path, destination: &Path) -> Result<(), String> {
    let source_meta = fs::metadata(fs_path(source))
        .map_err(|err| format!("No se pudo acceder a `{}`: {err}", source.display()))?;

    if source_meta.is_file() {
        copy_file_overwrite(source, destination)?;
        return Ok(());
    }

    create_dir_all_safe(destination)?;
    let entries = fs::read_dir(fs_path(source)).map_err(|err| {
        format!(
            "No se pudo leer el directorio `{}`: {err}",
            source.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "No se pudo leer entrada en copia recursiva de `{}`: {err}",
                source.display()
            )
        })?;
        let entry_path = entry.path();
        let relative_path = entry_path
            .strip_prefix(fs_path(source))
            .map_err(|err| format!("No se pudo calcular ruta relativa: {err}"))?;
        let target_path = destination.join(relative_path);

        recursive_copy(&entry_path, &target_path)?;
    }

    Ok(())
}
