use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use image::imageops::FilterType;
use image::GenericImageView;
use image::ImageReader;
use image::Limits;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;

/// 大预览的最大尺寸（用于预览框）
const PREVIEW_MAX_W: u32 = 1616;
const PREVIEW_MAX_H: u32 = 1080;

/// 小缩略图的最大尺寸（用于文件列表图标）
const THUMB_MAX_W: u32 = 360;
const THUMB_MAX_H: u32 = 240;

/// 解码前的尺寸上限。
///
/// **必要性**：`image` 的 `ImageReader` 按格式分派解码器时，**只有 PNG 会拿到
/// `Limits`**（`io/image_reader_type.rs:183` 只对 `ImageFormat::Png` 传
/// `limits_for_png`），JPEG / GIF / WebP / TIFF / AVIF 一律走
/// `Decoder::new(...)` —— 也就是**默认的 `Limits::default()`，而它的
/// `max_image_width` / `max_image_height` 都是 `None`（不限）**。
/// 结果是：上传一张声明为大尺寸、实际高压缩比的 JPEG（几 MB 文件能撑出
/// 几个 GB 的解码结果），`image::open` 会把它整张读进内存，直接 OOM 掉整个
/// 进程。`preview_semaphore` 只限并发数（2），限不住单个任务的内存。
///
/// 所以这里绕开 `ImageReader::decode()`，改成自己拿解码器再 `set_limits`，
/// 这条路径对所有格式都生效。
///
/// 取值 20000×10000：允许远超常规的 200MP 原图（无人机/中画幅常见），
/// 同时把「几 MB 文件撑爆内存」彻底堵死。200MP × 4 通道 ≈ 800MB，
/// 配合 2 并发已远在可控范围。
pub(crate) const MAX_DECODE_PIXELS_W: u32 = 20_000;
pub(crate) const MAX_DECODE_PIXELS_H: u32 = 10_000;

/// 打开图片并施加解码上限。
///
/// 错误信息会明确指向尺寸超限，便于从 GC 的 `preview_attempts` 重投上限里
/// 定位到这类文件（这类文件重投也一定失败，最终会以 `preview_path IS NULL`
/// 留在库里，属于可接受的失败形态——总比进程 OOM 强）。
pub(crate) fn open_limited(path: &Path) -> Result<image::DynamicImage, image::ImageError> {
    use image::ImageDecoder;

    // Limits 是 #[non_exhaustive]，只能用 Default 再逐字段改，
    // 不能写结构体字面量。
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_PIXELS_W);
    limits.max_image_height = Some(MAX_DECODE_PIXELS_H);

    let mut decoder = ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    decoder.set_limits(limits)?;
    Ok(image::DynamicImage::from_decoder(decoder)?)
}

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
///
/// 注：生产路径（`generate_preview_and_thumb`）已改为「解码一次缩放两次」，
/// 不再单独调用本函数；保留给只需要预览图的调用方与测试。
#[allow(dead_code)]
pub fn generate_preview(
    file_path: &Path,
    preview_path: &Path,
    file_type: &str,
) -> Result<(), AppError> {
    generate_resized(file_path, preview_path, file_type, PREVIEW_MAX_W, PREVIEW_MAX_H, "preview")
}

