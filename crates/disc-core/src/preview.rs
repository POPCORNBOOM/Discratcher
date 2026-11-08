//! Physical preview: what the disc shows at human viewing scale.
//!
//! Each pixel on the `actual` disc is traced to the channel bit under it, then that bit
//! is looked up in the `design` geometry the image was carved for. With the same preset
//! on both sides the art comes back straight; with the wrong stamper it shears exactly as
//! a real burn would.

use crate::artwork::Composition;
use crate::spiral::Spiral;
use base64::Engine;
use rayon::prelude::*;
use std::f64::consts::TAU;
use std::io::Cursor;

pub struct PreviewInput<'a> {
    pub design: &'a Spiral,
    pub actual: &'a Spiral,
    pub art: &'a Composition,
    /// Lowest luma the guard lets through, 0 when off.
    pub min_luma: f32,
    pub size: u32,
}

/// Luma at pixel-centre polar position. `None` outside the recorded band.
fn trace(p: &PreviewInput, r: f64, phi: f64) -> Option<f32> {
    let a = p.actual;
    if r < a.params.ri || r > a.params.ro {
        return None;
    }
    let (n_r, th_r) = a.at_radius(r);
    let turns = ((th_r - phi) / TAU).round();
    let bit = n_r + (phi + turns * TAU - th_r) * r / a.params.bit_mm;
    if bit < 0.0 || bit >= p.design.total_bits() {
        return None;
    }
    let (rd, th) = p.design.locate(bit);
    let ro2 = 2.0 * p.design.params.ro;
    Some(p.art.sample(rd * th.cos() / ro2, rd * th.sin() / ro2).max(p.min_luma))
}

/// RGBA pixels, transparent off the disc.
pub fn render(p: &PreviewInput) -> Vec<u8> {
    let size = p.size as usize;
    let view_r = p.actual.params.ro.max(p.design.params.ro) + 0.6;
    let mm_per_px = 2.0 * view_r / size as f64;
    let mut img = vec![0u8; size * size * 4];
    img.par_chunks_mut(size * 4).enumerate().for_each(|(y, row)| {
        let dy = (y as f64 + 0.5 - size as f64 / 2.0) * mm_per_px;
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let dx = (x as f64 + 0.5 - size as f64 / 2.0) * mm_per_px;
            let r = dx.hypot(dy);
            let phi = dy.atan2(dx);
            let (v, a) = match trace(p, r, phi) {
                Some(v) => (v, 255.0),
                // Unrecorded mirror band and lead areas read as the bare disc.
                None if (15.0..=59.0).contains(&r) => (232.0, 255.0),
                None => (0.0, 0.0),
            };
            px.copy_from_slice(&[v as u8, v as u8, v as u8, a as u8]);
        }
    });
    img
}

pub fn render_data_url(p: &PreviewInput) -> Result<String, String> {
    let img = image::RgbaImage::from_raw(p.size, p.size, render(p)).ok_or("preview buffer size")?;
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&buf)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artwork::{tests::whole, Composition, Layer, Picture};
    use crate::preset::reference_preset;
    use std::sync::Arc;

    fn setup(design: &Spiral, actual: &Spiral, art: &Composition) -> Vec<u8> {
        render(&PreviewInput { design, actual, art, min_luma: 0.0, size: 256 })
    }

    #[test]
    fn same_geometry_reproduces_the_art() {
        let s = Spiral::new(reference_preset().spiral_params()).unwrap();
        // Left half black, right half white.
        let art = whole(Picture::from_luma(64, 64, (0..64 * 64).map(|i| if i % 64 < 32 { 0 } else { 255 }).collect()));
        let img = setup(&s, &s, &art);
        let px = |x: usize, y: usize| img[(y * 256 + x) * 4];
        assert_eq!(px(70, 128), 0, "left is black");
        assert_eq!(px(186, 128), 255, "right is white");
    }

    /// The live preview in discRenderer.ts skips the spiral: with one geometry on both
    /// sides, tracing a point to its channel bit and back lands where it started, so each
    /// layer is just turned, moved and scaled onto the disc. This pins that down.
    #[test]
    fn same_geometry_is_a_plain_placement() {
        let s = Spiral::new(reference_preset().spiral_params()).unwrap();
        let picture = Arc::new(Picture::from_luma(64, 48, (0..64 * 48).map(|i| ((i * 37) % 251) as u8).collect()));
        let layer = Layer { path: String::new(), x: 0.12, y: -0.08, size: 0.9, rotation_deg: 40.0, invert: false, contrast: 0.0 };
        let art = Composition::new([(layer, picture)]);
        let p = PreviewInput { design: &s, actual: &s, art: &art, min_luma: 0.0, size: 0 };
        let ro2 = 2.0 * s.params.ro;
        for i in 0..400 {
            let r = s.params.ri + 0.2 + (s.params.ro - s.params.ri - 0.4) * (i as f64 / 399.0);
            let phi = i as f64 * 2.399;
            let traced = trace(&p, r, phi).unwrap();
            let direct = art.sample(r * phi.cos() / ro2, r * phi.sin() / ro2);
            assert!((traced - direct).abs() < 2.0, "r={r} phi={phi}: {traced} vs {direct}");
        }
    }

    #[test]
    fn wrong_stamper_distorts() {
        let s = Spiral::new(reference_preset().spiral_params()).unwrap();
        let art = whole(Picture::from_luma(2, 2, vec![0, 255, 0, 255]));
        let good = setup(&s, &s, &art);
        let other = Spiral::new(crate::spiral::SpiralParams { ro: s.params.ro + 0.02, ..s.params }).unwrap();
        let bad = setup(&s, &other, &art);
        let n = good.chunks(4).zip(bad.chunks(4)).filter(|(x, y)| x[3] == 255 && (x[0] as i32 - y[0] as i32).abs() > 60).count();
        assert!(n as f64 / (good.len() / 4) as f64 > 0.05);
    }
}
