//! Pictures: carved flat as the carve page carves them, each inside its position's kept area,
//! laid out in the position's own frame, and only where it covers the disc.
//!
//! Disc points here are the carve page's: a share of the diameter from the centre, y down (see
//! `artwork`). That is the solve grid's frame too, so a picture on a position at 0° burns
//! exactly as the same layer does on the carve page.

use super::model3d::{Position, Scene};
use super::turntable::{items, turn, Item};
use crate::artwork::{Composition, Layer};
use crate::preset::DiscGeometryPreset;
use std::sync::{Arc, Mutex};

/// Which picture owns each cell of the solve grid.
#[derive(Clone, Debug, Default)]
pub struct FlatOwners {
    pub grid: usize,
    /// 0 where none does, otherwise 1 + an index into `pictures`.
    pub owner: Vec<u8>,
    /// The pictures: id, and their position's turn in radians.
    pub pictures: Vec<(String, f64)>,
}

impl FlatOwners {
    /// The picture owning the disc point (`x`, `y`), if any.
    #[inline]
    pub fn at(&self, x: f64, y: f64) -> Option<usize> {
        let (u, v) = (x + 0.5, y + 0.5);
        if self.owner.is_empty() || !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        let n = self.grid;
        match self.owner[(v * n as f64) as usize * n + (u * n as f64) as usize] {
            0 => None,
            o => Some(o as usize - 1),
        }
    }
}

/// One picture, turned with its position.
pub struct Picture {
    pub id: String,
    /// Its position's index.
    pub at: usize,
    /// Its place in its position's list.
    pub rank: usize,
    pub phi_rad: f64,
    picture: Arc<Composition>,
}

impl Picture {
    /// Grey at the disc point (`x`, `y`) where the picture covers it; none where it leaves the
    /// disc bare, which then stays as it would be without it.
    #[inline]
    pub fn covered(&self, x: f64, y: f64) -> Option<f32> {
        let [lx, ly, _] = turn([x, y, 0.0], -self.phi_rad);
        self.picture.sample_covered(lx, ly)
    }
}

/// Every position's pictures, read with their long edge cut to `max_edge` when given.
pub fn pictures(scene: &Scene, positions: &[Position], max_edge: Option<u32>) -> Result<Vec<Picture>, String> {
    let mut out = vec![];
    for (at, pos) in positions.iter().enumerate() {
        for (rank, it) in items(scene, positions, at).into_iter().enumerate() {
            if let Item::Picture(p) = it {
                let picture = composition(std::slice::from_ref(&p.layer), max_edge)?;
                out.push(Picture { id: p.id.clone(), at, rank, phi_rad: pos.phi_deg.to_radians(), picture });
            }
        }
    }
    Ok(out)
}

/// Whether the disc point (`x`, `y`), in mm on the solve grid, is in the kept area of a position
/// turned by `phi_rad` that keeps `crop_deg` either side of its axis (`turntable::solve`'s test).
#[inline]
pub fn kept(x: f64, y: f64, phi_rad: f64, crop_deg: f64) -> bool {
    if crop_deg >= 90.0 {
        return true;
    }
    let at = super::model3d::scene_to_disc(turn([x, y, 0.0], -phi_rad));
    at[0].abs().atan2(at[1].abs()) <= crop_deg.to_radians()
}

/// Whether a grid point in mm lies on the written ring of the disc.
#[inline]
pub fn on_ring(x: f64, y: f64, preset: &DiscGeometryPreset) -> bool {
    let r = x.hypot(y);
    r >= preset.inner_radius_mm && r <= preset.outer_radius_mm
}

/// The pictures where they own the disc, ready to sample.
pub struct Flat {
    pub owners: Arc<FlatOwners>,
    parts: Vec<Arc<Composition>>,
}