/// **解码一次、缩放两次**，同时产出预览图与缩略图。
///
/// 为什么要专门开这个入口：原先 `generate_preview_and_thumb` 分别调用
/// `generate_preview` 与 `generate_thumbnail`，两者各自 `open_limited` 一遍——
/// 于是**同一个文件被完整解码两次**。解码是整条链路里最贵的一步（一张
/// 45MP 的 RAW 解码后约占 180MB 内存），做两遍纯属浪费，而且两遍的峰值
/// 不会同时出现、但会推高整体 CPU 占用。
///
/// 这里把解码提到循环外，两个尺寸各自 `resize_and_save` 复用同一份
/// `DynamicImage`。RAW 路径仍走各自的 `extract_embedded_jpeg`（那是流式
/// 常量内存的，不构成浪费），但复用了解码结果。
fn generate_both_from_source(
    file_path: &Path,
    preview_out: &Path,
    thumb_out: &Path,
    file_type: &str,
) -> (Result<(), AppError>, Result<(), AppError>) {
    let ft = file_type.to_lowercase();
    let raw_formats = [
        "nef", "cr2", "cr3", "crw", "arw", "sr2", "srf", "dng", "raf", "orf", "rw2", "nrw",
    ];

    if raw_formats.contains(&ft.as_str()) {
        // RAW 有两条路：抽内嵌 JPEG（流式常量内存），或整体解码。
        // 两条都各做一遍即可——它们本身不重复工作，重复的只是「解码」，
        // 而 `open_limited` 那条已经解码出 DynamicImage 时顺手就复用。
        let preview_result = extract_embedded_jpeg(file_path, preview_out, PREVIEW_MAX_W, PREVIEW_MAX_H, "preview");
        let thumb_result = if preview_result.is_ok() {
            // 预览图已由流式路径产出；缩略图直接缩放它，避免再抽一次 RAW。
            // 缩略图只有 360×240，从预览图（≤1616×1080）缩下来画质足够。
            match open_limited(preview_out) {
                Ok(img) => resize_and_save(&img, thumb_out, THUMB_MAX_W, THUMB_MAX_H).map(|_| ()),
                Err(e) => Err(AppError::Internal(e.to_string())),
            }
        } else {
            extract_embedded_jpeg(file_path, thumb_out, THUMB_MAX_W, THUMB_MAX_H, "thumbnail")
        };
        return (preview_result, thumb_result);
    }

    // 普通图片：解码一次，两个尺寸复用同一份 DynamicImage
    let img = match open_limited(file_path) {
        Ok(i) => i,
        Err(e) => {
            // AppError 不实现 Clone，两个返回值各自构造一个
            return (
                Err(AppError::Internal(e.to_string())),
                Err(AppError::Internal(e.to_string())),
            );
        }
    };
    let preview = resize_and_save(&img, preview_out, PREVIEW_MAX_W, PREVIEW_MAX_H).map(|_| ());
    let thumb = resize_and_save(&img, thumb_out, THUMB_MAX_W, THUMB_MAX_H).map(|_| ());
    (preview, thumb)
}

/// 生成小缩略图（最大360×240）用于文件列表图标
///
/// 注：同上——生产路径走 `generate_both_from_source`。
#[allow(dead_code)]
pub fn generate_thumbnail(
    file_path: &Path,
    thumb_path: &Path,
    file_type: &str,
) -> Result<(), AppError> {
    generate_resized(file_path, thumb_path, file_type, THUMB_MAX_W, THUMB_MAX_H, "thumbnail")
}

/// 注：仅由上面的 `generate_preview` / `generate_thumbnail` 调用（均已不在生产路径）。
/// 核心生成逻辑：打开图像，缩放到适合 max_w×max_h，保存为JPEG格式
fn generate_resized(
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
        std::fs::create_dir_all(parent).ok();
    }

    if image_formats.contains(&ft.as_str()) {
        let img = open_limited(file_path)?;
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
        match open_limited(file_path) {
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
                extract_embedded_jpeg(file_path, output_path, max_w, max_h, label)?;
            }
        }
    } else {
        return Err(AppError::BadRequest("该文件类型不支持预览".into()));
    }

    Ok(())
}

/// 扫描嵌入 JPEG 时的分块大小。
const SCAN_CHUNK: usize = 64 * 1024;

/// 抽取嵌入 JPEG 的字节上限（64MB）。
///
/// 嵌入预览通常远小于此值（60MP 机型的全尺寸预览约 30MB 以内）。
/// 设上限的目的是防止「整个文件都是 JPEG 样数据」时把预览目录写爆——
/// 修复前这里是无上限地写整个文件。
const MAX_EMBEDDED_JPEG_BYTES: u64 = 64 * 1024 * 1024;

/// 单次扫描最多记录的候选数，防止病态输入造成无界内存增长。
const MAX_CANDIDATES: usize = 64;

/// 判断 SOI 之后的字节是否为合法的 JPEG 标记起始。
/// 有效：0xC0-0xCF（保留的 0xC4/0xC8/0xCC 除外）、0xDB-0xDF、0xE0-0xEF、0xFE
fn is_valid_jpeg_marker(b: u8) -> bool {
    matches!(b,
        0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF |
        0xDB..=0xDF |
        0xE0..=0xEF |
        0xFE
    )
}

