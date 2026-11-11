//! ISO synthesis: spiral sampling, servo-safe dithering, scrambler XOR.

use crate::artwork::{Composition, Layer};
use crate::ecma267::*;
use crate::preset::DiscGeometryPreset;
use crate::spiral::Spiral;
use memmap2::{Mmap, MmapMut};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// Luma is re-sampled every this many bytes, about 76 channel bits or 10 µm of track.
/// Source pixels are far larger, so sampling each byte would only burn time.
const SAMPLE_EVERY_BYTES: usize = 4;
/// Highest single-sector dark share a finished image may contain.
pub const MAX_SECTOR_DARK_LIMIT: f64 = 0.70;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarveJobConfig {
    /// Bottom first.
    pub layers: Vec<Layer>,
    pub output_iso_path: String,
    pub preset: DiscGeometryPreset,
    pub servo_safe_dither: bool,
    pub max_dark_duty_cycle: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Tone {
    pub servo_safe: bool,
    /// Luma floor in 0..=255, only used when `servo_safe` is set.
    pub min_luma: f32,
}

impl Tone {
    pub fn from_duty(servo_safe: bool, duty: f64) -> Self {
        let duty = duty.clamp(0.50, 0.85);
        Self { servo_safe, min_luma: (255.0 * (1.0 - duty)).round() as f32 }
    }
}

pub struct CarveCtx<'a> {
    pub spiral: &'a Spiral,
    pub art: &'a Composition,
    pub lut: &'a ScramblerLut,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BlockStats {
    pub dark_bytes: u64,
    pub max_sector_dark: u32,
    pub max_dark_run: u32,
}

/// Low-discrepancy start phase, so neighbouring blocks do not line their dither up
/// into radial streaks. Deterministic, which keeps builds reproducible.
#[inline]
fn block_phase(block: u32) -> f32 {
    let f = (block as f64 * 0.618_033_988_749_894_9).fract();
    (f - 0.5) as f32
}

/// Fill `out` with the wanted scrambled bytes of one sector, before the XOR.
/// `acc` carries the 1D error-diffusion state along the track.
fn sector_bytes(ctx: &CarveCtx, lba: u32, acc: &mut f32, out: &mut [u8]) {
    let bits = SECTOR_CHANNEL_BITS as f64;
    let half_step = 0.5 / SECTOR_BYTES as f64;
    let (r0, th0) = ctx.spiral.locate((lba as f64 + half_step) * bits);
    let (r1, th1) = ctx.spiral.locate((lba as f64 + 1.0 - half_step) * bits);
    let chunks = SECTOR_BYTES / SAMPLE_EVERY_BYTES;
    let (dr, dth) = ((r1 - r0) / (chunks - 1) as f64, (th1 - th0) / (chunks - 1) as f64);
    let ro2 = 2.0 * ctx.spiral.params.ro;
    // Phasor stepping: the angle moves a fraction of a radian per sector, and this loop
    // runs billions of times on a full disc, so no sin/cos per sample.
    let (mut s, mut c) = th0.sin_cos();
    let (ds, dc) = dth.sin_cos();
    let Tone { servo_safe, min_luma } = ctx.tone;
    for (i, chunk) in out.chunks_exact_mut(SAMPLE_EVERY_BYTES).enumerate() {
        let r = r0 + i as f64 * dr;
        let grey = ctx.art.sample(r * c / ro2, r * s / ro2);
        if servo_safe {
            let p = grey.max(min_luma) / 255.0;
            for b in chunk {
                *acc += p;
                let white = *acc >= 0.5;
                if white {
                    *acc -= 1.0;
                }
                *b = if white { LIGHT_BYTE } else { DARK_BYTE };
            }
        } else {
            let q = if grey >= 128.0 { LIGHT_BYTE } else { DARK_BYTE };
            chunk.fill(q);
        }
        let (ns, nc) = (s * dc + c * ds, c * dc - s * ds);
        s = ns;
        c = nc;
    }
}

