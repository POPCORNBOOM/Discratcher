//! Source pictures: load, lay them out on the disc as layers, analyse.
//!
//! Positions are shares of the disc's diameter measured from its centre, with y pointing
//! down, so (0, 0) is the hub and ±0.5 the outer rim. The live preview in discRenderer.ts
//! reads layers the same way; keep the two in step.

use base64::Engine;
use image::{imageops::FilterType, GenericImageView};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

/// One picture placed on the disc. Layers are listed bottom first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub path: String,
    /// Centre, as a share of the diameter from the disc centre; y points down.
    pub x: f64,
    pub y: f64,
    /// Long edge as a share of the diameter.
    pub size: f64,
    /// Clockwise as seen on screen.
    pub rotation_deg: f64,
    #[serde(default)]
    pub invert: bool,
    /// -100 ..= 100.
    #[serde(default)]
    pub contrast: f32,
    /// -100 ..= 100: shifts every grey by up to a full range, after the contrast.
    #[serde(default)]
    pub brightness: f32,
    /// Sample the nearest pixel instead of blending the neighbours, so pixel art stays sharp.
    #[serde(default)]
    pub pixelated: bool,
    /// Tile the picture over the whole disc.
    #[serde(default)]
    pub repeat: bool,
    /// How much of its grid cell a tile fills, in percent: 100 abuts, less leaves gaps.
    #[serde(default = "full_density")]
    pub repeat_density: f32,
    /// Turn of the tile grid, in degrees; the pictures keep the layer's own rotation.
    #[serde(default)]
    pub repeat_angle: f32,
    /// How far every other row slides along, in percent of a tile: 50 lays bricks.
    #[serde(default)]
    pub repeat_stagger: f32,
}

fn full_density() -> f32 {
    100.0
}

impl Layer {
    /// A layer with every effect off, for the rest to be filled in.
    pub fn plain() -> Self {
        Layer {
            path: String::new(),
            x: 0.0,
            y: 0.0,
            size: 1.0,
            rotation_deg: 0.0,
            invert: false,
            contrast: 0.0,
            brightness: 0.0,
            pixelated: false,
            repeat: false,
            repeat_density: 100.0,
            repeat_angle: 0.0,
            repeat_stagger: 0.0,
        }
    }
}

/// A decoded picture: luma and coverage per pixel, rows top first.
#[derive(Clone, Debug)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub luma: Vec<u8>,
    pub alpha: Vec<u8>,
}

impl Picture {
    /// `max_edge` caps the long edge; `None` keeps the source size.
    pub fn load(path: &Path, max_edge: Option<u32>) -> Result<Self, String> {
        let img = image::open(path).map_err(|e| format!("无法读取图像 {}：{e}", path.display()))?;
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Err("图像是空的".into());
        }
        let img = match max_edge {
            Some(m) if w.max(h) > m => img.resize(m, m, FilterType::Triangle),
            _ => img,
        };
        Ok(Self::from_rgba(&img.to_rgba8()))
    }

    fn from_rgba(rgba: &image::RgbaImage) -> Self {
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let mut luma = Vec::with_capacity(w * h);
        let mut alpha = Vec::with_capacity(w * h);
        for p in rgba.pixels() {
            let [r, g, b, a] = p.0;
            // A fully clear pixel has no colour of its own; light keeps its edge from darkening.
            luma.push(if a == 0 { 255 } else { (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32).round() as u8 });
            alpha.push(a);
        }
        Self { width: w, height: h, luma, alpha }
    }

    /// An opaque picture from grey values, for tests and tools.
    pub fn from_luma(width: usize, height: usize, luma: Vec<u8>) -> Self {
        assert_eq!(luma.len(), width * height);
        Self { width, height, alpha: vec![255; luma.len()], luma }
    }

    /// The pixel `u`, `v` falls in, unblended.
    #[inline]
    fn nearest(&self, u: f64, v: f64) -> (f32, f32) {
        let x = ((u * self.width as f64) as usize).min(self.width - 1);
        let y = ((v * self.height as f64) as usize).min(self.height - 1);
        let i = y * self.width + x;
        (self.luma[i] as f32, self.alpha[i] as f32)
    }

    /// Luma and coverage, both 0 ..= 255, at `u`, `v` in [0, 1]. Edges clamp, as the GPU does.
    #[inline]
    fn bilinear(&self, u: f64, v: f64) -> (f32, f32) {
        let (w, h) = (self.width, self.height);
        let x = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
        let y = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
        let (ix, iy) = ((x as usize).min(w.saturating_sub(2)), (y as usize).min(h.saturating_sub(2)));
        let (fx, fy) = ((x - ix as f64) as f32, (y - iy as f64) as f32);
        let (dx, dy) = (usize::from(w > 1), if h > 1 { w } else { 0 });
        let i = iy * w + ix;
        let lerp = |d: &[u8]| {
            let (a, b, c, e) = (d[i] as f32, d[i + dx] as f32, d[i + dy] as f32, d[i + dy + dx] as f32);
            let top = a + (b - a) * fx;
            top + (c + (e - c) * fx - top) * fy
        };
        (lerp(&self.luma), lerp(&self.alpha))
    }
}