/// 从指定偏移开始流式解析 JPEG 头部，返回 `(width, height)`。
///
/// 只向前顺序读取（按段长度跳过 APP1/EXIF 等大段），因此内存占用为常量，
/// 与 `MAX_FILE_SIZE` 无关。这一点是修复的关键：原实现要求把整个文件读进内存。
fn jpeg_dimensions_at(path: &Path, offset: u64) -> Option<(u32, u32)> {
    let f = std::fs::File::open(path).ok()?;
    let mut r = BufReader::with_capacity(8 * 1024, f);
    r.seek(SeekFrom::Start(offset)).ok()?;

    // SOI（FF D8）
    let mut soi = [0u8; 2];
    r.read_exact(&mut soi).ok()?;
    if soi != [0xFF, 0xD8] {
        return None;
    }

    let mut byte = [0u8; 1];
    loop {
        // 找下一个标记，跳过 FF 填充字节
        r.read_exact(&mut byte).ok()?;
        if byte[0] != 0xFF {
            return None; // 结构与 JPEG 不符，放弃
        }
        let marker;
        loop {
            r.read_exact(&mut byte).ok()?;
            if byte[0] != 0xFF {
                marker = byte[0];
                break;
            }
        }

        // SOS：熵编码数据开始；此前没遇到 SOF 就说明取不到尺寸
        // EOI：图像已结束
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        // 无长度字段的独立标记（TEM / RST）
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        let mut len = [0u8; 2];
        r.read_exact(&mut len).ok()?;
        let seg_len = u16::from_be_bytes(len) as usize;
        if seg_len < 2 {
            return None;
        }

        // SOF 段：长度(2) 精度(1) 高度(2) 宽度(2)
        let is_sof = matches!(marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF);
        if is_sof {
            let mut hdr = [0u8; 5];
            r.read_exact(&mut hdr).ok()?;
            let height = u16::from_be_bytes([hdr[1], hdr[2]]) as u32;
            let width = u16::from_be_bytes([hdr[3], hdr[4]]) as u32;
            if width > 0 && height > 0 {
                return Some((width, height));
            }
            return None;
        }

        // 其余段：按长度跳过负载（EXIF 段可达数十 KB）
        let skip = (seg_len - 2) as u64;
        if skip > 0 {
            let copied = std::io::copy(&mut r.by_ref().take(skip), &mut std::io::sink()).ok()?;
            if copied < skip {
                return None; // 文件提前结束
            }
        }
    }
}

/// 分块扫描文件，收集所有「看起来像嵌入 JPEG 起始」的候选及其尺寸。
///
/// 与旧实现不同，这里**不把文件读进内存**：按 `SCAN_CHUNK` 顺序读取，
/// 每块只保留前一块末尾 3 字节作为重叠窗口，以便发现跨块边界的 `FF D8 FF`。
/// 内存占用为 `SCAN_CHUNK + 3`，与文件大小无关。
fn find_embedded_jpeg_candidates(path: &Path) -> Result<Vec<(u64, u32, u32)>, AppError> {
    let mut f = std::fs::File::open(path)?;
    let file_len = f.metadata().map(|m| m.len()).unwrap_or(0);

    let mut candidates: Vec<(u64, u32, u32)> = Vec::new();
    let mut chunk = vec![0u8; SCAN_CHUNK];
    let mut carry: Vec<u8> = Vec::with_capacity(3);
    let mut chunk_start: u64 = 0; // 本块新数据的绝对起始偏移

    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }

        // 窗口 = 上一块尾部保留的 3 字节 + 本块
        let mut window = Vec::with_capacity(carry.len() + n);
        window.extend_from_slice(&carry);
        window.extend_from_slice(&chunk[..n]);
        let base = chunk_start - carry.len() as u64;

        let mut i = 0usize;
        while i + 3 < window.len() {
            if window[i] == 0xFF && window[i + 1] == 0xD8 && window[i + 2] == 0xFF
                && is_valid_jpeg_marker(window[i + 3])
            {
                let abs = base + i as u64;
                // 只接受「起始字节落在本块或重叠窗口内」的候选，避免重复记录
                // 上一块已经检查过的标记（否则会在块边界重复收集）。
                if abs + 4 > chunk_start && candidates.len() < MAX_CANDIDATES {
                    if let Some((w, h)) = jpeg_dimensions_at(path, abs) {
                        tracing::debug!(
                            "Found embedded JPEG at offset {} in {}: {}x{}",
                            abs,
                            path.display(),
                            w,
                            h
                        );
                        candidates.push((abs, w, h));
                    }
                }
            }
            i += 1;
        }

        // 保留末尾 3 字节到下一块（最多 3 字节，覆盖 FF D8 FF 的跨界情形）
        let keep = window.len().min(3);
        carry = window[window.len() - keep..].to_vec();
        chunk_start += n as u64;

        if chunk_start >= file_len {
            break;
        }
    }

    Ok(candidates)
}