impl Flat {
    /// Reads each owning picture, its long edge cut to `max_edge` when given.
    pub fn load(owners: Arc<FlatOwners>, scene: &Scene, max_edge: Option<u32>) -> Result<Self, String> {
        let parts = owners
            .pictures
            .iter()
            .map(|(id, _)| {
                let image = scene.images.iter().find(|i| &i.id == id).ok_or("图片已不在设计里")?;
                composition(std::slice::from_ref(&image.layer), max_edge)
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { owners, parts })
    }

    /// Grey, 0 ..= 255, at the disc point (`x`, `y`), or none where no picture is drawn there.
    #[inline]
    pub fn sample(&self, x: f64, y: f64) -> Option<f32> {
        let p = self.owners.at(x, y)?;
        let [lx, ly, _] = turn([x, y, 0.0], -self.owners.pictures[p].1);
        self.parts[p].sample_covered(lx, ly)
    }

    /// One RGBA texel per solve-grid cell for the preview: the share of light bytes `tone` writes
    /// there, and 255 where a flat picture is drawn.
    pub fn preview_layer(&self, tone: crate::carver::Tone) -> Vec<u8> {
        let n = self.owners.grid;
        let mut out = vec![0u8; n * n * 4];
        for (c, px) in out.chunks_exact_mut(4).enumerate() {
            let x = ((c % n) as f64 + 0.5) / n as f64 - 0.5;
            let y = ((c / n) as f64 + 0.5) / n as f64 - 0.5;
            if let Some(g) = self.sample(x, y) {
                let light = if tone.servo_safe { g.max(tone.min_luma) } else if g >= 128.0 { 255.0 } else { 0.0 };
                px[0] = light.round().clamp(0.0, 255.0) as u8;
                px[1] = 255;
            }
        }
        out
    }
}

/// A composition of `layers`, kept for the next ask while the files are unchanged: the preview
/// asks again on every edit, and reading pictures is the slow part.
fn composition(layers: &[Layer], max_edge: Option<u32>) -> Result<Arc<Composition>, String> {
    const KEEP: usize = 64;
    static CACHE: Mutex<Vec<(String, Arc<Composition>)>> = Mutex::new(Vec::new());
    let modified: Vec<_> = layers.iter().map(|l| std::fs::metadata(&l.path).and_then(|m| m.modified()).ok()).collect();
    let key = serde_json::to_string(&(layers, &modified, max_edge)).map_err(|e| e.to_string())?;
    if let Some(c) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|c| c.0 == key) {
        return Ok(c.1.clone());
    }
    let c = Arc::new(Composition::load(layers, max_edge)?);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= KEEP {
        cache.remove(0);
    }
    cache.push((key, c.clone()));
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holo::design::{self, shared_codebook};
    use crate::holo::model3d::{self, Position, SceneImage, SceneObject, SCENE_VERSION};
    use crate::holo::turntable::{self, Options};
    use crate::preset::reference_preset;
    use std::sync::atomic::AtomicBool;

    /// A picture whose every spot differs: grey runs left to right, and a bar marks its top.
    fn picture(dir: &std::path::Path) -> String {
        let path = dir.join("p.png");
        let img = image::GrayImage::from_fn(64, 64, |x, y| image::Luma([if y < 8 { 0 } else { (x * 4) as u8 }]));
        img.save(&path).unwrap();
        path.to_string_lossy().into()
    }

    fn flat_pos(id: &str, phi_deg: f64, crop_deg: f64) -> Position {
        Position { id: id.into(), name: String::new(), phi_deg, crop_deg, neutral: None, order: vec![] }
    }

    fn image(path: &str, position: &str) -> SceneImage {
        SceneImage { id: "i".into(), name: "p".into(), width: 64, height: 64, position_id: position.into(), layer: Layer { path: path.into(), size: 0.8, ..Layer::plain() } }
    }

