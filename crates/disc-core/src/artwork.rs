//! Source image handling: load, map onto the disc square, tone adjust, analyse.

use base64::Engine;
use image::{imageops::FilterType, GenericImageView};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CropMode {
    #[default]
    SquareCenter,
    Stretch,
    Fit,
    /// Fit, then let the user place and scale the picture themselves.
    Custom,
}

/// Square grayscale raster covering the disc diameter. 255 is the light tone.
#[derive(Clone, Debug)]
pub struct GrayField {
    pub size: usize,
    pub data: Vec<u8>,
}

impl GrayField {
    /// Nearest neighbour; `u`, `v` in [0, 1). Outside the raster is light.
    #[inline]
    pub fn nearest(&self, u: f64, v: f64) -> u8 {
        let (x, y) = (u * self.size as f64, v * self.size as f64);
        if x < 0.0 || y < 0.0 || x >= self.size as f64 || y >= self.size as f64 {
            return 255;
        }
        self.data[y as usize * self.size + x as usize]
    }

    /// Bilinear; `u`, `v` in [0, 1). Outside the raster is light.
    #[inline]
    pub fn bilinear(&self, u: f64, v: f64) -> f32 {
        let s = self.size;
        let (x, y) = (u * s as f64 - 0.5, v * s as f64 - 0.5);
        if x < 0.0 || y < 0.0 || x >= (s - 1) as f64 || y >= (s - 1) as f64 {
            return self.nearest(u, v) as f32;
        }
        let (ix, iy) = (x as usize, y as usize);
        let (fx, fy) = ((x - ix as f64) as f32, (y - iy as f64) as f32);
        let i = iy * s + ix;
        let d = &self.data;
        let (a, b, c, e) = (d[i] as f32, d[i + 1] as f32, d[i + s] as f32, d[i + s + 1] as f32);
        let top = a + (b - a) * fx;
        let bot = c + (e - c) * fx;
        top + (bot - top) * fy
    }
}

/// Field coordinates of a point already rotated into image space. The image is placed
/// scale first, then rotation, then position, so this undoes translate, rotation (done by
/// the caller) and scale last. Undoing scale last is what keeps the position independent
/// of the zoom: moving by 10% stays 10% at any scale.
#[inline]
pub fn to_field(rotated_u: f64, rotated_v: f64, offset_u: f64, offset_v: f64, scale: f64) -> (f64, f64) {
    let k = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    ((rotated_u - offset_u) / k + 0.5, (rotated_v - offset_v) / k + 0.5)
}

fn composite_luma(rgba: [u8; 4]) -> u8 {
    let [r, g, b, a] = rgba;
    let y = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    let a = a as f32 / 255.0;
    (y * a + 255.0 * (1.0 - a)).round().clamp(0.0, 255.0) as u8
}

/// Standard contrast curve. `contrast` is -100 ..= 100.
fn contrast_lut(contrast: f32, invert: bool) -> [u8; 256] {
    let c = contrast.clamp(-100.0, 100.0) * 2.55;
    let k = 259.0 * (c + 255.0) / (255.0 * (259.0 - c));
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        let y = ((i as f32 - 128.0) * k + 128.0).round().clamp(0.0, 255.0) as u8;
        *v = if invert { 255 - y } else { y };
    }
    lut
}

/// Load `path` onto a square raster of `size`, or the source's own long edge when `None`.
pub fn load_field(path: &Path, crop: CropMode, size: Option<u32>, contrast: f32, invert: bool) -> Result<GrayField, String> {
    let img = image::open(path).map_err(|e| format!("无法读取图像 {}：{e}", path.display()))?;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err("图像是空的".into());
    }
    let s = size.unwrap_or(w.max(h)).max(16);
    let filter = if s == w.max(h) { FilterType::Nearest } else { FilterType::Triangle };
    let (cx, cy, cw, ch) = match crop {
        CropMode::SquareCenter => {
            let m = w.min(h);
            ((w - m) / 2, (h - m) / 2, m, m)
        }
        _ => (0, 0, w, h),
    };
    let cropped = img.crop_imm(cx, cy, cw, ch);
    let (tw, th) = match crop {
        CropMode::Fit | CropMode::Custom => {
            let k = s as f64 / w.max(h) as f64;
            (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1))
        }
        _ => (s, s),
    };
    let scaled = if (tw, th) == (cw, ch) { cropped } else { cropped.resize_exact(tw, th, filter) };
    let (ox, oy) = ((s - tw) / 2, (s - th) / 2);
    let s = s as usize;
    let lut = contrast_lut(contrast, invert);
    // Letterbox stays light whatever `invert` says: it must not add write load.
    let mut data = vec![255u8; s * s];
    let rgba = scaled.to_rgba8();
    for (x, y, p) in rgba.enumerate_pixels() {
        data[(y + oy) as usize * s + (x + ox) as usize] = lut[composite_luma(p.0) as usize];
    }
    Ok(GrayField { size: s, data })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtworkAnalysis {
    pub width: u32,
    pub height: u32,
    /// Share of pixels with luma below 128.
    pub dark_ratio: f64,
    pub suggest_invert: bool,
    /// Longest horizontal run of solid black, a proxy for the wide lines that break tracking.
    pub max_black_run_px: u32,
    pub thumbnail: String,
}