/// 从 `offset` 起流式抽取嵌入 JPEG 到 `out_path`，返回写出的字节数。
///
/// 语义与旧实现一致：在文件中向前找到**最后**一个 `FF D9`（EOI）作为结束位置，
/// 把 `[offset, eoi_end)` 写成 JPEG。区别是全程流式，且写入量受
/// `MAX_EMBEDDED_JPEG_BYTES` 限制。
fn extract_jpeg_stream(
    path: &Path,
    offset: u64,
    out_path: &Path,
) -> Result<Option<u64>, AppError> {
    let src = std::fs::File::open(path)?;
    let mut reader = BufReader::with_capacity(256 * 1024, src);
    reader.seek(SeekFrom::Start(offset))?;

    let mut out = std::fs::File::create(out_path)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut written: u64 = 0;
    let mut last_eoi_end: Option<u64> = None; // 相对 offset
    let mut prev: Option<u8> = None; // 跨块的 FF 配对

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }

        // 记录 EOI（FF D9）——用 prev 跨块配对，避免漏掉块边界上的 EOI
        for (idx, &byte) in buf[..n].iter().enumerate() {
            if prev == Some(0xFF) && byte == 0xD9 {
                last_eoi_end = Some(written + idx as u64 + 1);
            }
            prev = Some(byte);
        }

        // 超过上限后仍继续扫描（为了定位最后一个 EOI），但不再写盘
        if written < MAX_EMBEDDED_JPEG_BYTES {
            let room = (MAX_EMBEDDED_JPEG_BYTES - written) as usize;
            let take = n.min(room);
            out.write_all(&buf[..take])?;
            written += take as u64;
        }
    }

    if written == 0 {
        let _ = std::fs::remove_file(out_path);
        return Ok(None);
    }

    out.flush()?;
    // 截到最后一个 EOI；没找到 EOI 时保留已写内容（与旧实现一致）。
    // 若 EOI 落在写入上限之外，则保留截断后的内容。
    let final_len = match last_eoi_end {
        Some(e) if e <= written => e,
        _ => written,
    };
    out.set_len(final_len)?;

    Ok(Some(final_len))
}

