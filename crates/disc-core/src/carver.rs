//! ISO synthesis: spiral sampling, servo-safe dithering, scrambler XOR.

use crate::artwork::{Composition, Layer};
use crate::ecma130;
use crate::ecma267::*;
use crate::media::Media;
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
    /// CD only: first user byte in the raw sector. Left out, it is what this platform's
    /// burner writes.
    #[serde(default)]
    pub cd_user_offset: Option<usize>,
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
    if cfg.preset.media == Media::Cd {
        return build_cd_iso(cfg, spiral, art, cancel, progress);
    }
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

/// Sectors carved together on a CD: enough work per task, small enough for progress.
const CD_CHUNK_SECTORS: usize = 64;

pub struct CdCtx<'a> {
    pub spiral: &'a Spiral,
    pub art: &'a Composition,
    pub tone: Tone,
    pub user_offset: usize,
    pub mask: [u8; SECTOR_BYTES],
    pub placements: [ecma130::Placement; 24],
    /// See `DiscGeometryPreset::along_track_bits`.
    pub shift_bits: f64,
}

impl<'a> CdCtx<'a> {
    pub fn new(spiral: &'a Spiral, art: &'a Composition, tone: Tone, user_offset: usize, shift_bits: f64) -> Self {
        Self { spiral, art, tone, user_offset, mask: ecma130::user_mask(user_offset), placements: ecma130::placements(), shift_bits }
    }
}

/// Dither threshold in 0..1 for a symbol, from its place on the track. A CD sector's
/// bytes are scattered over two sectors' length, so error cannot be carried from one
/// byte to the next as on a DVD; a golden-ratio sequence along the track spreads the
/// dark bytes just as evenly and needs no neighbour.
#[inline]
fn track_threshold(frame: u64, slot: u8) -> f32 {
    let i = frame * 32 + slot as u64;
    (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) as f32 / (1u64 << 24) as f32
}

/// Carve one CD sector into 2048 bytes ready to write; returns how many came out dark.
///
/// Bytes that enter the encoder in the same position of a frame keep the same delay, so
/// each of the 24 positions is a straight run of 98 frames along the track: its ends are
/// looked up on the spiral and the rest stepped between them.
pub fn carve_cd_sector(ctx: &CdCtx, lba: u32, out: &mut [u8]) -> u32 {
    let frames = ecma130::FRAMES_PER_SECTOR as usize;
    let first_frame = lba as u64 * frames as u64;
    let ro2 = 2.0 * ctx.spiral.params.ro;
    let Tone { servo_safe, min_luma } = ctx.tone;
    let mut dark = 0u32;
    for (q, place) in ctx.placements.iter().enumerate() {
        let start = first_frame + place.delay as u64;
        let bit = |frame: u64| ecma130::symbol_bit(frame as f64, place.slot) - ctx.shift_bits;
        let (r0, th0) = ctx.spiral.locate(bit(start));
        let (r1, th1) = ctx.spiral.locate(bit(start + frames as u64 - 1));
        let (dr, dth) = ((r1 - r0) / (frames - 1) as f64, (th1 - th0) / (frames - 1) as f64);
        let (mut s, mut c) = th0.sin_cos();
        let (ds, dc) = dth.sin_cos();
        for k in 0..frames {
            // Clause 16 swaps each byte pair on the way in.
            let raw = (k * 24 + q) ^ 1;
            if (ctx.user_offset..ctx.user_offset + SECTOR_BYTES).contains(&raw) {
                let r = r0 + k as f64 * dr;
                let grey = ctx.art.sample(r * c / ro2, r * s / ro2);
                let white = if servo_safe {
                    track_threshold(start + k as u64, place.slot) < grey.max(min_luma) / 255.0
                } else {
                    grey >= 128.0
                };
                let i = raw - ctx.user_offset;
                dark += !white as u32;
                out[i] = if white { ecma130::LIGHT_BYTE } else { ecma130::DARK_BYTE } ^ ctx.mask[i];
            }
            let (ns, nc) = (s * dc + c * ds, c * dc - s * ds);
            s = ns;
            c = nc;
        }
    }
    dark
}

