use std::path::Path;

use image::imageops::FilterType;
use image::GenericImageView;

use crate::errors::AppError;

/// Max dimensions for large preview (used in preview box)
const PREVIEW_MAX_W: u32 = 1616;
const PREVIEW_MAX_H: u32 = 1080;

/// Max dimensions for small thumbnail (used in file list icons)
const THUMB_MAX_W: u32 = 360;
const THUMB_MAX_H: u32 = 240;

/// Calculate resize dimensions maintaining aspect ratio, capped at max_w x max_h
fn calc_resize_dims(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let w_ratio = max_w as f64 / width as f64;
    let h_ratio = max_h as f64 / height as f64;
    let ratio = w_ratio.min(h_ratio);
    if ratio >= 1.0 {
        (width, height)
    } else {
        ((width as f64 * ratio) as u32, (height as f64 * ratio) as u32)
    }
}

/// Resize an image to fit within max dimensions, save as JPEG
fn resize_and_save(
    img: &image::DynamicImage,
    preview_path: &Path,
    max_w: u32,
    max_h: u32,
) -> Result<(u32, u32), AppError> {
    let (width, height) = img.dimensions();
    let (new_w, new_h) = calc_resize_dims(width, height, max_w, max_h);
    let resized = img.resize_exact(new_w, new_h, FilterType::Lanczos3);
    let rgb = resized.to_rgb8();
    rgb.save(preview_path)?;
    Ok((new_w, new_h))
}

/// Generate a full-size preview image (max 1616×1080) for the preview box
pub async fn generate_preview(
    file_path: &Path,
    preview_path: &Path,
    file_type: &str,
) -> Result<(), AppError> {
    generate_resized(file_path, preview_path, file_type, PREVIEW_MAX_W, PREVIEW_MAX_H, "preview").await
}

/// Generate a small thumbnail (max 360×240) for file list icons
pub async fn generate_thumbnail(
    file_path: &Path,
    thumb_path: &Path,
    file_type: &str,
) -> Result<(), AppError> {
    generate_resized(file_path, thumb_path, file_type, THUMB_MAX_W, THUMB_MAX_H, "thumbnail").await
}

/// Core generation logic: open image, resize to fit max_w×max_h, save as JPEG
async fn generate_resized(
    file_path: &Path,
    output_path: &Path,
    file_type: &str,
    max_w: u32,
    max_h: u32,
    label: &str,
) -> Result<(), AppError> {
    let ft = file_type.to_lowercase();
    let image_formats = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];
    let raw_formats = [
        "nef", "cr2", "cr3", "crw", "arw", "sr2", "srf", "dng", "raf", "orf", "rw2", "nrw",
    ];

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    if image_formats.contains(&ft.as_str()) {
        let img = image::open(file_path)?;
        let (width, height) = img.dimensions();
        let (new_w, new_h) = resize_and_save(&img, output_path, max_w, max_h)?;
        tracing::info!(
            "Generated {} {}: {} ({}x{} -> {}x{})",
            ft,
            label,
            file_path.display(),
            width,
            height,
            new_w,
            new_h
        );
    } else if raw_formats.contains(&ft.as_str()) {
        match image::open(file_path) {
            Ok(img) => {
                let (width, height) = img.dimensions();
                let (new_w, new_h) = resize_and_save(&img, output_path, max_w, max_h)?;
                tracing::info!(
                    "Generated RAW {} via image crate: {} ({}x{} -> {}x{})",
                    label,
                    file_path.display(),
                    width,
                    height,
                    new_w,
                    new_h
                );
            }
            Err(_) => {
                extract_embedded_jpeg(file_path, output_path, max_w, max_h, label).await?;
            }
        }
    } else {
        return Err(AppError::BadRequest("该文件类型不支持预览".into()));
    }

    Ok(())
}