/// 尝试从RAW文件中提取嵌入的JPEG预览。
///
/// 许多RAW格式（尤其是索尼ARW）包含多个嵌入的JPEG：
/// 先是一个小缩略图，然后是一个更大的预览。此函数扫描所有JPEG段，
/// 按像素面积选取最大的一个，并缩放到适合 max_w x max_h 的尺寸。
///
/// **内存模型**：全程分块流式处理，峰值内存为常数（约 `SCAN_CHUNK` + 输出缓冲），
/// 与文件大小无关。此前这里是 `std::fs::read(file_path)` 整体读入，
/// 配合默认 `MAX_FILE_SIZE` = 10GB 与预览信号量 = 2，峰值可达约 20GB 而 OOM。
fn extract_embedded_jpeg(
    file_path: &Path,
    preview_path: &Path,
    max_w: u32,
    max_h: u32,
    label: &str,
) -> Result<(), AppError> {
    let file_size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);

    // 第 1 遍：分块扫描候选（只记录偏移与尺寸，不保留文件内容）
    let candidates = find_embedded_jpeg_candidates(file_path)?;

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

    // 第 2 遍：从命中偏移流式抽取到最后一个 EOI（写入量受上限约束）
    let extracted = extract_jpeg_stream(file_path, offset, preview_path)?
        .ok_or_else(|| AppError::Internal("无法生成RAW文件预览".into()))?;

    // 第 3 遍：这里 `image::open` 读的是**已抽出的嵌入 JPEG**（≤ 64MB），
    // 而不是整个 RAW 文件——这是本次修复的关键。
    match open_limited(preview_path) {
        Ok(img) => {
            let (width, height) = img.dimensions();
            let (new_w, new_h) = calc_resize_dims(width, height, max_w, max_h);
            let resized = img.resize_exact(new_w, new_h, FilterType::Lanczos3);
            let rgb = resized.to_rgb8();
            rgb.save(preview_path)?;

            tracing::info!(
                "Extracted embedded JPEG {} from RAW: {} ({}x{} -> {}x{}, raw_size={}MB, extracted={}KB, found {} candidates)",
                label,
                file_path.display(),
                orig_w,
                orig_h,
                new_w,
                new_h,
                file_size / 1_048_576,
                extracted / 1024,
                candidates.len()
            );
        }
        Err(e) => {
            // 抽出的字节不是可解码的 JPEG（例如噪声里恰好出现 FF D8 FF）。
            // 删掉半成品并报错，让调用方保持 preview_path = NULL——
            // 否则会把一个无法解码的文件登记成「已生成预览」。
            tracing::warn!(
                "Extracted bytes are not a decodable JPEG from {}: {:?}",
                file_path.display(),
                e
            );
            let _ = std::fs::remove_file(preview_path);
            return Err(AppError::Internal("无法生成RAW文件预览".into()));
        }
    }

    Ok(())
}

