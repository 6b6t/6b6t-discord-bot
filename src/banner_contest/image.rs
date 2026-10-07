use anyhow::{Result, bail};
use image::{ImageFormat, ImageReader, imageops::FilterType};
use std::io::Cursor;
pub const MAX_DOWNLOAD: u64 = 20 * 1024 * 1024;
pub const MAX_GUILD_IMAGE: usize = 10 * 1024 * 1024;

/// Decode with strict allocation and dimension limits, centre crop once, JPEG once.
pub fn crop(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() > usize::try_from(MAX_DOWNLOAD)? {
        bail!("image too large");
    }
    let format = image::guess_format(bytes)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        bail!("unsupported format");
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?;
    let (w, h) = (image.width(), image.height());
    if w < 1280 || h < 720 {
        bail!("image too small");
    }
    // Integral 16:9 dimensions avoid a second crop in any later step.
    let unit = (w / 16).min(h / 9);
    let (cw, ch) = (unit * 16, unit * 9);
    let cropped = image.crop_imm((w - cw) / 2, (h - ch) / 2, cw, ch);
    let unit = unit.min(120);
    let resized = cropped
        .resize_exact(unit * 16, unit * 9, FilterType::Lanczos3)
        .to_rgb8();
    let mut output = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 85).encode_image(&resized)?;
    if output.len() >= MAX_GUILD_IMAGE {
        bail!("encoded image too large");
    }
    Ok(output)
}
