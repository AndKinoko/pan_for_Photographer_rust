use std::path::Path;

use image::imageops::FilterType;
use image::GenericImageView;

use crate::errors::AppError;

/// 大预览的最大尺寸（用于预览框）
const PREVIEW_MAX_W: u32 = 1616;
const PREVIEW_MAX_H: u32 = 1080;

/// 小缩略图的最大尺寸（用于文件列表图标）
const THUMB_MAX_W: u32 = 360;
const THUMB_MAX_H: u32 = 240;

/// 计算保持宽高比的缩放尺寸，上限为 max_w x max_h
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

/// 将图像缩放到适合最大尺寸，保存为JPEG格式
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

/// 生成全尺寸预览图像（最大1616×1080）用于预览框
pub async fn generate_preview(
    file_path: &Path,
    preview_path: &Path,
    file_type: &str,
) -> Result<(), AppError> {
    generate_resized(file_path, preview_path, file_type, PREVIEW_MAX_W, PREVIEW_MAX_H, "preview").await
}

/// 生成小缩略图（最大360×240）用于文件列表图标
pub async fn generate_thumbnail(
    file_path: &Path,
    thumb_path: &Path,
    file_type: &str,
) -> Result<(), AppError> {
    generate_resized(file_path, thumb_path, file_type, THUMB_MAX_W, THUMB_MAX_H, "thumbnail").await
}

/// 核心生成逻辑：打开图像，缩放到适合 max_w×max_h，保存为JPEG格式
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

/// 从原始字节快速读取JPEG图像尺寸，无需完全解码。
/// 扫描SOF（帧开始）标记：FF C0（基线）或 FF C2（渐进式）。
/// 返回 (width, height) 或如果无法读取则返回 None。
fn read_jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 10 {
        return None;
    }
    let mut i = 2; // 跳过SOI标记（FF D8）
    while i < data.len().saturating_sub(8) {
        if data[i] == 0xFF {
            let marker = data[i + 1];
            // SOF标记：C0-C3, C5-C7, C9-CB, CD-CF（所有帧开始变体）
            if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                // 标记结构：FF XX, 长度（2字节大端）, 精度（1）, 高度（2）, 宽度（2）
                if i + 9 < data.len() {
                    let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    if width > 0 && height > 0 {
                        return Some((width, height));
                    }
                }
                return None; // 找到SOF但无法解析，停止
            }
            // 跳过标记：FF 00 是转义，FF D0-D7 是RST标记（无数据）
            if marker == 0x00 || (0xD0..=0xD7).contains(&marker) {
                i += 2;
                continue;
            }
            // SOS标记（FF DA）—— 扫描开始，之后不再有元数据
            if marker == 0xDA {
                return None;
            }
            // 其他标记：读取长度并跳过
            if i + 3 < data.len() {
                let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                i += seg_len + 2; // +2 表示 FF XX 标记字节
            } else {
                i += 2;
            }
        } else {
            i += 1;
        }
    }
    None
}

/// 尝试从RAW文件中提取嵌入的JPEG预览。
///
/// 许多RAW格式（尤其是索尼ARW）包含多个嵌入的JPEG：
/// 先是一个小缩略图，然后是一个更大的预览。此函数扫描所有JPEG段，
/// 按像素面积选取最大的一个，并缩放到适合 max_w x max_h 的尺寸。
async fn extract_embedded_jpeg(
    file_path: &Path,
    preview_path: &Path,
    max_w: u32,
    max_h: u32,
    label: &str,
) -> Result<(), AppError> {
    let data = tokio::fs::read(file_path).await?;
    let file_size = data.len();

    // 扫描所有JPEG SOI标记（FF D8 FF）并记录其偏移量和尺寸
    let mut candidates: Vec<(usize, u32, u32)> = Vec::new();
    let mut i = 0;
    while i < data.len().saturating_sub(4) {
        if data[i] == 0xFF && data[i + 1] == 0xD8 && data[i + 2] == 0xFF {
            // 验证SOI后的字节是有效的JPEG标记
            // 有效标记：0xC0-0xCF（保留的0xC4/0xC8/0xCC除外），0xDB-0xDF，0xE0-0xEF，0xFE
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

            // 找到有效的JPEG SOI标记——尝试读取尺寸
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
            // 跳过无效的候选（尺寸不可读）以避免误报
        }
        i += 1;
    }

    if candidates.is_empty() {
        tracing::warn!("No embedded JPEG found in RAW file: {}", file_path.display());
        return Err(AppError::Internal("无法生成RAW文件预览".into()));
    }

    // 选取像素面积最大的候选
    let best = candidates
        .iter()
        .max_by_key(|(_, w, h)| (*w as u64) * (*h as u64))
        .ok_or_else(|| AppError::Internal("无法生成RAW文件预览".into()))?;

    let &(offset, orig_w, orig_h) = best;
    let jpeg_data = &data[offset..];

    // 找到EOI标记以裁剪尾部数据
    let mut end = jpeg_data.len();
    for i in (0..jpeg_data.len().saturating_sub(2)).rev() {
        if jpeg_data[i] == 0xFF && jpeg_data[i + 1] == 0xD9 {
            end = i + 2;
            break;
        }
    }

    tokio::fs::write(preview_path, &jpeg_data[..end]).await?;

    // 使用提供的最大尺寸进行缩放
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