    fn solve(scene: &Scene) -> turntable::Solution {
        let shared = shared_codebook(0.1333);
        turntable::solve(scene, &Options::default(), &shared.0, &shared.1, &reference_preset()).unwrap()
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("holo-flat-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_flat_picture_reads_as_on_the_carve_page_turned_with_its_position() {
        let d = dir("frame");
        let path = picture(&d);
        let carve = Composition::load(&[Layer { path: path.clone(), size: 0.8, ..Layer::plain() }], None).unwrap();
        for phi_deg in [0.0f64, 90.0, 215.0] {
            let scene = Scene { version: SCENE_VERSION, objects: vec![], positions: vec![flat_pos("f", phi_deg, 30.0)], lamp_mm: None, images: vec![image(&path, "f")] };
            let s = solve(&scene);
            assert!(s.overlapping.is_empty());
            let f = Flat::load(s.flat.clone(), &scene, None).unwrap();
            // Up the position's axis, and square across it, in its own frame.
            let up = turn([0.0, -0.3, 0.0], phi_deg.to_radians());
            let across = turn([0.3, 0.0, 0.0], phi_deg.to_radians());
            assert!(f.sample(up[0], up[1]).is_some(), "phi {phi_deg}: the axis is kept");
            assert!(f.sample(across[0], across[1]).is_none(), "phi {phi_deg}: square across is not");
            // Where it is kept it reads the picture as the carve page lays it, turned with the position.
            let mut checked = 0;
            for i in 0..400 {
                let (x, y) = ((i % 20) as f64 / 20.0 - 0.5, (i / 20) as f64 / 20.0 - 0.5);
                if let Some(g) = f.sample(x, y) {
                    let [lx, ly, _] = turn([x, y, 0.0], -phi_deg.to_radians());
                    assert_eq!(g, carve.sample(lx, ly));
                    checked += 1;
                }
            }
            assert!(checked > 10, "phi {phi_deg}: {checked}");
        }
        // At 0° the top bar lies toward 12 o'clock, as the carve page shows it.
        let scene = Scene { version: SCENE_VERSION, objects: vec![], positions: vec![flat_pos("f", 0.0, 30.0)], lamp_mm: None, images: vec![image(&path, "f")] };
        let f = Flat::load(solve(&scene).flat, &scene, None).unwrap();
        assert_eq!(f.sample(0.0, -0.38), Some(0.0));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn bare_disc_around_a_picture_is_left_alone() {
        let d = dir("bare");
        let path = picture(&d);
        let small = SceneImage { layer: Layer { path: path.clone(), y: -0.3, size: 0.2, ..Layer::plain() }, ..image(&path, "f") };
        let scene = Scene { version: SCENE_VERSION, objects: vec![], positions: vec![flat_pos("f", 0.0, 90.0)], lamp_mm: None, images: vec![small] };
        let s = solve(&scene);
        let f = Flat::load(s.flat.clone(), &scene, None).unwrap();
        assert!(f.sample(0.0, -0.3).is_some(), "under the picture");
        assert!(f.sample(0.0, 0.3).is_none(), "the other end of the kept area is bare");
        assert!(f.sample(0.3, -0.05).is_none(), "beside the picture is bare");
        let owned = s.flat.owner.iter().filter(|&&o| o != 0).count();
        assert!(owned > 0 && owned < s.flat.owner.len() / 20, "only what the picture covers: {owned}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn models_and_pictures_on_the_same_disc_are_told_to_overlap() {
        let d = dir("clash");
        let path = picture(&d);
        let star = SceneObject {
            id: "s".into(), path: model3d::BUILTIN_SAMPLE_PATH.into(), name: "s".into(), visible: true, position_mm: [0.0, 40.0, 6.0],
            rotation_deg: [0.0; 3], scale_mm: 22.0, position_id: "r".into(), style: Default::default(), motion: Default::default(),
            mode: Default::default(),
            parallax: 2.0,
            tilt_deg: 0.0,
        };
        let real = flat_pos("r", 0.0, 30.0);
        let scene = |phi: f64| Scene { version: SCENE_VERSION, objects: vec![star.clone()], positions: vec![real.clone(), flat_pos("f", phi, 30.0)], lamp_mm: None, images: vec![image(&path, "f")] };
        let mut both = solve(&scene(0.0)).overlapping;
        both.sort();
        assert_eq!(both, ["f", "r"]);
        assert!(solve(&scene(90.0)).overlapping.is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_disc_of_only_pictures_burns_them_as_dark_and_light_bytes() {
        let d = dir("burn");
        let path = picture(&d);
        let scene = Scene { version: SCENE_VERSION, objects: vec![], positions: vec![flat_pos("f", 0.0, 90.0)], lamp_mm: None, images: vec![image(&path, "f")] };
        let total = 16 * 400;
        let preset = crate::preset::DiscGeometryPreset { total_sectors: total, ..reference_preset() };
        let shared = shared_codebook(0.1333);
        let s = turntable::solve(&scene, &Options::default(), &shared.0, &shared.1, &preset).unwrap();
        let f = Flat::load(s.flat.clone(), &scene, None).unwrap();
        let tone = crate::carver::Tone::from_duty(true, 0.65);
        let iso = d.join("f.iso");
        design::build_iso(&preset, &shared.0, &s.solved, Some((&f, tone)), iso.to_str().unwrap(), &AtomicBool::new(false), &|_| {}).unwrap();
        let bytes = std::fs::read(&iso).unwrap();
        // Undo the scrambler, and every run on a flat spot holds only the carve page's two bytes.
        let lut = crate::ecma267::build_scrambler_lut();
        let spiral = crate::spiral::Spiral::new(preset.spiral_params()).unwrap();
        let (mut carved, mut other) = (0, 0);
        for lba in (total / 2..total / 2 + 64).filter(|&l| !crate::iso9660::is_system_sector(l, total)) {
            let mut sector = bytes[lba as usize * crate::ecma267::SECTOR_BYTES..][..crate::ecma267::SECTOR_BYTES].to_vec();
            crate::ecma267::xor_sector(&lut, lba, &mut sector);
            crate::carver::for_each_sample(&spiral, lba, |r, x, y| {
                let run = crate::carver::run_of(&mut sector, r);
                if f.owners.at(x, y).is_some() {
                    assert!(run.iter().all(|&b| b == crate::ecma267::DARK_BYTE || b == crate::ecma267::LIGHT_BYTE), "lba {lba} run {r}: {run:?}");
                    carved += 1;
                } else {
                    other += 1;
                }
            });
        }
        assert!(carved > 1000, "carved {carved}, other {other}");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