/// Gain of the standard contrast curve; `contrast` is -100 ..= 100.
pub fn contrast_gain(contrast: f32) -> f32 {
    let c = contrast.clamp(-100.0, 100.0) * 2.55;
    259.0 * (c + 255.0) / (255.0 * (259.0 - c))
}

/// What `brightness` (-100 ..= 100) adds to a grey, in levels.
pub fn brightness_shift(brightness: f32) -> f32 {
    if brightness.is_finite() { brightness.clamp(-100.0, 100.0) * 2.55 } else { 0.0 }
}

struct Placed {
    picture: Arc<Picture>,
    cx: f64,
    cy: f64,
    cos: f64,
    sin: f64,
    /// The picture's width and height as shares of the diameter.
    w: f64,
    h: f64,
    gain: f32,
    /// In grey levels, 0 ..= 255 either way.
    shift: f32,
    invert: bool,
    pixelated: bool,
    grid: Option<Grid>,
}

/// A layer's tiles: where the cells fall around the layer's own centre.
struct Grid {
    /// One cell's width and height, as shares of the diameter.
    cell_w: f64,
    cell_h: f64,
    cos: f64,
    sin: f64,
    /// Share of a cell every odd row slides along.
    stagger: f64,
}

impl Grid {
    /// `(dx, dy)` from the layer's centre becomes the same offset from the centre of the tile it falls in.
    #[inline]
    fn tile_offset(&self, dx: f64, dy: f64) -> (f64, f64) {
        // Into the grid's own axes, where rows run along x.
        let (gx, gy) = (self.cos * dx + self.sin * dy, -self.sin * dx + self.cos * dy);
        let row = (gy / self.cell_h).round();
        let gx = gx - (row.rem_euclid(2.0)) * self.stagger * self.cell_w;
        let lx = gx - (gx / self.cell_w).round() * self.cell_w;
        let ly = gy - row * self.cell_h;
        (self.cos * lx - self.sin * ly, self.sin * lx + self.cos * ly)
    }
}

/// Layers ready to sample.
pub struct Composition {
    layers: Vec<Placed>,
}

impl Composition {
    pub fn new(layers: impl IntoIterator<Item = (Layer, Arc<Picture>)>) -> Self {
        let layers = layers
            .into_iter()
            .map(|(l, picture)| {
                let long = picture.width.max(picture.height) as f64;
                let size = if l.size.is_finite() && l.size > 0.0 { l.size } else { 1.0 };
                let (sin, cos) = l.rotation_deg.to_radians().sin_cos();
                let (w, h) = (size * picture.width as f64 / long, size * picture.height as f64 / long);
                let grid = l.repeat.then(|| {
                    let fill = if l.repeat_density.is_finite() { l.repeat_density.clamp(10.0, 100.0) as f64 / 100.0 } else { 1.0 };
                    let (gs, gc) = (if l.repeat_angle.is_finite() { l.repeat_angle as f64 } else { 0.0 }).to_radians().sin_cos();
                    let stagger = if l.repeat_stagger.is_finite() { l.repeat_stagger.clamp(0.0, 100.0) as f64 / 100.0 } else { 0.0 };
                    Grid { cell_w: w / fill, cell_h: h / fill, cos: gc, sin: gs, stagger }
                });
                Placed {
                    cx: l.x,
                    cy: l.y,
                    cos,
                    sin,
                    w,
                    h,
                    gain: contrast_gain(l.contrast),
                    shift: brightness_shift(l.brightness),
                    invert: l.invert,
                    pixelated: l.pixelated,
                    grid,
                    picture,
                }
            })
            .collect();
        Self { layers }
    }

    pub fn load(layers: &[Layer], max_edge: Option<u32>) -> Result<Self, String> {
        let mut out = Vec::with_capacity(layers.len());
        for l in layers {
            out.push((l.clone(), Arc::new(Picture::load(Path::new(&l.path), max_edge)?)));
        }
        Ok(Self::new(out))
    }

