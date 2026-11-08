//! Physical preview: what the disc shows at human viewing scale.
//!
//! Each pixel on the `actual` disc is traced to the channel bit under it, then that bit
//! is looked up in the `design` geometry the image was carved for. With the same preset
//! on both sides the art comes back straight; with the wrong stamper it shears exactly as
//! a real burn would.

use crate::artwork::{to_field, GrayField};
use crate::spiral::Spiral;
use base64::Engine;
use rayon::prelude::*;
use std::f64::consts::TAU;
use std::io::Cursor;

pub struct PreviewInput<'a> {
    pub design: &'a Spiral,
    pub actual: &'a Spiral,
    pub field: &'a GrayField,
    pub rotation_rad: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    /// Image scale about the disc centre; 1 is the size the crop mode chose.
    pub scale: f64,
    /// Lowest luma the guard lets through, 0 when off.
    pub min_luma: f32,
    pub size: u32,
}

/// Luma at pixel-centre polar position. `None` outside the recorded band.
fn trace(p: &PreviewInput, r: f64, phi: f64, du: f64, dv: f64) -> Option<f32> {
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
    let (s, c) = (th - p.rotation_rad).sin_cos();
    let ro2 = 2.0 * p.design.params.ro;
    let (u, v) = to_field(rd * c / ro2, rd * s / ro2, du, dv, p.scale);
    Some(p.field.bilinear(u, v).max(p.min_luma))
}

/// RGBA pixels, transparent off the disc.
pub fn render(p: &PreviewInput) -> Vec<u8> {
    let size = p.size as usize;
    let view_r = p.actual.params.ro.max(p.design.params.ro) + 0.6;
    let mm_per_px = 2.0 * view_r / size as f64;
    let (rot_sin, rot_cos) = p.rotation_rad.sin_cos();
    let du = p.offset_x * rot_cos + p.offset_y * rot_sin;
    let dv = -p.offset_x * rot_sin + p.offset_y * rot_cos;
    let mut img = vec![0u8; size * size * 4];
    img.par_chunks_mut(size * 4).enumerate().for_each(|(y, row)| {
        let dy = (y as f64 + 0.5 - size as f64 / 2.0) * mm_per_px;
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let dx = (x as f64 + 0.5 - size as f64 / 2.0) * mm_per_px;
            let r = dx.hypot(dy);
            let phi = dy.atan2(dx);
            let (v, a) = match trace(p, r, phi, du, dv) {
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
    use crate::preset::reference_preset;

    /// Half black, half white across x, so edge position is easy to measure.
    fn split_field() -> GrayField {
        let s = 256;
        GrayField { size: s, data: (0..s * s).map(|i| if i % s < s / 2 { 0 } else { 255 }).collect() }
    }

    fn setup(design: &Spiral, actual: &Spiral, field: &GrayField) -> Vec<u8> {
        render(&PreviewInput { design, actual, field, rotation_rad: 0.0, offset_x: 0.0, offset_y: 0.0, scale: 1.0, min_luma: 0.0, size: 256 })
    }

    #[test]
    fn offset_shifts_the_art() {
        let s = Spiral::new(reference_preset().spiral_params()).unwrap();
        let f = split_field();
        // Shift image right by 20% (0.2)
        let img = render(&PreviewInput { design: &s, actual: &s, field: &f, rotation_rad: 0.0, offset_x: 0.2, offset_y: 0.0, scale: 1.0, min_luma: 0.0, size: 256 });
        let px = |x: usize, y: usize| img[(y * 256 + x) * 4];
        // The dividing line at center (x=128) should now have shifted rightwards, so x=140 is still black
        assert_eq!(px(70, 128), 0);
        assert_eq!(px(140, 128), 0, "center shifted rightwards");
    }

    fn mismatch(a: &[u8], b: &[u8]) -> f64 {
        let n = a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x[3] == 255 && (x[0] as i32 - y[0] as i32).abs() > 60).count();
        n as f64 / (a.len() / 4) as f64
    }

    #[test]
    fn same_geometry_reproduces_the_art() {
        let s = Spiral::new(reference_preset().spiral_params()).unwrap();
        let f = split_field();
        let img = setup(&s, &s, &f);
        let px = |x: usize, y: usize| img[(y * 256 + x) * 4];
        assert_eq!(px(70, 128), 0, "left is black");
        assert_eq!(px(186, 128), 255, "right is white");
    }

    /// The live preview in discRenderer.ts skips the spiral: with one geometry on both
    /// sides, tracing a point to its channel bit and back lands where it started, so the
    /// picture is just rotated, shifted and scaled onto the disc. This pins that down.
    #[test]
    fn same_geometry_is_a_plain_placement() {
        let s = Spiral::new(reference_preset().spiral_params()).unwrap();
        let field = GrayField { size: 64, data: (0..64 * 64).map(|i| ((i * 37) % 251) as u8).collect() };
        let (rot, ox, oy, k): (f64, f64, f64, f64) = (0.7, 0.12, -0.08, 1.3);
        let p = PreviewInput { design: &s, actual: &s, field: &field, rotation_rad: rot, offset_x: ox, offset_y: oy, scale: k, min_luma: 0.0, size: 0 };
        let (du, dv) = (ox * rot.cos() + oy * rot.sin(), -ox * rot.sin() + oy * rot.cos());
        let ro2 = 2.0 * s.params.ro;
        let mut checked = 0;
        for i in 0..400 {
            let r = s.params.ri + 0.2 + (s.params.ro - s.params.ri - 0.4) * (i as f64 / 399.0);
            let phi = i as f64 * 2.399;
            let traced = trace(&p, r, phi, du, dv).unwrap();
            // What the shader does: undo the rotation, then the shift and the scale.
            let (x, y) = (r * phi.cos() / ro2, r * phi.sin() / ro2);
            let (qx, qy) = (rot.cos() * x + rot.sin() * y, -rot.sin() * x + rot.cos() * y);
            let direct = field.bilinear((qx - du) / k + 0.5, (qy - dv) / k + 0.5);
            assert!((traced - direct).abs() < 2.0, "r={r} phi={phi}: {traced} vs {direct}");
            checked += 1;
        }
        assert_eq!(checked, 400);
    }

    #[test]
    fn wrong_stamper_distorts() {
        let s = Spiral::new(reference_preset().spiral_params()).unwrap();
        let f = split_field();
        let good = setup(&s, &s, &f);
        let other = Spiral::new(crate::spiral::SpiralParams { ro: s.params.ro + 0.02, ..s.params }).unwrap();
        assert!(mismatch(&good, &setup(&s, &other, &f)) > 0.05);
    }
}
