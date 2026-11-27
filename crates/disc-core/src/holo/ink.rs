//! Draws a model for the disc: even and bright, with its features as dark lines.
//!
//! A cell of disc shows about seven brightness steps between its views at best, and haze eats
//! the dark ones, so soft grey shading arrives as mud. What survives is what is either bright
//! or dark: the model is lit almost flat, its texture kept for local contrast, and creases and
//! overlaps are inked in.

use super::model3d::{self, Mesh};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

type V3 = [f64; 3];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Style {
    /// How dark the inked lines are, 0 to 1.
    pub lines: f64,
    /// How much facing away from the light darkens, 0 to 1.
    pub shading: f64,
    /// How much of the texture shows, 0 to 1.
    pub texture: f64,
}

impl Default for Style {
    fn default() -> Self {
        Style { lines: 0.85, shading: 0.25, texture: 0.8 }
    }
}

/// Light from a little above and to the right of the viewer: enough to round things off.
const LIGHT: V3 = [0.25, 0.4, 0.88];
/// Depth two neighbours may differ by, beyond a plane, before it is an overlap, mm.
const STEP_MM: f64 = 0.7;
/// Cosine of the angle two neighbouring normals may differ by before it is a crease.
const CREASE: f32 = 0.62;
/// Radius texture is compared with its surroundings over, mm.
const LOCAL_MM: f64 = 2.0;

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Brightness 0..1 of placed meshes seen from `eye`, `n` square over the disc, disc frame. The
/// light stays with the viewer: it is turned by `angle` with them. Each mesh carries its own style.
pub fn draw(meshes: &[(Mesh, Style)], eye: V3, angle: f64, n: usize, inner_mm: f64, outer_mm: f64) -> Vec<f32> {
    draw_with(meshes, Some(eye), angle, n, inner_mm, outer_mm)
}

/// As [`draw`], seen face-on from infinitely far along `+z` (orthographic, 1:1 mm scale at any
/// depth). Viewing distance has no effect; overlaps are still inked by relative depth.
pub fn draw_ortho(meshes: &[(Mesh, Style)], angle: f64, n: usize, inner_mm: f64, outer_mm: f64) -> Vec<f32> {
    draw_with(meshes, None, angle, n, inner_mm, outer_mm)
}

