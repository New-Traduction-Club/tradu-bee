use reqwest::blocking::Client;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::extractor::ArchiveFormat;
use crate::utils::{create_dir_all_safe, debug_log, fs_path, path_exists, remove_file_safe};

pub fn extract_gdrive_id(url_str: &str) -> Option<String> {
    if let Ok(url) = reqwest::Url::parse(url_str) {
        let domain = url.domain()?;
        if domain == "drive.google.com" || domain == "docs.google.com" {
            if let Some(segments) = url.path_segments() {
                let segs: Vec<&str> = segments.collect();
                for i in 0..segs.len() {
                    if segs[i] == "d" && i + 1 < segs.len() {
                        return Some(segs[i + 1].to_owned());
                    }
                }
            }
            for (key, val) in url.query_pairs() {
                if key == "id" {
                    return Some(val.into_owned());
                }
            }
        }
    }
    None
}

pub fn extract_gdrive_confirm_token(html: &str) -> Option<String> {
    if let Some(confirm_pos) = html.find("confirm=") {
        let sub = &html[confirm_pos + 8..];
        if let Some(end_idx) = sub.find(|c| c == '&' || c == '"' || c == ';') {
            return Some(sub[..end_idx].to_owned());
        }
    }
    None
}

pub fn extract_mediafire_direct_url(html: &str) -> Option<String> {
    if let Some(btn_idx) = html.find("id=\"downloadButton\"") {
        let search_start = btn_idx.saturating_sub(250);
        let search_end = (btn_idx + 250).min(html.len());
        let chunk = &html[search_start..search_end];
        if let Some(href_idx) = chunk.find("href=\"") {
            let start = href_idx + 6;
            let sub = &chunk[start..];
            if let Some(end_idx) = sub.find('"') {
                let url = &sub[..end_idx];
                if url.contains("mediafire.com") {
                    return Some(url.to_owned());
                }
            }
        }
    }

    for prefix in &["https://download", "http://download"] {
        let mut search_pos = 0;
        while let Some(pos) = html[search_pos..].find(prefix) {
            let actual_pos = search_pos + pos;
            let sub = &html[actual_pos..];
            if let Some(end) = sub.find('"') {
                let url = &sub[..end];
                if url.contains(".mediafire.com/") {
                    return Some(url.to_owned());
                }
                search_pos = actual_pos + end;
            } else {
                break;
            }
        }
    }

    None
}

pub fn resolve_mediafire_url(client: &Client, url: &str) -> Result<String, String> {
    debug_log(format!("Resolving MediaFire link: {url}"));
    let html = client
        .get(url)
        .send()
        .map_err(|err| format!("Failed to request MediaFire page: {err}"))?
        .text()
        .map_err(|err| format!("Failed to read MediaFire HTML: {err}"))?;

    extract_mediafire_direct_url(&html).ok_or_else(|| {
        "Direct download link not found in MediaFire page. The link may have expired or requires a CAPTCHA/login."
            .to_owned()
    })
}

pub fn extract_input_value(html: &str, name: &str) -> Option<String> {
    let pattern = format!("name=\"{}\" value=\"", name);
    if let Some(pos) = html.find(&pattern) {
        let sub = &html[pos + pattern.len()..];
        if let Some(end) = sub.find('"') {
            return Some(sub[..end].to_owned());
        }
    }
    None
}

pub fn extract_form_action(html: &str) -> Option<String> {
    if let Some(form_pos) = html.find("<form") {
        let sub = &html[form_pos..];
        if let Some(action_pos) = sub.find("action=\"") {
            let action_sub = &sub[action_pos + 8..];
            if let Some(end_pos) = action_sub.find('"') {
                return Some(action_sub[..end_pos].to_owned());
            }
        }
    }
    None
}

