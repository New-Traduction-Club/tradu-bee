use std::{
    fs::File,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};
use unrar::Archive as RarArchive;
use zip::ZipArchive;

use crate::utils::{create_dir_all_safe, debug_log, ensure_file_exists, fs_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    Rar,
}

pub fn detect_archive_format(path: &Path) -> Result<ArchiveFormat, String> {
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

pub fn sanitize_archive_entry_path(entry_path: &Path) -> Result<PathBuf, String> {
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

pub fn extract_archive_here(archive_path: &Path, destination: &Path) -> Result<(), String> {
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

pub fn extract_rar_archive(archive_path: &Path, destination: &Path) -> Result<(), String> {
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
            "No se pudo leer cabecera RAR `{}`: {err}",
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

pub fn extract_zip_archive(zip_path: &Path, destination: &Path) -> Result<(), String> {
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

pub fn infer_archive_extension_from_url(url: &str) -> Option<&'static str> {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
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