fn draw_with(meshes: &[(Mesh, Style)], eye: Option<V3>, angle: f64, n: usize, inner_mm: f64, outer_mm: f64) -> Vec<f32> {
    const SS: usize = 2;
    let w = n * SS;
    let px_mm = 2.0 * outer_mm / w as f64;
    let empty = if eye.is_some() { 0.0f32 } else { f32::NEG_INFINITY };
    let mut depth = vec![empty; w * w]; // perspective: 1/distance (0 empty); ortho: z_mm (-inf empty)
    let mut normal = vec![[0.0f32; 3]; w * w];
    let mut albedo = vec![0.0f32; w * w];
    let mut owner = vec![0u32; w * w];
    let (mut x0, mut x1, mut y0, mut y1) = (w, 0usize, w, 0usize);
    for (mi, (m, _)) in meshes.iter().enumerate() {
        for tri in &m.tris {
            let mut p = [(0.0, 0.0, 0.0); 3];
            let mut ok = true;
            for (q, v) in p.iter_mut().zip(tri) {
                match eye {
                    Some(e) => match model3d::project_point(v.pos, e) {
                        Some((x, y, d)) => *q = ((x / (2.0 * outer_mm) + 0.5) * w as f64, (y / (2.0 * outer_mm) + 0.5) * w as f64, 1.0 / d),
                        None => ok = false,
                    },
                    None => {
                        *q = ((v.pos[0] / (2.0 * outer_mm) + 0.5) * w as f64, (v.pos[1] / (2.0 * outer_mm) + 0.5) * w as f64, v.pos[2]);
                    }
                }
            }
            let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0);
            if !ok || area.abs() < 1e-9 {
                continue;
            }
            let lim = |v: f64| v.clamp(0.0, w as f64 - 1.0) as usize;
            let (ax, bx) = (lim(p[0].0.min(p[1].0).min(p[2].0).floor()), lim(p[0].0.max(p[1].0).max(p[2].0).ceil()));
            let (ay, by) = (lim(p[0].1.min(p[1].1).min(p[2].1).floor()), lim(p[0].1.max(p[1].1).max(p[2].1).ceil()));
            let mut hit = false;
            for y in ay..=by {
                for x in ax..=bx {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let a = ((p[1].0 - px) * (p[2].1 - py) - (p[1].1 - py) * (p[2].0 - px)) / area;
                    let b = ((p[2].0 - px) * (p[0].1 - py) - (p[2].1 - py) * (p[0].0 - px)) / area;
                    let c = 1.0 - a - b;
                    if a < -1e-9 || b < -1e-9 || c < -1e-9 {
                        continue;
                    }
                    let d = a * p[0].2 + b * p[1].2 + c * p[2].2;
                    let i = y * w + x;
                    if d as f32 <= depth[i] {
                        continue;
                    }
                    hit = true;
                    depth[i] = d as f32;
                    owner[i] = mi as u32;
                    let k = if eye.is_some() { [a * p[0].2 / d, b * p[1].2 / d, c * p[2].2 / d] } else { [a, b, c] };
                    let mix = |f: &dyn Fn(usize) -> f64| k[0] * f(0) + k[1] * f(1) + k[2] * f(2);
                    let mut nn = [0, 1, 2].map(|c| mix(&|v| tri[v].normal[c]));
                    let facing = match eye {
                        Some(e) => {
                            let at = [0, 1, 2].map(|c| mix(&|v| tri[v].pos[c]));
                            dot(nn, [e[0] - at[0], e[1] - at[1], e[2] - at[2]])
                        }
                        None => nn[2],
                    };
                    let l = dot(nn, nn).sqrt().max(1e-9) * if facing < 0.0 { -1.0 } else { 1.0 };
                    nn = nn.map(|v| v / l);
                    normal[i] = nn.map(|v| v as f32);
                    albedo[i] = mix(&|v| tri[v].albedo as f64) as f32;
                }
            }
            if hit {
                (x0, x1, y0, y1) = (x0.min(ax), x1.max(bx), y0.min(ay), y1.max(by));
            }
        }
    }
    let mut out = vec![0.0f32; n * n];
    if x0 > x1 {
        return out;
    }
    let near = surroundings(&albedo, &depth, empty, w, (x0, x1, y0, y1), (LOCAL_MM / px_mm) as usize);
    let light = super::turntable::turn(model3d::scene_to_disc(LIGHT), angle);
    let ll = dot(light, light).sqrt();
    let value = |x: usize, y: usize| -> f32 {
        let i = y * w + x;
        if depth[i] <= empty {
            return 0.0;
        }
        let style = &meshes[owner[i] as usize].1;
        let z = |x: usize, y: usize| {
            let d = depth[y * w + x];
            if d > empty {
                Some(if eye.is_some() { 1.0 / d as f64 } else { -(d as f64) })
            } else {
                None
            }
        };
        let mut ink = 0.0f64;
        if style.lines > 0.0 {
            const R: usize = 2;
            for (dx, dy) in [(R, 0), (0, R)] {
                if x < dx || y < dy || x + dx >= w || y + dy >= w {
                    continue;
                }
                if let (Some(a), Some(b), Some(c)) = (z(x - dx, y - dy), z(x, y), z(x + dx, y + dy)) {
                    // Only the far side is inked, so a line does not eat the nearer shape.
                    let bend = b - (a + c) / 2.0;
                    ink = ink.max(((bend - STEP_MM * 0.5) / (STEP_MM * 0.5)).clamp(0.0, 1.0));
                    let (na, nc) = (normal[(y - dy) * w + x - dx], normal[(y + dy) * w + x + dx]);
                    let cos = na[0] * nc[0] + na[1] * nc[1] + na[2] * nc[2];
                    ink = ink.max((((CREASE - cos) / 0.4) as f64).clamp(0.0, 1.0) * 0.8);
                }
            }
        }
        let nl = (dot(normal[i].map(|v| v as f64), light) / ll).max(0.0);
        let shade = 1.0 - style.shading * (1.0 - nl);
        // Against its surroundings more than outright: dark hair is not a hole.
        let local = (albedo[i] as f64 / near[i].max(0.02) as f64).clamp(0.0, 1.15) / 1.15 * 1.1;
        let tone = 1.0 - style.texture + style.texture * (0.7 * local.min(1.0) + 0.3 * (albedo[i] as f64).sqrt());
        (shade * tone * (1.0 - style.lines * ink)) as f32
    };
    out.par_chunks_mut(n).enumerate().for_each(|(cy, row)| {
        if cy * SS > y1 || cy * SS + SS < y0 {
            return;
        }
        for (cx, o) in row.iter_mut().enumerate() {
            if cx * SS > x1 || cx * SS + SS < x0 {
                continue;
            }
            let at = |c: usize| ((c as f64 + 0.5) / n as f64 - 0.5) * 2.0 * outer_mm;
            let mask = model3d::radial_mask(at(cx), at(cy), inner_mm, outer_mm) as f32;
            if mask > 0.0 {
                let mut s = 0.0;
                for dy in 0..SS {
                    for dx in 0..SS {
                        s += value(cx * SS + dx, cy * SS + dy);
                    }
                }
                *o = s / (SS * SS) as f32 * mask;
            }
        }
    });
    out
}