pub fn resolve_gdrive_url(client: &Client, url: &str) -> Result<String, String> {
    debug_log(format!("Resolving Google Drive link: {url}"));
    let file_id = extract_gdrive_id(url)
        .ok_or_else(|| format!("Could not parse Google Drive File ID from URL: `{url}`."))?;

    let uc_url = format!("https://docs.google.com/uc?export=download&id={file_id}");
    let response = client
        .get(&uc_url)
        .send()
        .map_err(|err| format!("Failed to request Google Drive: {err}"))?;

    let final_url = response.url().clone();
    if final_url.as_str().contains("confirm=") {
        return Ok(final_url.to_string());
    }

    let body = response
        .text()
        .map_err(|err| format!("Failed to read Google Drive response body: {err}"))?;

    let form_action = extract_form_action(&body)
        .unwrap_or_else(|| "https://drive.usercontent.google.com/download".to_owned());
    if let Some(confirm_token) = extract_input_value(&body, "confirm") {
        let mut resolved =
            format!("{form_action}?id={file_id}&export=download&confirm={confirm_token}");
        if let Some(uuid) = extract_input_value(&body, "uuid") {
            resolved = format!("{resolved}&uuid={uuid}");
        }
        return Ok(resolved);
    }

    if let Some(token) = extract_gdrive_confirm_token(&body) {
        return Ok(format!(
            "https://docs.google.com/uc?export=download&confirm={token}&id={file_id}"
        ));
    }

    if body.contains("google-header-bar") || body.contains("Google Drive - ") {
        if body.contains("No se puede acceder") || body.contains("cannot access") {
            return Err("Access denied. The Google Drive file may be private.".to_owned());
        }
        if body.contains("quota exceeded") || body.contains("cuota superada") {
            return Err("Download quota exceeded for this Google Drive file.".to_owned());
        }
        return Err(
            "Google Drive returned an error page. The file may be deleted or private.".to_owned(),
        );
    }

    Ok(uc_url)
}

pub fn resolve_download_url(client: &Client, url: &str) -> Result<String, String> {
    let lower = url.to_lowercase();
    if lower.contains("mediafire.com") {
        resolve_mediafire_url(client, url)
    } else if lower.contains("drive.google.com") || lower.contains("docs.google.com") {
        resolve_gdrive_url(client, url)
    } else {
        Ok(url.to_owned())
    }
}

fn detect_archive_format_from_file(path: &Path) -> Result<ArchiveFormat, String> {
    let mut file = std::fs::File::open(fs_path(path))
        .map_err(|err| format!("Failed to open downloaded file for signature check: {err}"))?;
    let mut magic = [0u8; 7];
    let read = file
        .read(&mut magic)
        .map_err(|err| format!("Failed to read file signature bytes: {err}"))?;

    if read >= 4 && &magic[0..4] == b"PK\x03\x04" {
        Ok(ArchiveFormat::Zip)
    } else if read >= 7
        && (&magic[0..7] == b"Rar!\x1A\x07\x00" || &magic[0..7] == b"Rar!\x1A\x07\x01")
    {
        Ok(ArchiveFormat::Rar)
    } else {
        Err(
            "The downloaded file is not a valid ZIP or RAR archive (invalid file signature)."
                .to_owned(),
        )
    }
}