fn build_cd_iso(
    cfg: &CarveJobConfig,
    spiral: &Spiral,
    art: &Composition,
    cancel: &AtomicBool,
    progress: &(dyn Fn(CarveProgress) + Sync),
) -> Result<CarveJobSummary, String> {
    let total = cfg.preset.total_sectors;
    let tone = Tone::from_duty(cfg.servo_safe_dither, cfg.max_dark_duty_cycle);
    let offset = cfg.cd_user_offset.unwrap_or_else(crate::drive::cd_user_offset);
    if offset + SECTOR_BYTES > ecma130::RAW_BYTES {
        return Err("CD 扇区布局无效".into());
    }
    let ctx = CdCtx::new(spiral, art, tone, offset, cfg.preset.along_track_bits());
    let bytes = total as u64 * SECTOR_BYTES as u64;
    let path = Path::new(&cfg.output_iso_path);
    let mut map = create_mapped(path, bytes)?;

    let start = Instant::now();
    let done = AtomicU64::new(0);
    let dark = AtomicU64::new(0);
    let max_dark = AtomicU64::new(0);
    let chunks = (total as u64).div_ceil(CD_CHUNK_SECTORS as u64);
    let tick = (chunks / 200).max(1);
    let run = map.par_chunks_mut(CD_CHUNK_SECTORS * SECTOR_BYTES).enumerate().try_for_each(|(chunk, out)| {
        if cancel.load(Ordering::Relaxed) {
            return Err(());
        }
        for (i, sector) in out.chunks_exact_mut(SECTOR_BYTES).enumerate() {
            let d = carve_cd_sector(&ctx, (chunk * CD_CHUNK_SECTORS + i) as u32, sector);
            dark.fetch_add(d as u64, Ordering::Relaxed);
            max_dark.fetch_max(d as u64, Ordering::Relaxed);
        }
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % tick == 0 {
            let dt = start.elapsed().as_secs_f64().max(1e-6);
            progress(CarveProgress {
                percent: d as f64 / chunks as f64 * 100.0,
                mb_per_s: d as f64 * (CD_CHUNK_SECTORS * SECTOR_BYTES) as f64 / 1e6 / dt,
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
        // A sector's bytes are not neighbours on a CD, so a run of them means nothing.
        max_dark_run_bytes: 0,
        servo_safe: cfg.servo_safe_dither,
        limit_ok: !cfg.servo_safe_dither || max_ratio <= MAX_SECTOR_DARK_LIMIT,
    })
}

/// What an image must descramble to: the stream each sector was XORed with, and the two
/// bytes that may remain.
pub enum Encoding {
    Dvd,
    Cd { user_offset: usize },
}

impl Encoding {
    pub fn of(cfg: &CarveJobConfig) -> Self {
        match cfg.preset.media {
            Media::Dvd => Self::Dvd,
            Media::Cd => Self::Cd { user_offset: cfg.cd_user_offset.unwrap_or_else(crate::drive::cd_user_offset) },
        }
    }
}

/// Read a finished image back from disk and measure it, independent of the build's
/// own counters. Foreign bytes mean the XOR or the file is wrong.
pub fn audit_iso(path: &Path, encoding: &Encoding) -> Result<IsoAudit, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let map = unsafe { Mmap::map(&f) }.map_err(|e| e.to_string())?;
    if map.len() % SECTOR_BYTES != 0 {
        return Err("镜像大小不是 2048 的倍数".into());
    }
    let (cd_mask, dark_byte, light_byte) = match encoding {
        Encoding::Dvd => (None, DARK_BYTE, LIGHT_BYTE),
        Encoding::Cd { user_offset } => (Some(ecma130::user_mask(*user_offset)), ecma130::DARK_BYTE, ecma130::LIGHT_BYTE),
    };
    let lut = build_scrambler_lut();
    let (foreign, worst, worst_lba) = map
        .par_chunks(SECTOR_BYTES)
        .enumerate()
        .map(|(i, s)| {
            let table = cd_mask.as_ref().unwrap_or(&lut[seed_index(i as u32)]);
            let (mut dark, mut foreign) = (0u32, 0u64);
            for (b, k) in s.iter().zip(table.iter()) {
                match b ^ k {
                    q if q == dark_byte => dark += 1,
                    q if q == light_byte => {}
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
    use crate::preset::{reference_preset, DiscGeometryPreset};
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
        let cfg = CarveJobConfig { layers: Vec::new(), output_iso_path: out.to_string_lossy().into(), preset, servo_safe_dither: true, max_dark_duty_cycle: 0.65, cd_user_offset: None };
        let s = build_iso_from(&cfg, &spiral, &art, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(s.limit_ok);
        let a = audit_iso(&out, &Encoding::Dvd).unwrap();
        assert_eq!(a.foreign_bytes, 0);
        assert_eq!(a.sectors, 6400);
        assert!((a.max_sector_dark_ratio - s.max_sector_dark_ratio).abs() < 1e-12);
        std::fs::remove_file(&out).unwrap();
    }

    /// What a CD player's laser would meet: walk whole recorded frames, find which host
    /// byte the encoder put in each symbol, and compare its tone with the picture at that
    /// spot on the disc. This runs the mapping backwards from how the carver uses it.
    fn frames_show_the_picture(along_track_frames: Option<f64>, user_offset: usize) {
        let preset = DiscGeometryPreset { along_track_frames, ..crate::preset::cd_test_preset() };
        let spiral = Spiral::new(preset.spiral_params()).unwrap();
        // Left half black, right half white.
        let art = crate::artwork::tests::whole(Picture::from_luma(64, 64, (0..64 * 64).map(|i| if i % 64 < 32 { 0 } else { 255 }).collect()));
        let ctx = CdCtx::new(&spiral, &art, Tone::from_duty(false, 0.65), user_offset, preset.along_track_bits());
        let (first, count) = (150_000u32, 6usize);
        let mut sectors = vec![0u8; count * SECTOR_BYTES];
        let mut dark = 0;
        for (i, s) in sectors.chunks_exact_mut(SECTOR_BYTES).enumerate() {
            dark += carve_cd_sector(&ctx, first + i as u32, s);
        }
        assert!(dark > 1000 && (dark as usize) < count * SECTOR_BYTES - 1000, "both tones present: {dark}");
        let (mut seen, mut wrong, mut mixed) = (0, 0, 0);
        let ro2 = 2.0 * spiral.params.ro;
        // Frames every one of whose data symbols comes from the carved sectors.
        for frame in first as u64 * 98 + 108..(first as u64 + count as u64) * 98 {
            let mut sources = std::collections::HashSet::new();
            for (q, place) in ctx.placements.iter().enumerate() {
                let global = (frame - place.delay as u64) * 24 + q as u64;
                let (lba, raw) = (global / 2352, (global % 2352) as usize ^ 1);
                if !(user_offset..user_offset + SECTOR_BYTES).contains(&raw) {
                    continue;
                }
                let i = raw - user_offset;
                let recorded = sectors[(lba as usize - first as usize) * SECTOR_BYTES + i] ^ ctx.mask[i];
                assert!(recorded == ecma130::DARK_BYTE || recorded == ecma130::LIGHT_BYTE);
                let (r, th) = spiral.locate(ecma130::symbol_bit(frame as f64, place.slot) - preset.along_track_bits());
                let want_light = art.sample(r * th.cos() / ro2, r * th.sin() / ro2) >= 128.0;
                seen += 1;
                wrong += (want_light != (recorded == ecma130::LIGHT_BYTE)) as usize;
                sources.insert(lba);
            }
            mixed += (sources.len() > 1) as usize;
        }
        assert!(seen > 8000, "{seen}");
        assert!(wrong * 500 < seen, "{wrong} of {seen} symbols show the wrong tone");
        assert!(mixed > 300, "CIRC mixes sectors within a frame: {mixed}");
    }

    #[test]
    fn cd_frames_show_the_picture() {
        frames_show_the_picture(None, ecma130::MODE1_USER_OFFSET);
        frames_show_the_picture(Some(206.0), ecma130::MODE2_FORM1_USER_OFFSET);
    }

    #[test]
    fn cd_dither_follows_grey_and_keeps_the_guard() {
        let preset = crate::preset::cd_test_preset();
        let spiral = Spiral::new(preset.spiral_params()).unwrap();
        let mut out = [0u8; SECTOR_BYTES];
        for (grey, want) in [(191u8, 0.25), (0, 0.651), (255, 0.0)] {
            let art = flat(vec![grey; 16]);
            let ctx = CdCtx::new(&spiral, &art, safe(), 16, 0.0);
            for lba in [1000u32, 200_000, 359_000] {
                let share = carve_cd_sector(&ctx, lba, &mut out) as f64 / SECTOR_BYTES as f64;
                assert!((share - want).abs() < 0.02, "grey {grey} lba {lba}: {share}");
            }
        }
    }

    #[test]
    fn cd_build_then_audit() {
        let dir = std::env::temp_dir().join(format!("disc-core-cd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("cd.iso");
        let mut preset = crate::preset::cd_test_preset();
        preset.total_sectors = 3000;
        let spiral = Spiral::new(preset.spiral_params()).unwrap();
        let art = crate::artwork::tests::whole(Picture::from_luma(2, 2, vec![0, 255, 255, 0]));
        let cfg = CarveJobConfig { layers: Vec::new(), output_iso_path: out.to_string_lossy().into(), preset, servo_safe_dither: true, max_dark_duty_cycle: 0.65, cd_user_offset: Some(16) };
        let s = build_iso_from(&cfg, &spiral, &art, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(s.limit_ok, "{}", s.max_sector_dark_ratio);
        let a = audit_iso(&out, &Encoding::of(&cfg)).unwrap();
        assert_eq!((a.foreign_bytes, a.sectors), (0, 3000));
        // Read as the other layout, or as a DVD, the same file is noise.
        assert!(audit_iso(&out, &Encoding::Cd { user_offset: 24 }).unwrap().foreign_bytes > 1_000_000);
        assert!(audit_iso(&out, &Encoding::Dvd).unwrap().foreign_bytes > 1_000_000);
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