/// Mean of `v` over the covered pixels within `r`, two box passes, inside a window.
fn surroundings(v: &[f32], cover: &[f32], empty: f32, w: usize, (x0, x1, y0, y1): (usize, usize, usize, usize), r: usize) -> Vec<f32> {
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut sum: Vec<f32> = Vec::with_capacity(cw * ch);
    let mut count: Vec<f32> = Vec::with_capacity(cw * ch);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let c = cover[y * w + x] > empty;
            sum.push(if c { v[y * w + x] } else { 0.0 });
            count.push(c as u8 as f32);
        }
    }
    let blur = |buf: &mut Vec<f32>| {
        for _ in 0..2 {
            for (len, lines, step, stride) in [(cw, ch, 1, cw), (ch, cw, cw, 1)] {
                let mut line = vec![0.0f64; len + 1];
                for l in 0..lines {
                    for i in 0..len {
                        line[i + 1] = line[i] + buf[l * stride + i * step] as f64;
                    }
                    for i in 0..len {
                        let (a, b) = (i.saturating_sub(r), (i + r + 1).min(len));
                        buf[l * stride + i * step] = ((line[b] - line[a]) / (2 * r + 1) as f64) as f32;
                    }
                }
            }
        }
    };
    blur(&mut sum);
    blur(&mut count);
    let mut out = vec![0.0f32; w * w];
    for y in 0..ch {
        for x in 0..cw {
            out[(y + y0) * w + x + x0] = sum[y * cw + x] / count[y * cw + x].max(1e-6);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holo::model3d::Vertex;

    fn quad(x: [f64; 2], y: [f64; 2], z: f64, albedo: f32) -> Mesh {
        let v = |x, y| Vertex { pos: [x, y, z], normal: [0.0, 0.0, 1.0], albedo };
        Mesh { tris: vec![[v(x[0], y[0]), v(x[1], y[0]), v(x[1], y[1])], [v(x[0], y[0]), v(x[1], y[1]), v(x[0], y[1])]] }
    }

    fn at(p: &[f32], n: usize, x: f64, y: f64) -> f32 {
        let c = |v: f64| ((v / 116.0 + 0.5) * n as f64) as usize;
        p[c(y) * n + c(x)]
    }

    #[test]
    fn faces_are_even_and_overlaps_are_inked() {
        let n = 512;
        let s = Style::default();
        let p = draw(&[(quad([-12.0, 12.0], [-52.0, -30.0], 0.0, 1.0), s), (quad([-4.0, 4.0], [-46.0, -36.0], 4.0, 1.0), s)], [0.0, 0.0, 350.0], 0.0, n, 24.0, 58.0);
        assert!(at(&p, n, 0.0, -41.0) > 0.85 && at(&p, n, 9.0, -41.0) > 0.85);
        let darkest = (0..40).map(|i| at(&p, n, 3.0 + i as f64 * 0.06, -41.0)).fold(1.0f32, f32::min);
        assert!(darkest < 0.5, "a line where one stands before the other: {darkest}");
        assert_eq!(at(&p, n, 30.0, -30.0), 0.0);
    }

    #[test]
    fn texture_shows_against_its_surroundings() {
        let n = 512;
        let style = Style { lines: 0.0, ..Style::default() };
        // A dark spot on a bright face shows; a face dark all over is still bright.
        let spot = draw(&[(quad([-12.0, 12.0], [-52.0, -30.0], 0.0, 0.8), style), (quad([-0.6, 0.6], [-41.6, -40.4], 0.01, 0.1), style)], [0.0, 0.0, 350.0], 0.0, n, 24.0, 58.0);
        let dark = draw(&[(quad([-12.0, 12.0], [-52.0, -30.0], 0.0, 0.1), style)], [0.0, 0.0, 350.0], 0.0, n, 24.0, 58.0);
        assert!(at(&spot, n, 0.0, -41.0) < 0.6 * at(&spot, n, 6.0, -41.0));
        assert!(at(&dark, n, 0.0, -41.0) > 0.5);
    }

    #[test]
    fn each_mesh_uses_its_own_style() {
        let n = 512;
        let inked = Style { lines: 1.0, shading: 0.0, texture: 0.0 };
        let plain = Style { lines: 0.0, shading: 0.0, texture: 0.0 };
        // Far quad with lines=0 should not receive an overlap ink line from the nearer quad.
        let p = draw(&[(quad([-12.0, 12.0], [-52.0, -30.0], 0.0, 1.0), plain), (quad([-4.0, 4.0], [-46.0, -36.0], 4.0, 1.0), inked)], [0.0, 0.0, 350.0], 0.0, n, 24.0, 58.0);
        let darkest = (0..40).map(|i| at(&p, n, 3.0 + i as f64 * 0.06, -41.0)).fold(1.0f32, f32::min);
        assert!(darkest > 0.9, "lines=0 mesh should stay un-inked: {darkest}");
    }

    #[test]
    fn ortho_matches_depth_zero_and_inks_overlaps() {
        let n = 512;
        let s = Style::default();
        // Flat quad at z = 0 matches perspective draw from [0, 0, 350] pixel-for-pixel.
        let q0 = [(quad([-12.0, 12.0], [-52.0, -30.0], 0.0, 1.0), s)];
        let persp = draw(&q0, [0.0, 0.0, 350.0], 0.0, n, 24.0, 58.0);
        let ortho = draw_ortho(&q0, 0.0, n, 24.0, 58.0);
        assert_eq!(ortho, persp, "depth-0 ortho must match perspective size and tone");
        // Overlapping quads still ink the far edge without perspective scaling.
        let p = draw_ortho(&[(quad([-12.0, 12.0], [-52.0, -30.0], 0.0, 1.0), s), (quad([-4.0, 4.0], [-46.0, -36.0], 4.0, 1.0), s)], 0.0, n, 24.0, 58.0);
        assert!(at(&p, n, 0.0, -41.0) > 0.85 && at(&p, n, 9.0, -41.0) > 0.85);
        let darkest = (0..40).map(|i| at(&p, n, 3.0 + i as f64 * 0.06, -41.0)).fold(1.0f32, f32::min);
        assert!(darkest < 0.6, "ortho overlap should be inked: {darkest}");
    }
}
