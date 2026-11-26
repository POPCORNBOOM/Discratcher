//! Codebook calibration disc: concentric rings, each filled with one byte run.
//!
//! A ring holds every track direction, so one photo under a small light sweeps a whole
//! stretch of each run's spectrum, and no angle calibration is needed: rings are cut by
//! LBA, and the radius an LBA range lands at is good to a fraction of a millimetre even
//! on an uncalibrated disc. Plain data fills the gaps so rings are easy to tell apart.

use super::codebook::Codebook;
use super::frame::{frame_slots, FrameSlot};
use crate::carver::{create_mapped, CarveProgress};
use crate::ecma267::*;
use crate::preset::DiscGeometryPreset;
use crate::spiral::Spiral;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const FIRST_RADIUS_MM: f64 = 25.0;
pub const RING_MM: f64 = 1.4;
pub const GAP_MM: f64 = 0.6;
pub const CODEBOOK_ENTRIES: usize = 12;
pub const CODEBOOK_SEED: u64 = 0x5EED_0001;
pub const CODEBOOK_ITERS: usize = 30_000;

/// What a ring is filled with.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Fill {
    /// A run placed frame by frame, starting right after each SYNC code.
    Aligned { bytes: Vec<u8> },
    /// The same run repeated end to end through main data, ignoring frames: what a
    /// layout that does not know about frames would produce.
    Naive { bytes: Vec<u8> },
    /// One byte everywhere.
    Constant { byte: u8 },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ring {
    pub label: String,
    pub r_in_mm: f64,
    pub r_out_mm: f64,
    pub first_lba: u32,
    pub end_lba: u32,
    pub fill: Fill,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SwatchLayout {
    pub preset_id: String,
    pub total_sectors: u32,
    pub codebook: Codebook,
    /// Inner first.
    pub rings: Vec<Ring>,
}

/// Hans Chiu's "Blue" run, from his screenshot. It has drive choice points, so its ring
/// shows whether this drive's DSV rule changes the colour.
pub const PUBLISHED_BLUE: &str = "58 BB C6 66 67 6F 94 CB BF FC 67 C8 CE 58 CB 7D CF CC CF CC 67 B7 89 67 94 BF C0 CE \
    67 94 A3 81 FC 98 AD 7F 98 64 98 64 5C 66 89 67 CC 5D A4 7F FC 86 CF CC 67 CC 82 7D \
    98 CD B7 81 C6 67 6F AD 6F 94 BF A5 98 BF FC C2 C6 66 6F 6F 94 BB E2 7D A2";

pub fn hex_bytes(s: &str) -> Vec<u8> {
    s.split_whitespace().filter_map(|h| u8::from_str_radix(h, 16).ok()).collect()
}

pub fn layout(preset: &DiscGeometryPreset) -> Result<SwatchLayout, String> {
    let spiral = Spiral::new(preset.spiral_params())?;
    let bit_um = preset.channel_bit_length_nm * 1e-3;
    // The calibration disc uses the fixed 12-entry swatch codebook (which is also the
    // prefix 0..CODEBOOK_ENTRIES of the designer's extended codebook), so burned swatch
    // discs stay reproducible across designer codebook additions.
    let cb = super::design::swatch_codebook(bit_um).as_ref().clone();
    // Inner to outer. A solid dark ring would break the carver's servo rule (dark share
    // <= 65-70 %), so the dark control alternates. Dense short marks are the risk to
    // tracking, so codebook rings go outward in rising frequency: if a ring makes the
    // drive lose track, only the end of the disc is lost.
    let twin = &cb.entries[CODEBOOK_ENTRIES / 2];
    let mut fills: Vec<(String, Fill)> = vec![
        ("light 92".into(), Fill::Constant { byte: LIGHT_BYTE }),
        ("dark A5/92".into(), Fill::Naive { bytes: vec![DARK_BYTE, LIGHT_BYTE] }),
        ("published blue".into(), Fill::Aligned { bytes: hex_bytes(PUBLISHED_BLUE) }),
        (format!("C{:02} naive", CODEBOOK_ENTRIES / 2), Fill::Naive { bytes: twin.bytes.clone() }),
    ];
    for (k, e) in cb.entries.iter().enumerate() {
        fills.push((format!("C{k:02} f={:.3}", e.f0), Fill::Aligned { bytes: e.bytes.clone() }));
    }

    let mut rings = Vec::new();
    let mut r = FIRST_RADIUS_MM;
    let max_r = preset.outer_radius_mm - 0.8;
    for (label, fill) in fills {
        let r_out = r + RING_MM;
        if r_out > max_r {
            return Err(format!("盘面放不下全部 {} 个环带", rings.len() + 1));
        }
        rings.push(Ring {
            label,
            r_in_mm: r,
            r_out_mm: r_out,
            first_lba: spiral.lba_at_radius(r),
            end_lba: spiral.lba_at_radius(r_out),
            fill,
        });
        r = r_out + GAP_MM;
    }
    Ok(SwatchLayout { preset_id: preset.id.clone(), total_sectors: preset.total_sectors, codebook: cb, rings })
}

/// The wanted recorded (scrambled) bytes of one sector inside a ring.
fn fill_sector(fill: &Fill, slots: &[FrameSlot], lba: u32, out: &mut [u8]) {
    match fill {
        Fill::Constant { byte } => out.fill(*byte),
        Fill::Aligned { bytes } => {
            for s in slots {
                for j in 0..s.len {
                    out[s.first + j] = bytes[j % bytes.len()];
                }
            }
        }
        Fill::Naive { bytes } => {
            let start = lba as usize * SECTOR_BYTES;
            for (i, b) in out.iter_mut().enumerate() {
                *b = bytes[(start + i) % bytes.len()];
            }
        }
    }
}

/// Bytes a host writes for sector `lba`: plain data (all zero) outside the rings.
pub fn sector(layout: &SwatchLayout, slots: &[FrameSlot], lut: &ScramblerLut, lba: u32, out: &mut [u8]) {
    match layout.rings.iter().find(|r| (r.first_lba..r.end_lba).contains(&lba)) {
        Some(ring) => {
            fill_sector(&ring.fill, slots, lba, out);
            xor_sector(lut, lba, out);
        }
        None => out.fill(0),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwatchSummary {
    pub output_iso_path: String,
    pub layout_path: String,
    pub total_sectors: u32,
    pub rings: usize,
    /// Bytes that differ from the layout when the image is read back: must be 0.
    pub mismatched_bytes: u64,
}

pub fn build_iso(
    preset: &DiscGeometryPreset,
    output_iso_path: &str,
    layout_path: &Path,
    cancel: &AtomicBool,
    progress: &(dyn Fn(CarveProgress) + Sync),
) -> Result<SwatchSummary, String> {
    if preset.media != crate::media::Media::Dvd {
        return Err("码本校准盘只支持 DVD".into());
    }
    let lay = layout(preset)?;
    let total = preset.total_sectors;
    let path = Path::new(output_iso_path);
    std::fs::write(layout_path, serde_json::to_vec_pretty(&lay).map_err(|e| e.to_string())?)
        .map_err(|e| format!("无法写入 {}：{e}", layout_path.display()))?;
    let mut map = create_mapped(path, total as u64 * SECTOR_BYTES as u64)?;
    let lut = build_scrambler_lut();
    let slots = frame_slots();
    let done = AtomicU64::new(0);
    let blocks = (total as u64).div_ceil(ECC_BLOCK_SECTORS as u64);
    let tick = (blocks / 200).max(1);
    let run = map.par_chunks_mut(ECC_BLOCK_BYTES).enumerate().try_for_each(|(b, chunk)| {
        if cancel.load(Ordering::Relaxed) {
            return Err(());
        }
        for (i, s) in chunk.chunks_exact_mut(SECTOR_BYTES).enumerate() {
            sector(&lay, &slots, &lut, (b * ECC_BLOCK_SECTORS + i) as u32, s);
        }
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % tick == 0 {
            progress(CarveProgress { percent: d as f64 / blocks as f64 * 100.0, ..Default::default() });
        }
        Ok(())
    });
    if run.is_err() {
        drop(map);
        let _ = std::fs::remove_file(path);
        return Err("已取消".into());
    }
    crate::iso9660::write_system_area(&mut map, crate::iso9660::SWATCH_VOLUME, total);
    map.flush().map_err(|e| e.to_string())?;
    drop(map);
    Ok(SwatchSummary {
        output_iso_path: output_iso_path.into(),
        layout_path: layout_path.to_string_lossy().into_owned(),
        total_sectors: total,
        rings: lay.rings.len(),
        mismatched_bytes: audit(path, &lay)?,
    })
}

/// Read an image back and check every sector against the layout and system area.
pub fn audit(path: &Path, lay: &SwatchLayout) -> Result<u64, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let map = unsafe { memmap2::Mmap::map(&f) }.map_err(|e| e.to_string())?;
    let lut = build_scrambler_lut();
    let slots = frame_slots();
    Ok(map
        .par_chunks(SECTOR_BYTES)
        .enumerate()
        .map(|(i, s)| {
            let mut want = [0u8; SECTOR_BYTES];
            if !crate::iso9660::system_sector(i as u32, crate::iso9660::SWATCH_VOLUME, lay.total_sectors, &mut want) {
                sector(lay, &slots, &lut, i as u32, &mut want);
            }
            s.iter().zip(want.iter()).filter(|(a, b)| a != b).count() as u64
        })
        .sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holo::efm;
    use crate::holo::frame::main_byte_slot;
    use crate::preset::reference_preset;

    #[test]
    fn rings_fit_and_do_not_overlap() {
        let lay = layout(&reference_preset()).unwrap();
        assert_eq!(lay.rings.len(), CODEBOOK_ENTRIES + 4);
        assert!(
            lay.rings[0].first_lba >= crate::iso9660::SYSTEM_HEAD_SECTORS as u32,
            "ring 0 starts at LBA {}, inside head system area",
            lay.rings[0].first_lba
        );
        for w in lay.rings.windows(2) {
            assert!(w[0].end_lba < w[1].first_lba);
        }
        assert!(
            lay.rings.last().unwrap().end_lba <= lay.total_sectors - crate::iso9660::SYSTEM_TAIL_SECTORS as u32,
            "last ring ends at LBA {}, inside tail system area",
            lay.rings.last().unwrap().end_lba
        );
    }

    /// Descramble a ring sector and check each frame after SYNC holds the run, choice free.
    #[test]
    fn aligned_ring_puts_the_run_after_every_sync() {
        let lay = layout(&reference_preset()).unwrap();
        let ring = &lay.rings[4];
        let Fill::Aligned { bytes } = &ring.fill else { panic!() };
        let (lut, slots) = (build_scrambler_lut(), frame_slots());
        let lba = (ring.first_lba + ring.end_lba) / 2;
        let mut s = [0u8; SECTOR_BYTES];
        sector(&lay, &slots, &lut, lba, &mut s);
        xor_sector(&lut, lba, &mut s);
        for slot in slots.iter().filter(|s| s.after_sync) {
            let run = &s[slot.first..slot.first + slot.len];
            assert_eq!(run, &bytes[..slot.len]);
            assert!(efm::encode_after_sync(run).choices.is_empty());
            assert_eq!(main_byte_slot(slot.first).1, 0);
        }
    }

    #[test]
    fn gaps_are_plain_data() {
        let lay = layout(&reference_preset()).unwrap();
        let (lut, slots) = (build_scrambler_lut(), frame_slots());
        let mut s = [1u8; SECTOR_BYTES];
        sector(&lay, &slots, &lut, lay.rings[0].end_lba + 1, &mut s);
        assert!(s.iter().all(|&b| b == 0));
    }

    #[test]
    fn swatch_codebook_is_unchanged_12_entries() {
        assert_eq!(CODEBOOK_ENTRIES, 12);
        assert_eq!(CODEBOOK_SEED, 0x5EED_0001);
        assert_eq!(CODEBOOK_ITERS, 30_000);
        let preset = reference_preset();
        let bit_um = preset.channel_bit_length_nm * 1e-3;
        let expected = crate::holo::codebook::build(CODEBOOK_ENTRIES, bit_um, CODEBOOK_ITERS, CODEBOOK_SEED);
        let swatch_cb = crate::holo::design::swatch_codebook(bit_um);
        assert_eq!(swatch_cb.entries.len(), CODEBOOK_ENTRIES);
        for (a, b) in swatch_cb.entries.iter().zip(expected.entries.iter()) {
            assert_eq!(a.bytes, b.bytes);
            assert_eq!(a.f0, b.f0);
        }
        let lay = layout(&preset).unwrap();
        assert_eq!(lay.codebook.entries.len(), CODEBOOK_ENTRIES);
        for (a, b) in lay.codebook.entries.iter().zip(expected.entries.iter()) {
            assert_eq!(a.bytes, b.bytes);
        }
        // Designer's shared codebook extends the swatch codebook without mutating its first 12 entries.
        let designer = crate::holo::design::shared_codebook(bit_um);
        assert!(designer.0.entries.len() > CODEBOOK_ENTRIES);
        for (a, b) in designer.0.entries[..CODEBOOK_ENTRIES].iter().zip(expected.entries.iter()) {
            assert_eq!(a.bytes, b.bytes);
        }
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use crate::preset::reference_preset;

    /// A whole (tiny) image built, written and read back must match its layout exactly.
    #[test]
    fn built_image_passes_its_own_audit() {
        let preset = DiscGeometryPreset { total_sectors: 16 * 64, ..reference_preset() };
        let dir = std::env::temp_dir().join(format!("holo-swatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let iso = dir.join("s.iso");
        let sum = build_iso(&preset, iso.to_str().unwrap(), &dir.join("s.json"), &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(sum.mismatched_bytes, 0);
        assert_eq!(std::fs::metadata(&iso).unwrap().len(), 16 * 64 * 2048);
        crate::iso9660::tests::assert_system_area(&std::fs::read(&iso).unwrap(), crate::iso9660::SWATCH_VOLUME, preset.total_sectors);
        let lay: SwatchLayout = serde_json::from_slice(&std::fs::read(dir.join("s.json")).unwrap()).unwrap();
        assert_eq!(lay.rings.len(), CODEBOOK_ENTRIES + 4);
        let _ = std::fs::remove_dir_all(dir);
    }
}