/// 在后台任务（blocking 线程）中为某个已落盘的文件生成预览图与缩略图。
///
/// 返回 `(preview_rel, thumb_rel)`，失败对应的项返回 `None`；两者都失败时
/// 不产出任何文件。调用方应在成功后 UPDATE files 表写入路径。
pub fn generate_preview_and_thumb(
    config: &Config,
    owner_id: i64,
    file_path: &Path,
    file_type: &str,
) -> (Option<String>, Option<String>) {
    let preview_dir = config
        .upload_dir
        .join(format!("user_{}", owner_id))
        .join("previews");
    let _ = std::fs::create_dir_all(&preview_dir);

    let preview_name = format!("{}.jpg", Uuid::new_v4().simple());
    let preview_rel = format!("user_{}/previews/{}", owner_id, preview_name);
    let thumb_name = format!("{}_thumb.jpg", Uuid::new_v4().simple());
    let thumb_rel = format!("user_{}/previews/{}", owner_id, thumb_name);

    let preview_full = config.upload_dir.join(&preview_rel);
    let thumb_full = config.upload_dir.join(&thumb_rel);

    // 解码一次、两张图复用，而不是各自 open_limited 一遍
    let (preview_result, thumb_result) =
        generate_both_from_source(file_path, &preview_full, &thumb_full, file_type);

    match (preview_result, thumb_result) {
        (Ok(()), Ok(())) => (Some(preview_rel), Some(thumb_rel)),
        (Ok(()), Err(e)) => {
            tracing::warn!("Thumbnail generation failed: {:?}", e);
            let _ = std::fs::remove_file(&thumb_full);
            (Some(preview_rel), None)
        }
        (Err(e), Ok(())) => {
            tracing::warn!("Preview generation failed: {:?}", e);
            let _ = std::fs::remove_file(&preview_full);
            (None, Some(thumb_rel))
        }
        (Err(e1), Err(e2)) => {
            tracing::warn!("Preview generation failed: {:?}, thumbnail: {:?}", e1, e2);
            let _ = std::fs::remove_file(&preview_full);
            let _ = std::fs::remove_file(&thumb_full);
            (None, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use std::path::PathBuf;

    fn test_config(upload_dir: PathBuf) -> Config {
        Config {
            server_host: "127.0.0.1".into(),
            server_port: 0,
            database_url: "sqlite::memory:".into(),
            upload_dir,
            static_dir: "static".into(),
            jwt_secret: b"0123456789abcdef0123456789abcdef".to_vec(),
            max_file_size: 1024,
            gc_interval_sec: 0,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pan_preview_test_{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn encode_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::new(width, height);
        let mut buf = Vec::new();
        JpegEncoder::new(&mut buf)
            .encode(img.as_raw(), width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
        buf
    }

    #[test]
    fn calc_resize_dims_preserves_aspect_and_never_upscales() {
        // 横图受高度限制
        assert_eq!(calc_resize_dims(4000, 3000, 1616, 1080), (1440, 1080));
        // 竖图受宽度限制
        assert_eq!(calc_resize_dims(3000, 4000, 1616, 1080), (810, 1080));
        // 小图不放大
        assert_eq!(calc_resize_dims(100, 50, 1616, 1080), (100, 50));
        // 恰好等于上限
        assert_eq!(calc_resize_dims(1616, 1080, 1616, 1080), (1616, 1080));
        // 正方形
        assert_eq!(calc_resize_dims(2000, 2000, 1000, 1000), (1000, 1000));
    }

    /// 写一个临时文件，便于测试基于文件的流式解析。
    fn write_tmp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn jpeg_dimensions_at_reads_encoded_jpeg() {
        let dir = temp_dir("dims");
        let jpeg = encode_jpeg(40, 24);
        let p = write_tmp(&dir, "a.jpg", &jpeg);
        assert_eq!(jpeg_dimensions_at(&p, 0), Some((40, 24)));
        // 从非零偏移开始同样能解析（RAW 里嵌入 JPEG 就是这种情形）
        let mut padded = vec![0u8; 128];
        padded.extend_from_slice(&jpeg);
        let p2 = write_tmp(&dir, "b.bin", &padded);
        assert_eq!(jpeg_dimensions_at(&p2, 128), Some((40, 24)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jpeg_dimensions_at_rejects_garbage() {
        let dir = temp_dir("dims_bad");
        let cases: [(&str, &[u8]); 4] = [
            ("empty.bin", &[]),
            ("soi_only.bin", &[0xFF, 0xD8]),
            ("zeros.bin", &[0u8; 64]),
            ("text.bin", b"not a jpeg at all"),
        ];
        for (name, bytes) in cases {
            let p = write_tmp(&dir, name, bytes);
            assert_eq!(jpeg_dimensions_at(&p, 0), None, "{} 不应解析出尺寸", name);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P0-2 的核心回归：嵌入 JPEG 必须能被**分块**扫描发现，
    /// 即便它的 SOI 标记恰好跨越 `SCAN_CHUNK` 边界。
    /// 这里刻意让 JPEG 起始于 `SCAN_CHUNK - 2`，使 `FF D8 FF xx` 跨越块边界。
    #[test]
    fn scanner_finds_embedded_jpeg_across_chunk_boundary() {
        let dir = temp_dir("scan_cross");
        let jpeg = encode_jpeg(120, 80);
        let mut raw = vec![0x11u8; SCAN_CHUNK - 2];
        let jpeg_off = raw.len();
        raw.extend_from_slice(&jpeg);
        raw.extend_from_slice(&[0x22u8; 1_000]);

        let p = write_tmp(&dir, "fake.nef", &raw);
        let cands = find_embedded_jpeg_candidates(&p).unwrap();
        assert!(
            cands
                .iter()
                .any(|(off, w, h)| *off == jpeg_off as u64 && *w == 120 && *h == 80),
            "应定位到偏移 {} 处的嵌入 JPEG，实际得到 {:?}",
            jpeg_off,
            cands
        );
        // 跨块场景下不应把同一个标记重复收集两次
        let same_offset = cands.iter().filter(|(o, _, _)| *o == jpeg_off as u64).count();
        assert_eq!(same_offset, 1, "同一偏移不应重复入候选: {:?}", cands);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 抽取必须只写出嵌入 JPEG 的字节（到最后一个 EOI），尾部噪声要裁掉。
    #[test]
    fn extract_jpeg_stream_writes_only_the_jpeg() {
        let dir = temp_dir("extract");
        let jpeg = encode_jpeg(120, 80);
        let mut raw = vec![0x11u8; 2_000];
        let off = raw.len();
        raw.extend_from_slice(&jpeg);
        raw.extend_from_slice(&[0x22u8; 50_000]); // 必须被裁掉

        let src = write_tmp(&dir, "fake.nef", &raw);
        let out = dir.join("out.jpg");
        let n = extract_jpeg_stream(&src, off as u64, &out).unwrap().unwrap();
        assert_eq!(
            n,
            jpeg.len() as u64,
            "抽取长度应恰好等于嵌入 JPEG（裁到最后一个 EOI）"
        );
        // 抽出的字节应当能直接解码
        assert_eq!(image::open(&out).unwrap().dimensions(), (120, 80));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 端到端：伪 RAW（含嵌入 JPEG）应能生成预览与缩略图。
    /// 这条路径在修复前会把整个文件读进内存。
    #[test]
    fn generate_preview_and_thumb_extracts_embedded_jpeg_from_fake_raw() {
        let upload_dir = temp_dir("raw_e2e");
        let config = test_config(upload_dir.clone());

        let jpeg = encode_jpeg(1200, 900);
        let mut raw = vec![0u8; 1_024];
        raw.extend_from_slice(&jpeg);
        raw.extend_from_slice(&[0x33u8; 4_096]);
        let src = write_tmp(&upload_dir, "DSC_0001.NEF", &raw);

        let (preview_rel, thumb_rel) = generate_preview_and_thumb(&config, 3, &src, "nef");
        let preview_rel = preview_rel.expect("应从嵌入 JPEG 生成预览");
        let thumb_rel = thumb_rel.expect("应生成缩略图");

        // 源图 1200x900 小于预览上限 1616x1080，因此不放大
        assert_eq!(
            image::open(upload_dir.join(preview_rel))
                .unwrap()
                .dimensions(),
            (1200, 900)
        );
        assert!(upload_dir.join(thumb_rel).exists());
        let _ = std::fs::remove_dir_all(&upload_dir);
    }

    /// 完全找不到嵌入 JPEG 时应报错，而不是留下半成品文件
    /// （调用方据此保持 preview_path = NULL，由 GC 计入失败次数并最终放弃重投）。
    #[test]
    fn extract_reports_failure_when_no_embedded_jpeg() {
        let dir = temp_dir("no_jpeg");
        let src = write_tmp(&dir, "junk.nef", &[0xAAu8; 8_192]);
        let r = generate_preview_and_thumb(&test_config(dir.clone()), 4, &src, "nef");
        assert_eq!(r, (None, None), "无嵌入 JPEG 时不应产出预览");

        // 不应在预览目录留下半成品
        let previews = dir.join("user_4").join("previews");
        let leftovers: Vec<_> = std::fs::read_dir(&previews)
            .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "预览目录不应残留文件: {:?}", leftovers);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_preview_and_thumb_produces_both_files() {
        let upload_dir = temp_dir("e2e");
        let config = test_config(upload_dir.clone());

        let src = upload_dir.join("source.png");
        image::RgbImage::new(1000, 500).save(&src).unwrap();

        let (preview_rel, thumb_rel) =
            generate_preview_and_thumb(&config, 1, &src, "png");

        let preview_rel = preview_rel.expect("应生成预览图");
        let thumb_rel = thumb_rel.expect("应生成缩略图");
        let preview = upload_dir.join(&preview_rel);
        let thumb = upload_dir.join(&thumb_rel);

        assert!(preview.exists());
        assert!(thumb.exists());
        assert_eq!(image::open(&preview).unwrap().dimensions(), (1000, 500));
        assert_eq!(image::open(&thumb).unwrap().dimensions(), (360, 180));

        let _ = std::fs::remove_dir_all(&upload_dir);
    }

    #[test]
    fn generate_preview_and_thumb_rejects_unsupported_type() {
        let upload_dir = temp_dir("unsupported");
        let config = test_config(upload_dir.clone());

        let src = upload_dir.join("note.txt");
        std::fs::write(&src, b"hello").unwrap();

        let result = generate_preview_and_thumb(&config, 2, &src, "txt");
        assert_eq!(result, (None, None));

        let _ = std::fs::remove_dir_all(&upload_dir);
    }
}