pub const INVERT_SUGGEST_RATIO: f64 = 0.35;

pub fn analyze(path: &Path) -> Result<ArtworkAnalysis, String> {
    let img = image::open(path).map_err(|e| format!("无法读取图像 {}：{e}", path.display()))?;
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();
    let mut dark = 0u64;
    let mut run = 0u32;
    let mut max_run = 0u32;
    for (x, _, p) in rgba.enumerate_pixels() {
        if x == 0 {
            run = 0;
        }
        let y = composite_luma(p.0);
        dark += (y < 128) as u64;
        run = if y < 16 { run + 1 } else { 0 };
        max_run = max_run.max(run);
    }
    let dark_ratio = dark as f64 / (width as u64 * height as u64).max(1) as f64;
    let thumb = img.thumbnail(512, 512).to_rgb8();
    let mut buf = Vec::new();
    thumb
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(ArtworkAnalysis {
        width,
        height,
        dark_ratio,
        suggest_invert: dark_ratio > INVERT_SUGGEST_RATIO,
        max_black_run_px: max_run,
        thumbnail: format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&buf)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(dir: &Path, name: &str, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> std::path::PathBuf {
        let p = dir.join(name);
        image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y))).save(&p).unwrap();
        p
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("disc-core-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn dark_image_suggests_invert() {
        let d = tmp("dark");
        let p = write_png(&d, "a.png", 40, 40, |x, _| if x < 30 { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        let a = analyze(&p).unwrap();
        assert!(a.suggest_invert);
        assert!((a.dark_ratio - 0.75).abs() < 1e-9);
        assert_eq!(a.max_black_run_px, 30);
    }

    #[test]
    fn transparency_composites_to_light_and_fit_letterboxes_light() {
        let d = tmp("alpha");
        let p = write_png(&d, "a.png", 8, 4, |_, _| [0, 0, 0, 0]);
        let f = load_field(&p, CropMode::Stretch, Some(8), 0.0, false).unwrap();
        assert!(f.data.iter().all(|&v| v == 255));
        let p = write_png(&d, "b.png", 32, 16, |_, _| [0, 0, 0, 255]);
        let f = load_field(&p, CropMode::Fit, Some(32), 0.0, true).unwrap();
        assert_eq!(f.data[0], 255, "letterbox");
        assert_eq!(f.data[32 * 16], 255, "inverted black");
        let f = load_field(&p, CropMode::Fit, Some(32), 0.0, false).unwrap();
        assert_eq!(f.data[0], 255, "letterbox ignores invert");
        assert_eq!(f.data[32 * 16], 0);
    }

    #[test]
    fn position_is_not_scaled_with_the_image() {
        // The image's own scale divides the residual, never the offset: a 10% move is 10%
        // at 200% zoom and at 50% zoom alike.
        let (u, v) = to_field(0.1, 0.0, 0.1, 0.0, 2.0);
        assert!((u - 0.5).abs() < 1e-12 && (v - 0.5).abs() < 1e-12);
        let (u, _) = to_field(0.0, 0.0, 0.2, 0.0, 2.0);
        assert!((u - (0.5 - 0.1)).abs() < 1e-12);
        // A zero or nonsensical scale falls back to 1 instead of exploding.
        assert_eq!(to_field(0.2, 0.0, 0.0, 0.0, 0.0), (0.7, 0.5));
    }

    #[test]
    fn bilinear_outside_is_light() {
        let f = GrayField { size: 2, data: vec![0; 4] };
        assert_eq!(f.bilinear(-0.1, 0.5), 255.0);
        assert_eq!(f.nearest(1.2, 0.5), 255);
    }
}