pub fn download_to_file(
    client: &Client,
    url: &str,
    cache_dir: &Path,
    slug: &str,
    is_cancelled: &dyn Fn() -> bool,
    mut report_download_progress: impl FnMut(u64, u64, u64, u64),
) -> Result<PathBuf, String> {
    if !path_exists(cache_dir) {
        create_dir_all_safe(cache_dir)?;
    }

    let resolved_url = resolve_download_url(client, url)?;
    debug_log(format!("Downloading from resolved URL: {resolved_url}"));

    let temp_path = cache_dir.join(format!("{slug}.part"));
    if path_exists(&temp_path) {
        let _ = remove_file_safe(&temp_path);
    }

    let download_res = (|| {
        let mut response = client
            .get(&resolved_url)
            .send()
            .map_err(|err| format!("Failed to download from `{resolved_url}`: {err}"))?
            .error_for_status()
            .map_err(|err| format!("HTTP error downloading from `{resolved_url}`: {err}"))?;

        let total_size = response.content_length().unwrap_or(0);

        let mut file = std::fs::File::create(fs_path(&temp_path)).map_err(|err| {
            format!(
                "Failed to create temporary download file `{}`: {err}",
                temp_path.display()
            )
        })?;

        let mut buffer = [0u8; 65536];
        let mut downloaded: u64 = 0;
        let start_time = std::time::Instant::now();
        let mut last_emit = std::time::Instant::now();

        loop {
            if is_cancelled() {
                return Err("Installation cancelled by user.".to_owned());
            }

            let len = match response.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) => return Err(format!("Error reading download stream: {err}")),
            };

            file.write_all(&buffer[..len])
                .map_err(|err| format!("Error writing download file: {err}"))?;

            downloaded += len as u64;

            let now = std::time::Instant::now();
            if now.duration_since(last_emit).as_millis() >= 200 || downloaded == total_size {
                last_emit = now;
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed = if elapsed > 0.0 {
                    (downloaded as f64 / elapsed).round() as u64
                } else {
                    0
                };
                let eta = if speed > 0 && total_size > downloaded {
                    (total_size - downloaded) / speed
                } else {
                    0
                };

                report_download_progress(downloaded, total_size, speed, eta);
            }
        }

        Ok(())
    })();

    if let Err(err) = download_res {
        if path_exists(&temp_path) {
            let _ = remove_file_safe(&temp_path);
        }
        return Err(err);
    }

    let format = match detect_archive_format_from_file(&temp_path) {
        Ok(fmt) => fmt,
        Err(err) => {
            let _ = remove_file_safe(&temp_path);
            return Err(err);
        }
    };

    let extension = match format {
        ArchiveFormat::Zip => "zip",
        ArchiveFormat::Rar => "rar",
    };

    let final_path = cache_dir.join(format!("{slug}.{extension}"));
    if path_exists(&final_path) {
        let _ = remove_file_safe(&final_path)?;
    }

    std::fs::rename(fs_path(&temp_path), fs_path(&final_path)).map_err(|err| {
        format!(
            "Failed to rename temporary file to final path `{}`: {err}",
            final_path.display()
        )
    })?;

    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_gdrive_id() {
        assert_eq!(
            extract_gdrive_id("https://drive.google.com/file/d/1A2B3C4D5E/view?usp=sharing")
                .as_deref(),
            Some("1A2B3C4D5E")
        );
        assert_eq!(
            extract_gdrive_id("https://drive.google.com/open?id=9X8Y7Z6W5V").as_deref(),
            Some("9X8Y7Z6W5V")
        );
        assert_eq!(
            extract_gdrive_id("https://docs.google.com/file/d/abc-123_xyz/edit").as_deref(),
            Some("abc-123_xyz")
        );
        assert_eq!(extract_gdrive_id("https://google.com"), None);
    }

    #[test]
    fn test_extract_gdrive_confirm_token() {
        let html = r#"<a href="/uc?export=download&amp;confirm=t_12A_B&amp;id=1234">Download</a>"#;
        assert_eq!(
            extract_gdrive_confirm_token(html).as_deref(),
            Some("t_12A_B")
        );

        let html_no_token = r#"<p>Some other content</p>"#;
        assert_eq!(extract_gdrive_confirm_token(html_no_token), None);
    }

    #[test]
    fn test_extract_form_input_values() {
        let html = r#"<form action="https://drive.usercontent.google.com/download"><input type="hidden" name="confirm" value="abc_123"><input type="hidden" name="uuid" value="xyz-789"></form>"#;
        assert_eq!(
            extract_form_action(html).as_deref(),
            Some("https://drive.usercontent.google.com/download")
        );
        assert_eq!(
            extract_input_value(html, "confirm").as_deref(),
            Some("abc_123")
        );
        assert_eq!(
            extract_input_value(html, "uuid").as_deref(),
            Some("xyz-789")
        );
    }

    #[test]
    fn test_extract_mediafire_direct_url() {
        let html = r#"<a class="input flags" href="https://download1234.mediafire.com/xyz123/file.zip" id="downloadButton">Download</a>"#;
        assert_eq!(
            extract_mediafire_direct_url(html).as_deref(),
            Some("https://download1234.mediafire.com/xyz123/file.zip")
        );

        let html_fallback = r#"<div>Some text and <a href="https://download5678.mediafire.com/abc/mod.zip">link</a></div>"#;
        assert_eq!(
            extract_mediafire_direct_url(html_fallback).as_deref(),
            Some("https://download5678.mediafire.com/abc/mod.zip")
        );
    }
}