/// Quickly read JPEG image dimensions from raw bytes without full decoding.
/// Scans for SOF (Start of Frame) markers: FF C0 (baseline) or FF C2 (progressive).
/// Returns (width, height) or None if dimensions cannot be read.
fn read_jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 10 {
        return None;
    }
    let mut i = 2; // skip SOI marker (FF D8)
    while i < data.len().saturating_sub(8) {
        if data[i] == 0xFF {
            let marker = data[i + 1];
            // SOF markers: C0-C3, C5-C7, C9-CB, CD-CF (all Start of Frame variants)
            if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                // Marker structure: FF XX, length (2 bytes BE), precision (1), height (2), width (2)
                if i + 9 < data.len() {
                    let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    if width > 0 && height > 0 {
                        return Some((width, height));
                    }
                }
                return None; // SOF found but can't parse, stop
            }
            // Skip marker: FF 00 is escaped, FF D0-D7 are RST markers (no data)
            if marker == 0x00 || (0xD0..=0xD7).contains(&marker) {
                i += 2;
                continue;
            }
            // SOS marker (FF DA) — start of scan, no more metadata after this
            if marker == 0xDA {
                return None;
            }
            // Other markers: read length and skip
            if i + 3 < data.len() {
                let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                i += seg_len + 2; // +2 for the FF XX marker bytes
            } else {
                i += 2;
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Try to extract an embedded JPEG preview from a RAW file.
///
/// Many RAW formats (especially Sony ARW) contain multiple embedded JPEGs:
/// a small thumbnail first, then a larger preview. This function scans for
/// ALL JPEG segments, picks the largest one by pixel area, and resizes to
/// fit within max_w x max_h.
async fn extract_embedded_jpeg(
    file_path: &Path,
    preview_path: &Path,
    max_w: u32,
    max_h: u32,
    label: &str,
) -> Result<(), AppError> {
    let data = tokio::fs::read(file_path).await?;
    let file_size = data.len();

    // Scan for all JPEG SOI markers (FF D8 FF) and record their offsets + dimensions
    let mut candidates: Vec<(usize, u32, u32)> = Vec::new();
    let mut i = 0;
    while i < data.len().saturating_sub(4) {
        if data[i] == 0xFF && data[i + 1] == 0xD8 && data[i + 2] == 0xFF {
            // Validate that the byte after SOI is a valid JPEG marker
            // Valid markers: 0xC0-0xCF (except 0xC4/0xC8/0xCC reserved), 0xDB-0xDF, 0xE0-0xEF, 0xFE
            let next_byte = data[i + 3];
            let is_valid_marker = matches!(next_byte,
                0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF |
                0xDB..=0xDF |
                0xE0..=0xEF |
                0xFE
            );
            if !is_valid_marker {
                i += 1;
                continue;
            }

            // Found a valid JPEG SOI marker — try to read dimensions
            let jpeg_slice = &data[i..];
            if let Some((w, h)) = read_jpeg_dimensions(jpeg_slice) {
                candidates.push((i, w, h));
                tracing::debug!(
                    "Found embedded JPEG at offset {} in {}, dimensions: {}x{}",
                    i,
                    file_path.display(),
                    w,
                    h
                );
            }
            // Skip invalid candidates (dimensions unreadable) to avoid false positives
        }
        i += 1;
    }

    if candidates.is_empty() {
        tracing::warn!("No embedded JPEG found in RAW file: {}", file_path.display());
        return Err(AppError::Internal("无法生成RAW文件预览".into()));
    }

    // Pick the candidate with the largest pixel area
    let best = candidates
        .iter()
        .max_by_key(|(_, w, h)| (*w as u64) * (*h as u64))
        .unwrap();

    let (offset, orig_w, orig_h) = *best;
    let jpeg_data = &data[offset..];

    // Find the EOI marker to trim trailing data
    let mut end = jpeg_data.len();
    for i in (0..jpeg_data.len().saturating_sub(2)).rev() {
        if jpeg_data[i] == 0xFF && jpeg_data[i + 1] == 0xD9 {
            end = i + 2;
            break;
        }
    }

    tokio::fs::write(preview_path, &jpeg_data[..end]).await?;

    // Resize using the provided max dimensions
    if let Ok(img) = image::open(preview_path) {
        let (width, height) = img.dimensions();
        let (new_w, new_h) = calc_resize_dims(width, height, max_w, max_h);
        let resized = img.resize_exact(new_w, new_h, FilterType::Lanczos3);
        let rgb = resized.to_rgb8();
        rgb.save(preview_path)?;

        tracing::info!(
            "Extracted embedded JPEG {} from RAW: {} ({}x{} -> {}x{}, file_size={}MB, found {} candidates)",
            label,
            file_path.display(),
            orig_w,
            orig_h,
            new_w,
            new_h,
            file_size / 1_048_576,
            candidates.len()
        );
    }

    Ok(())
}