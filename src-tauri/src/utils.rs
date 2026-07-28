use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const HASH_CHUNK_SIZE: usize = 1024 * 1024;

pub fn debug_log(message: impl AsRef<str>) {
    eprintln!("[tradu-bee][debug] {}", message.as_ref());
}

pub fn now_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

pub fn to_absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let current_dir = env::current_dir()
            .map_err(|err| format!("No se pudo resolver directorio actual: {err}"))?;
        Ok(current_dir.join(path))
    }
}

pub fn path_exists(path: &Path) -> bool {
    fs::metadata(fs_path(path)).is_ok()
}

pub fn path_is_file(path: &Path) -> bool {
    fs::metadata(fs_path(path))
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

pub fn fs_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let normalized = path.to_string_lossy().replace('/', "\\");
        if normalized.starts_with(r"\\?\") {
            return PathBuf::from(normalized);
        }
        if normalized.starts_with(r"\\") {
            return PathBuf::from(format!(r"\\?\UNC\{}", normalized.trim_start_matches(r"\\")));
        }
        if path.is_absolute() {
            return PathBuf::from(format!(r"\\?\{normalized}"));
        }
    }
    path.to_path_buf()
}

pub fn create_dir_all_safe(path: &Path) -> Result<(), String> {
    fs::create_dir_all(fs_path(path))
        .map_err(|err| format!("No se pudo crear directorio `{}`: {err}", path.display()))
}

pub fn remove_dir_all_safe(path: &Path) -> Result<(), String> {
    fs::remove_dir_all(fs_path(path))
        .map_err(|err| format!("No se pudo eliminar directorio `{}`: {err}", path.display()))
}

pub fn remove_file_safe(path: &Path) -> Result<(), String> {
    fs::remove_file(fs_path(path))
        .map_err(|err| format!("No se pudo eliminar archivo `{}`: {err}", path.display()))
}

pub fn copy_file_overwrite(source: &Path, destination: &Path) -> Result<(), String> {
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

pub fn copy_file_secure(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        create_dir_all_safe(parent)?;
    }

    let tmp_destination = destination.with_extension(format!("tmp-{}", now_epoch_millis()));
    fs::copy(fs_path(source), fs_path(&tmp_destination)).map_err(|err| {
        format!(
            "No se pudo realizar copia temporal segura de `{}` hacia `{}`: {err}",
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

pub fn compute_sha256_chunked(path: &Path) -> Result<String, String> {
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

pub fn ensure_file_exists(path: &Path, label: &str) -> Result<(), String> {
    if !path_is_file(path) {
        return Err(format!("No se encontró {label} en `{}`.", path.display()));
    }
    Ok(())
}

pub fn debug_preserve_note(target_dir: &Path) -> String {
    if cfg!(debug_assertions) {
        format!(
            " [debug] Se conservaron archivos en `{}`.",
            target_dir.display()
        )
    } else {
        String::new()
    }
}

pub fn cleanup_failed_installation_target(target_dir: &Path) {
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

pub fn sanitize_slug_for_filename(slug: &str) -> String {
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
