use std::fs::{self, File};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageFormat, RgbaImage};

use super::staged::create_dir_all;
use crate::{CliError, Result};

const DEFAULT_FORGE_SVG_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">
  <rect width="128" height="128" rx="24" fill="#181c20"/>
  <path d="M24 44h80l-12 20H40v18h36l-8 16H24V44z" fill="#e58529"/>
  <rect x="24" y="96" width="80" height="8" rx="4" fill="#2fa89a"/>
</svg>
"##;

/// Multi-platform icon assets generated once during staging and consumed by Linux, Windows, and macOS bundlers.
#[derive(Debug, Clone)]
pub struct IconSet {
    pub svg_path: PathBuf,
    pub png_128: PathBuf,
    pub png_256: PathBuf,
    pub png_512: PathBuf,
    pub ico_path: PathBuf,
    pub icns_path: PathBuf,
}

impl IconSet {
    /// Generates `.svg`, `.png` (`128`/`256`/`512`), Windows `.ico`, and macOS `.icns` inside `out_dir`
    /// from an optional source PNG/SVG icon (falling back to FORGE's built-in badge).
    pub fn generate(
        project_root: &Path,
        configured_icon: Option<&Path>,
        out_dir: &Path,
    ) -> Result<Self> {
        let icons_dir = out_dir.join("icons");
        create_dir_all(&icons_dir)?;

        let resolved_src = configured_icon.map(|p| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                project_root.join(p)
            }
        });

        let svg_path = icons_dir.join("icon.svg");
        if let Some(src) = &resolved_src
            && src.is_file()
            && src
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
        {
            fs::copy(src, &svg_path).map_err(|e| CliError::Io {
                path: svg_path.clone(),
                message: format!("failed to copy SVG icon: {e}"),
            })?;
        } else {
            fs::write(&svg_path, DEFAULT_FORGE_SVG_ICON).map_err(|e| CliError::Io {
                path: svg_path.clone(),
                message: format!("failed to write default SVG icon: {e}"),
            })?;
        }

        let master: RgbaImage = match &resolved_src {
            Some(src) if src.is_file() => image::open(src)
                .map(|img| img.into_rgba8())
                .unwrap_or_else(|_| render_default_badge(512)),
            _ => render_default_badge(512),
        };

        let img_32 = image::imageops::resize(&master, 32, 32, FilterType::Triangle);
        let img_128 = image::imageops::resize(&master, 128, 128, FilterType::Triangle);
        let img_256 = image::imageops::resize(&master, 256, 256, FilterType::Triangle);
        let img_512 = image::imageops::resize(&master, 512, 512, FilterType::Triangle);

        let png_128 = icons_dir.join("icon-128.png");
        let png_256 = icons_dir.join("icon-256.png");
        let png_512 = icons_dir.join("icon-512.png");
        save_png(&img_128, &png_128)?;
        save_png(&img_256, &png_256)?;
        save_png(&img_512, &png_512)?;

        // Windows `.ico` (32x32 + 256x256 frames)
        let ico_path = icons_dir.join("icon.ico");
        write_ico_file(&ico_path, &[(&img_32, 32), (&img_256, 256)])?;

        // macOS `.icns` (`ic07` 128px, `ic08` 256px, `ic09` 512px PNG chunks)
        let icns_path = icons_dir.join("icon.icns");
        write_icns_file(
            &icns_path,
            &[
                (*b"ic07", &img_128),
                (*b"ic08", &img_256),
                (*b"ic09", &img_512),
            ],
        )?;

        Ok(Self {
            svg_path,
            png_128,
            png_256,
            png_512,
            ico_path,
            icns_path,
        })
    }
}

fn save_png(img: &RgbaImage, dst: &Path) -> Result<()> {
    img.save_with_format(dst, ImageFormat::Png)
        .map_err(|e| CliError::Io {
            path: dst.to_path_buf(),
            message: format!("failed to encode PNG icon: {e}"),
        })
}

fn encode_png_bytes(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| CliError::InvalidArgument(format!("failed to encode PNG frame: {e}")))?;
    Ok(buf.into_inner())
}

fn write_ico_file(dst: &Path, frames_in: &[(&RgbaImage, u32)]) -> Result<()> {
    let file = File::create(dst).map_err(|e| CliError::Io {
        path: dst.to_path_buf(),
        message: format!("failed to create .ico file: {e}"),
    })?;
    let encoder = IcoEncoder::new(file);
    let mut frames = Vec::with_capacity(frames_in.len());
    for &(img, dim) in frames_in {
        let frame = IcoFrame::as_png(img.as_raw(), dim, dim, ExtendedColorType::Rgba8)
            .map_err(|e| CliError::InvalidArgument(format!("failed to encode ICO frame: {e}")))?;
        frames.push(frame);
    }
    encoder.encode_images(&frames).map_err(|e| CliError::Io {
        path: dst.to_path_buf(),
        message: format!("failed to write .ico file: {e}"),
    })
}

fn write_icns_file(dst: &Path, chunks: &[([u8; 4], &RgbaImage)]) -> Result<()> {
    let mut encoded_chunks = Vec::with_capacity(chunks.len());
    let mut total_len: u32 = 8; // "icns" + u32 total size
    for (ostype, img) in chunks {
        let png_bytes = encode_png_bytes(img)?;
        let chunk_len = 8u32 + (png_bytes.len() as u32);
        total_len = total_len.saturating_add(chunk_len);
        encoded_chunks.push((*ostype, chunk_len, png_bytes));
    }

    let mut out = Vec::with_capacity(total_len as usize);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&total_len.to_be_bytes());
    for (ostype, chunk_len, png_bytes) in encoded_chunks {
        out.extend_from_slice(&ostype);
        out.extend_from_slice(&chunk_len.to_be_bytes());
        out.extend_from_slice(&png_bytes);
    }

    fs::write(dst, out).map_err(|e| CliError::Io {
        path: dst.to_path_buf(),
        message: format!("failed to write .icns file: {e}"),
    })
}

fn render_default_badge(size: u32) -> RgbaImage {
    let mut img = RgbaImage::new(size, size);
    let pad = size / 12;
    for (x, y, px) in img.enumerate_pixels_mut() {
        let in_box = x >= pad && x < size - pad && y >= pad && y < size - pad;
        if !in_box {
            *px = image::Rgba([0, 0, 0, 0]);
            continue;
        }
        let nx = (x * 128) / size;
        let ny = (y * 128) / size;
        let is_f = (24..104).contains(&nx) && (40..58).contains(&ny)
            || (24..44).contains(&nx) && (40..92).contains(&ny)
            || (24..82).contains(&nx) && (66..82).contains(&ny);
        let is_bar = (24..104).contains(&nx) && (96..106).contains(&ny);
        *px = if is_f {
            image::Rgba([229, 133, 41, 255])
        } else if is_bar {
            image::Rgba([47, 168, 154, 255])
        } else {
            image::Rgba([24, 28, 32, 255])
        };
    }
    img
}