    /// Grey, 0 ..= 255, at a disc position (see the module note). Where no picture covers
    /// the disc it stays light, so empty space adds no write load.
    #[inline]
    pub fn sample(&self, x: f64, y: f64) -> f32 {
        let mut grey = 255.0f32;
        for l in &self.layers {
            let (dx, dy) = (x - l.cx, y - l.cy);
            let (dx, dy) = l.grid.as_ref().map_or((dx, dy), |g| g.tile_offset(dx, dy));
            // Undo the clockwise turn.
            let u = (l.cos * dx + l.sin * dy) / l.w + 0.5;
            let v = (-l.sin * dx + l.cos * dy) / l.h + 0.5;
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                continue;
            }
            let (luma, alpha) = if l.pixelated { l.picture.nearest(u, v) } else { l.picture.bilinear(u, v) };
            let t = ((luma - 128.0) * l.gain + 128.0 + l.shift).clamp(0.0, 255.0);
            let t = if l.invert { 255.0 - t } else { t };
            grey += (t - grey) * alpha / 255.0;
        }
        grey
    }
}

/// The picture squeezed onto a `size` square for the live preview: per pixel its luma,
/// then its coverage. The preview stretches it back to the picture's own shape.
pub fn preview_texture(path: &Path, size: u32) -> Result<Vec<u8>, String> {
    let img = image::open(path).map_err(|e| format!("无法读取图像 {}：{e}", path.display()))?;
    let p = Picture::from_rgba(&img.resize_exact(size, size, FilterType::Triangle).to_rgba8());
    Ok(p.luma.iter().zip(&p.alpha).flat_map(|(&l, &a)| [l, a]).collect())
}