/// Carve the ECC block `block` into a 32 KiB buffer, ready to write.
pub fn carve_block(ctx: &CarveCtx, block: u32, out: &mut [u8]) -> BlockStats {
    let mut st = BlockStats::default();
    let mut acc = block_phase(block);
    let mut run = 0u32;
    for (i, sector) in out.chunks_exact_mut(SECTOR_BYTES).enumerate() {
        let lba = block * ECC_BLOCK_SECTORS as u32 + i as u32;
        sector_bytes(ctx, lba, &mut acc, sector);
        let mut dark = 0u32;
        for &b in sector.iter() {
            if b == DARK_BYTE {
                dark += 1;
                run += 1;
                st.max_dark_run = st.max_dark_run.max(run);
            } else {
                run = 0;
            }
        }
        st.dark_bytes += dark as u64;
        st.max_sector_dark = st.max_sector_dark.max(dark);
        xor_sector(ctx.lut, lba, sector);
    }
    st
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarveProgress {
    pub percent: f64,
    pub mb_per_s: f64,
    pub max_sector_dark_ratio: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarveJobSummary {
    pub output_iso_path: String,
    pub total_sectors: u32,
    pub bytes: u64,
    pub elapsed_seconds: f64,
    pub mb_per_s: f64,
    pub mean_dark_ratio: f64,
    pub max_sector_dark_ratio: f64,
    pub max_dark_run_bytes: u32,
    pub servo_safe: bool,
    pub limit_ok: bool,
}

fn free_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        Some(s.f_bavail as u64 * s.f_frsize as u64)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(free)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// Create an output file of exactly this size, after making sure the disk can hold it.
pub(crate) fn create_sized(path: &Path, bytes: u64) -> Result<std::fs::File, String> {
    if let Some(free) = free_space(path) {
        if free < bytes {
            return Err(format!("磁盘空间不足：需要 {:.2} GB，只剩 {:.2} GB", bytes as f64 / 1e9, free as f64 / 1e9));
        }
    }
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| format!("无法创建 {}：{e}", path.display()))?;
    f.set_len(bytes).map_err(|e| e.to_string())?;
    Ok(f)
}

/// Map a fresh output file, refusing up front if it cannot fit: writing past a full
/// disk through a mapping would kill the process.
pub(crate) fn create_mapped(path: &Path, bytes: u64) -> Result<MmapMut, String> {
    let f = create_sized(path, bytes)?;
    unsafe { MmapMut::map_mut(&f) }.map_err(|e| e.to_string())
}

pub fn build_iso(
    cfg: &CarveJobConfig,
    cancel: &AtomicBool,
    progress: &(dyn Fn(CarveProgress) + Sync),
) -> Result<CarveJobSummary, String> {
    let spiral = Spiral::new(cfg.preset.spiral_params())?;
    if cfg.layers.is_empty() {
        return Err("没有图像".into());
    }
    let art = Composition::load(&cfg.layers, None)?;
    build_iso_from(cfg, &spiral, &art, cancel, progress)
}

pub fn build_iso_from(
    cfg: &CarveJobConfig,
    spiral: &Spiral,
    art: &Composition,
    cancel: &AtomicBool,
    progress: &(dyn Fn(CarveProgress) + Sync),
) -> Result<CarveJobSummary, String> {
    let total = cfg.preset.total_sectors;
    if total % ECC_BLOCK_SECTORS as u32 != 0 {
        return Err("总扇区数必须是 16 的倍数".into());
    }
    let lut = build_scrambler_lut();
    let tone = Tone::from_duty(cfg.servo_safe_dither, cfg.max_dark_duty_cycle);
    let ctx = CarveCtx { spiral, art, lut: &lut, tone };
    let bytes = total as u64 * SECTOR_BYTES as u64;
    let path = Path::new(&cfg.output_iso_path);
    let mut map = create_mapped(path, bytes)?;

    let start = Instant::now();
    let done = AtomicU64::new(0);
    let dark = AtomicU64::new(0);
    let max_dark = AtomicU64::new(0);
    let max_run = AtomicU64::new(0);
    let blocks = total as u64 / ECC_BLOCK_SECTORS as u64;
    let tick = (blocks / 200).max(1);

    let run = map.par_chunks_mut(ECC_BLOCK_BYTES).enumerate().try_for_each(|(b, out)| {
        if cancel.load(Ordering::Relaxed) {
            return Err(());
        }
        let st = carve_block(&ctx, b as u32, out);
        dark.fetch_add(st.dark_bytes, Ordering::Relaxed);
        max_dark.fetch_max(st.max_sector_dark as u64, Ordering::Relaxed);
        max_run.fetch_max(st.max_dark_run as u64, Ordering::Relaxed);
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % tick == 0 {
            let dt = start.elapsed().as_secs_f64().max(1e-6);
            progress(CarveProgress {
                percent: d as f64 / blocks as f64 * 100.0,
                mb_per_s: d as f64 * ECC_BLOCK_BYTES as f64 / 1e6 / dt,
                max_sector_dark_ratio: max_dark.load(Ordering::Relaxed) as f64 / SECTOR_BYTES as f64,
            });
        }
        Ok(())
    });
    if run.is_err() {
        drop(map);
        let _ = std::fs::remove_file(path);
        return Err("已取消".into());
    }
    map.flush().map_err(|e| e.to_string())?;
    let dt = start.elapsed().as_secs_f64().max(1e-6);
    let max_ratio = max_dark.load(Ordering::Relaxed) as f64 / SECTOR_BYTES as f64;
    Ok(CarveJobSummary {
        output_iso_path: cfg.output_iso_path.clone(),
        total_sectors: total,
        bytes,
        elapsed_seconds: dt,
        mb_per_s: bytes as f64 / 1e6 / dt,
        mean_dark_ratio: dark.load(Ordering::Relaxed) as f64 / bytes as f64,
        max_sector_dark_ratio: max_ratio,
        max_dark_run_bytes: max_run.load(Ordering::Relaxed) as u32,
        servo_safe: cfg.servo_safe_dither,
        limit_ok: !cfg.servo_safe_dither || max_ratio <= MAX_SECTOR_DARK_LIMIT,
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsoAudit {
    pub sectors: u64,
    pub foreign_bytes: u64,
    pub max_sector_dark_ratio: f64,
    pub worst_lba: u32,
}

/// Read a finished image back from disk and measure it, independent of the build's
/// own counters. Foreign bytes mean the XOR or the file is wrong.
pub fn audit_iso(path: &Path) -> Result<IsoAudit, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let map = unsafe { Mmap::map(&f) }.map_err(|e| e.to_string())?;
    if map.len() % SECTOR_BYTES != 0 {
        return Err("镜像大小不是 2048 的倍数".into());
    }
    let lut = build_scrambler_lut();
    let (foreign, worst, worst_lba) = map
        .par_chunks(SECTOR_BYTES)
        .enumerate()
        .map(|(i, s)| {
            let table = &lut[seed_index(i as u32)];
            let (mut dark, mut foreign) = (0u32, 0u64);
            for (b, k) in s.iter().zip(table.iter()) {
                match b ^ k {
                    DARK_BYTE => dark += 1,
                    LIGHT_BYTE => {}
                    _ => foreign += 1,
                }
            }
            (foreign, dark, i as u32)
        })
        .reduce(|| (0, 0, 0), |a, b| (a.0 + b.0, a.1.max(b.1), if b.1 > a.1 { b.2 } else { a.2 }));
    Ok(IsoAudit {
        sectors: (map.len() / SECTOR_BYTES) as u64,
        foreign_bytes: foreign,
        max_sector_dark_ratio: worst as f64 / SECTOR_BYTES as f64,
        worst_lba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artwork::Picture;
    use crate::preset::reference_preset;
    use std::sync::Arc;

    fn ctx_parts(art: Composition) -> (Spiral, Composition, Box<ScramblerLut>) {
        (Spiral::new(reference_preset().spiral_params()).unwrap(), art, build_scrambler_lut())
    }

    /// A 4×4 grey picture covering the disc.
    fn flat(data: Vec<u8>) -> Composition {
        crate::artwork::tests::whole(Picture::from_luma(4, 4, data))
    }

    fn safe() -> Tone {
        Tone::from_duty(true, 0.65)
    }

    #[test]
    fn duty_floor_is_rounded_as_documented() {
        assert_eq!(Tone::from_duty(true, 0.65).min_luma, 89.0);
        assert_eq!(Tone::from_duty(true, 0.10).min_luma, 128.0, "clamped to 0.50");
    }

    /// The failure this layer exists for: a pure black sector must stay under the limit,
    /// and no long uninterrupted dark run may survive.
    #[test]
    fn offset_shifts_carved_pixels() {
        let half = Arc::new(Picture::from_luma(4, 4, vec![0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255]));
        let at = |x| Composition::new([(Layer { path: String::new(), x, y: 0.0, size: 1.0, rotation_deg: 0.0, invert: false, contrast: 0.0 }, half.clone())]);
        let (spiral, centred, lut) = ctx_parts(at(0.0));
        let shifted = at(0.2);
        let ctx_center = CarveCtx { spiral: &spiral, art: &centred, lut: &lut, tone: safe() };
        let ctx_shifted = CarveCtx { spiral: &spiral, art: &shifted, lut: &lut, tone: safe() };
        let mut buf1 = vec![0u8; ECC_BLOCK_BYTES];
        let mut buf2 = vec![0u8; ECC_BLOCK_BYTES];
        let st1 = carve_block(&ctx_center, 100_000, &mut buf1);
        let st2 = carve_block(&ctx_shifted, 100_000, &mut buf2);
        // The shifted version samples different pixels, leading to different block stats
        assert_ne!(st1.dark_bytes, st2.dark_bytes);
    }

    #[test]
    fn pure_black_stays_under_servo_limit() {
        let (spiral, field, lut) = ctx_parts(flat(vec![0; 16]));
        let ctx = CarveCtx { spiral: &spiral, art: &field, lut: &lut, tone: safe() };
        let mut buf = vec![0u8; ECC_BLOCK_BYTES];
        for block in [100_000u32, 100_001, 123_456] {
            let st = carve_block(&ctx, block, &mut buf);
            let ratio = st.max_sector_dark as f64 / 2048.0;
            assert!(ratio <= MAX_SECTOR_DARK_LIMIT, "{ratio}");
            assert!(ratio > 0.60, "{ratio}");
            assert!(st.max_dark_run <= 3, "{}", st.max_dark_run);
        }
    }

    #[test]
    fn without_guard_black_reaches_full_duty() {
        let (spiral, field, lut) = ctx_parts(flat(vec![0; 16]));
        let tone = Tone::from_duty(false, 0.65);
        let ctx = CarveCtx { spiral: &spiral, art: &field, lut: &lut, tone };
        let mut buf = vec![0u8; ECC_BLOCK_BYTES];
        let st = carve_block(&ctx, 100_000, &mut buf);
        assert!(st.max_sector_dark >= 2040);
    }

    #[test]
    fn white_stays_all_light_and_mean_tracks_gray() {
        let (spiral, white, lut) = ctx_parts(flat(vec![255; 16]));
        let ctx = CarveCtx { spiral: &spiral, art: &white, lut: &lut, tone: safe() };
        let mut buf = vec![0u8; ECC_BLOCK_BYTES];
        assert_eq!(carve_block(&ctx, 90_000, &mut buf).dark_bytes, 0);
        let gray = flat(vec![191; 16]);
        let ctx = CarveCtx { spiral: &spiral, art: &gray, lut: &lut, tone: safe() };
        let st = carve_block(&ctx, 90_000, &mut buf);
        let share = st.dark_bytes as f64 / ECC_BLOCK_BYTES as f64;
        assert!((share - 0.25).abs() < 0.01, "{share}");
    }

    /// The exact JSON shape the front end sends, so a renamed field fails here and not in
    /// the middle of a burn session.
    #[test]
    fn accepts_front_end_config_json() {
        let cfg: CarveJobConfig = serde_json::from_str(
            r#"{"layers":[{"id":"a","path":"/a.png","x":0,"y":0,"size":1,"rotationDeg":1.5,"invert":true,"contrast":0,"width":4,"height":4}],
            "outputIsoPath":"/a.iso","servoSafeDither":true,"maxDarkDutyCycle":0.65,
            "preset":{"id":"x","driveModel":"","mediaId":"","stamperCode":"","totalSectors":2295104,
            "channelBitLengthNm":133.3,"innerRadiusMm":24.1,"outerRadiusMm":57.9,"pitchLinear":0,
            "pitchQuadratic":0,"pitchCubic":0,"basePitchUm":0.73,"rmsResidualDeg":0.4,
            "measuredSpanMm":[25.2,56.9],"createdAt":""}}"#,
        )
        .unwrap();
        assert_eq!(cfg.layers[0].rotation_deg, 1.5);
        assert_eq!(cfg.preset.measured_span_mm, (25.2, 56.9));
        let back = serde_json::to_value(&reference_preset()).unwrap();
        assert!(back.get("basePitchUm").is_some() && back["measuredSpanMm"].is_array());
    }

    /// Small-disc round trip through the file path: audit reads back what the build wrote.
    #[test]
    fn build_then_audit_small_image() {
        let dir = std::env::temp_dir().join(format!("disc-core-carve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.iso");
        let mut preset = reference_preset().clone();
        preset.total_sectors = 16 * 400;
        let spiral = Spiral::new(preset.spiral_params()).unwrap();
        let art = crate::artwork::tests::whole(Picture::from_luma(2, 2, vec![0, 255, 255, 0]));
        let cfg = CarveJobConfig { layers: Vec::new(), output_iso_path: out.to_string_lossy().into(), preset, servo_safe_dither: true, max_dark_duty_cycle: 0.65 };
        let s = build_iso_from(&cfg, &spiral, &art, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(s.limit_ok);
        let a = audit_iso(&out).unwrap();
        assert_eq!(a.foreign_bytes, 0);
        assert_eq!(a.sectors, 6400);
        assert!((a.max_sector_dark_ratio - s.max_sector_dark_ratio).abs() < 1e-12);
        std::fs::remove_file(&out).unwrap();
    }

    /// Orientation oracle: per-sector dark share in a real burned image must follow the
    /// source art through our mapping. Handedness errors collapse this to about zero, which
    /// byte-for-byte agreement would hide on a mostly-light, finely dithered source.
    #[test]
    fn orientation_matches_real_burned_image() {
        let Some(root) = crate::probe_root() else { return };
        let (iso, art) = (root.join("carved_artwork.iso"), root.join("artwork_safe_dither.png"));
        let Ok(mut f) = std::fs::File::open(&iso) else { return };
        // The source is square and was carved filling the disc.
        let picture = Picture::load(&art, None).unwrap();
        assert_eq!(picture.width, picture.height);
        let (spiral, field, lut) = ctx_parts(crate::artwork::tests::whole(picture));
        let ctx = CarveCtx { spiral: &spiral, art: &field, lut: &lut, tone: Tone::from_duty(false, 0.65) };
        use std::io::{Read, Seek, SeekFrom};
        let (mut mine, mut theirs) = (Vec::new(), Vec::new());
        let mut a = vec![0u8; ECC_BLOCK_BYTES];
        let mut b = vec![0u8; ECC_BLOCK_BYTES];
        for k in 1..2000u32 {
            let block = k * 571 % 143_444;
            f.seek(SeekFrom::Start(block as u64 * ECC_BLOCK_BYTES as u64)).unwrap();
            f.read_exact(&mut b).unwrap();
            mine.push(carve_block(&ctx, block, &mut a).dark_bytes as f64);
            let mut d = 0.0;
            for (i, v) in b.iter().enumerate() {
                d += ((v ^ lut[seed_index(block * 16)][i % 2048]) == DARK_BYTE) as u32 as f64;
            }
            theirs.push(d);
        }
        let n = mine.len() as f64;
        let (mx, my) = (mine.iter().sum::<f64>() / n, theirs.iter().sum::<f64>() / n);
        let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
        for (x, y) in mine.iter().zip(&theirs) {
            xy += (x - mx) * (y - my);
            xx += (x - mx).powi(2);
            yy += (y - my).powi(2);
        }
        let corr = xy / (xx * yy).sqrt();
        eprintln!("orientation correlation {corr:.4}");
        assert!(corr > 0.90, "{corr}");
    }
}