fn composite_luma(rgba: [u8; 4]) -> u8 {
    let [r, g, b, a] = rgba;
    let y = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    let a = a as f32 / 255.0;
    (y * a + 255.0 * (1.0 - a)).round().clamp(0.0, 255.0) as u8
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
    let thumb = img.thumbnail(512, 512).to_rgba8();
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
pub(crate) mod tests {
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

    /// A layer of `picture` covering the disc, centred and upright.
    pub(crate) fn whole(picture: Picture) -> Composition {
        let l = Layer { path: String::new(), x: 0.0, y: 0.0, size: 1.0, ..Layer::plain() };
        Composition::new([(l, Arc::new(picture))])
    }

    fn layer(x: f64, y: f64, size: f64, rotation_deg: f64) -> Layer {
        Layer { path: String::new(), x, y, size, rotation_deg, ..Layer::plain() }
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
    fn empty_disc_and_clear_pixels_stay_light() {
        assert_eq!(Composition::new([]).sample(0.1, 0.2), 255.0);
        let d = tmp("alpha");
        let p = write_png(&d, "a.png", 8, 8, |_, _| [0, 0, 0, 0]);
        let mut l = layer(0.0, 0.0, 1.0, 0.0);
        l.path = p.to_string_lossy().into();
        l.invert = true;
        // Inverting does not turn a clear pixel dark: it is not part of the picture.
        assert_eq!(Composition::load(&[l], None).unwrap().sample(0.0, 0.0), 255.0);
    }

    #[test]
    fn wide_picture_keeps_its_shape() {
        // 2:1, so at size 1 it spans the whole width but only half the height.
        let c = Composition::new([(layer(0.0, 0.0, 1.0, 0.0), Arc::new(Picture::from_luma(2, 1, vec![0, 0])))]);
        assert_eq!(c.sample(0.45, 0.0), 0.0);
        assert_eq!(c.sample(0.0, 0.3), 255.0, "below the picture");
    }

    #[test]
    fn upper_layer_covers_lower() {
        let black = Arc::new(Picture::from_luma(1, 1, vec![0]));
        let grey = Arc::new(Picture::from_luma(1, 1, vec![100]));
        let c = Composition::new([(layer(0.0, 0.0, 1.0, 0.0), black), (layer(0.2, 0.0, 0.2, 0.0), grey)]);
        assert_eq!(c.sample(0.2, 0.0), 100.0);
        assert_eq!(c.sample(-0.2, 0.0), 0.0);
    }

    #[test]
    fn placement_moves_turns_and_scales() {
        // Left half black, right half white.
        let p = Arc::new(Picture::from_luma(2, 2, vec![0, 255, 0, 255]));
        let at = |l: Layer, x, y| Composition::new([(l, p.clone())]).sample(x, y);
        assert_eq!(at(layer(0.0, 0.0, 0.5, 0.0), -0.2, 0.0), 0.0);
        // Moved right by 0.2: its black half now sits where its centre was.
        assert_eq!(at(layer(0.2, 0.0, 0.5, 0.0), 0.05, 0.0), 0.0);
        // A quarter turn clockwise takes the left (black) half to the top, y < 0.
        assert_eq!(at(layer(0.0, 0.0, 0.5, 90.0), 0.0, -0.2), 0.0);
        assert_eq!(at(layer(0.0, 0.0, 0.5, 90.0), 0.0, 0.2), 255.0);
        // Twice the size reaches twice as far.
        assert_eq!(at(layer(0.0, 0.0, 0.5, 0.0), -0.4, 0.0), 255.0);
        assert_eq!(at(layer(0.0, 0.0, 1.0, 0.0), -0.4, 0.0), 0.0);
    }

    #[test]
    fn tone_follows_the_layer() {
        let p = Arc::new(Picture::from_luma(1, 1, vec![64]));
        let mut l = layer(0.0, 0.0, 1.0, 0.0);
        l.invert = true;
        assert_eq!(Composition::new([(l.clone(), p.clone())]).sample(0.0, 0.0), 191.0);
        l.invert = false;
        l.contrast = 100.0;
        assert_eq!(Composition::new([(l.clone(), p.clone())]).sample(0.0, 0.0), 0.0);
        l.contrast = 0.0;
        l.brightness = 50.0;
        assert_eq!(Composition::new([(l.clone(), p.clone())]).sample(0.0, 0.0), 64.0 + 127.5);
        l.brightness = -100.0;
        assert_eq!(Composition::new([(l, p)]).sample(0.0, 0.0), 0.0);
    }

    #[test]
    fn pixelated_keeps_each_pixel_whole() {
        let p = Arc::new(Picture::from_luma(2, 1, vec![0, 200]));
        let mut l = layer(0.0, 0.0, 1.0, 0.0);
        let at = |l: &Layer, x: f64| Composition::new([(l.clone(), p.clone())]).sample(x, 0.0);
        assert!((at(&l, 0.0) - 100.0).abs() < 1.0, "the seam blends");
        l.pixelated = true;
        assert_eq!(at(&l, -0.01), 0.0);
        assert_eq!(at(&l, 0.01), 200.0);
    }

    #[test]
    fn repeat_tiles_the_picture() {
        let p = Arc::new(Picture::from_luma(2, 1, vec![0, 255]));
        let mut l = layer(0.0, 0.0, 0.2, 0.0);
        let at = |l: &Layer, x: f64, y: f64| Composition::new([(l.clone(), p.clone())]).sample(x, y);
        assert_eq!(at(&l, 0.15, 0.0), 255.0, "off the picture, bare disc");
        l.repeat = true;
        assert_eq!(at(&l, 0.15, 0.0), 0.0, "the next tile over, left half");
        assert_eq!(at(&l, 0.25, 0.0), 255.0, "its right half");
        l.repeat_density = 50.0;
        assert_eq!(at(&l, 0.15, 0.0), 255.0, "a gap now");
        l.repeat_density = 100.0;
        l.repeat_angle = 90.0;
        assert!((at(&l, 0.12, 0.0) - 178.5).abs() < 1.0, "the grid has turned, the pictures have not");
        l.repeat_angle = 0.0;
        l.repeat_stagger = 50.0;
        // Row 1 slides half a tile: x = 0.1 is now a tile's centre.
        assert!((at(&l, 0.1, 0.1) - 127.5).abs() < 1.0);
    }

    #[test]
    fn preview_texture_interleaves_luma_and_coverage() {
        let d = tmp("texture");
        let p = write_png(&d, "a.png", 4, 4, |x, _| if x < 2 { [0, 0, 0, 255] } else { [0, 0, 0, 0] });
        let t = preview_texture(&p, 4).unwrap();
        assert_eq!(t.len(), 4 * 4 * 2);
        assert_eq!((t[0], t[1]), (0, 255));
        assert_eq!((t[6], t[7]), (255, 0), "clear");
    }

    #[test]
    fn layer_json_is_what_the_front_end_sends() {
        let l: Layer = serde_json::from_str(r#"{"id":"a","path":"/a.png","x":0.1,"y":-0.2,"size":0.8,"rotationDeg":15,"invert":true,"contrast":10,"brightness":-20,"width":10,"height":20}"#).unwrap();
        assert_eq!(l, Layer { path: "/a.png".into(), x: 0.1, y: -0.2, size: 0.8, rotation_deg: 15.0, invert: true, contrast: 10.0, brightness: -20.0, ..Layer::plain() });
    }
}
