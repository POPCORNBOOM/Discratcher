//! Minimal valid ISO 9660 volume (empty root) plus random-fill calibration images.
//!
//! The filesystem lets the OS recognise and unmount the disc and satisfies burners that
//! refuse a bare image. It occupies LBA 16..=19, inside the innermost millimetre.

use crate::carver::{create_sized, CarveProgress};
use crate::ecma267::SECTOR_BYTES;
use crate::rng::SplitMix64;
use std::io::Write;
use std::path::Path;

/// Source: the volume label in discprobe `randomdisc.py`.
pub const CALIBRATION_VOLUME: &str = "DISCPROBE_RANDOM";
pub const CARVE_VOLUME: &str = "DISCRATCHER";
pub const HOLO_VOLUME: &str = "DISCRATCHER_HOLO";
pub const SWATCH_VOLUME: &str = "DISCRATCHER_SWATCH";
pub const FS_LAST_LBA: usize = 19;

/// Leading sectors kept as plain user data (zeros plus the ISO 9660 volume at LBA 16..=19).
///
/// 512 sectors = 32 DVD ECC blocks. Covers MBR/APM/GPT primary (LBA 0..33), ISO 9660
/// descriptors/root (16..=19), and the UDF primary AVDP (256), while costing only ~0.013 mm
/// at a DVD's inner rim (~0.079 mm on a CD).
pub const SYSTEM_HEAD_SECTORS: usize = 512;

/// Trailing sectors kept as plain zero user data.
///
/// 272 sectors = 17 DVD ECC blocks (ECC-block aligned on any 16-sector-multiple DVD image).
/// Covers UDF backup AVDPs at N-257 and N-1, backup GPT at N-33..N-1, and HFS+ alternate
/// volume headers at N-1, while costing only ~0.003 mm at a DVD's outer rim (~0.018 mm on a CD).
pub const SYSTEM_TAIL_SECTORS: usize = 272;

/// True if `lba` lies in the reserved head (`0..SYSTEM_HEAD_SECTORS`) or tail
/// (`total_sectors - SYSTEM_TAIL_SECTORS .. total_sectors`) system area.
#[inline]
pub fn is_system_sector(lba: u32, total_sectors: u32) -> bool {
    let i = lba as usize;
    let total = total_sectors as usize;
    i < SYSTEM_HEAD_SECTORS || i >= total.saturating_sub(SYSTEM_TAIL_SECTORS)
}

fn both_u32(b: &mut [u8], v: u32) {
    b[..4].copy_from_slice(&v.to_le_bytes());
    b[4..8].copy_from_slice(&v.to_be_bytes());
}

fn both_u16(b: &mut [u8], v: u16) {
    b[..2].copy_from_slice(&v.to_le_bytes());
    b[2..4].copy_from_slice(&v.to_be_bytes());
}

fn dir_record(b: &mut [u8], extent: u32, size: u32, name: u8) {
    b[0] = 34;
    both_u32(&mut b[2..10], extent);
    both_u32(&mut b[10..18], size);
    b[25] = 0x02; // directory
    both_u16(&mut b[28..32], 1); // volume sequence
    b[32] = 1;
    b[33] = name;
}

/// Write the descriptors and tables into an image at least 20 sectors long.
pub fn write_volume(image: &mut [u8], label: &str, total_sectors: u32) {
    const ROOT: u32 = 19;
    const PATH_L: u32 = 18;
    let at = |lba: usize| lba * SECTOR_BYTES;
    image[at(16)..at(FS_LAST_LBA + 1)].fill(0);

    let pvd = &mut image[at(16)..at(17)];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[8..40].fill(b' ');
    pvd[40..72].fill(b' ');
    let n = label.len().min(32);
    pvd[40..40 + n].copy_from_slice(&label.as_bytes()[..n]);
    both_u32(&mut pvd[80..88], total_sectors);
    both_u16(&mut pvd[120..124], 1);
    both_u16(&mut pvd[124..128], 1);
    both_u16(&mut pvd[128..132], SECTOR_BYTES as u16);
    both_u32(&mut pvd[132..140], 10); // path table size
    pvd[140..144].copy_from_slice(&PATH_L.to_le_bytes());
    dir_record(&mut pvd[156..190], ROOT, SECTOR_BYTES as u32, 0);
    pvd[190..813].fill(b' ');
    for range in [813..830usize, 830..847, 847..864, 864..881] {
        pvd[range.start..range.end - 1].fill(b'0');
    }
    pvd[881] = 1;

    let term = &mut image[at(17)..at(18)];
    term[0] = 255;
    term[1..6].copy_from_slice(b"CD001");
    term[6] = 1;

    let path = &mut image[at(18)..at(19)];
    path[0] = 1;
    path[2..6].copy_from_slice(&ROOT.to_le_bytes());
    path[6..8].copy_from_slice(&1u16.to_le_bytes());

    let root = &mut image[at(19)..at(20)];
    dir_record(&mut root[0..34], ROOT, SECTOR_BYTES as u32, 0);
    dir_record(&mut root[34..68], ROOT, SECTOR_BYTES as u32, 1);
}

/// Stamp the readable system area onto an image built for burning: plain zero user data
/// in LBA `0..SYSTEM_HEAD_SECTORS` and `total_sectors - SYSTEM_TAIL_SECTORS..total_sectors`,
/// with a minimal ISO 9660 volume (including its Volume Descriptor Set Terminator) at
/// LBA `16..=19`.
///
/// Every image builder stores host user-data bytes in the ISO file: on write, the drive
/// scrambles zeros with its LFSR key (`key ^ 0 == key`, the same on-disc plain stream
/// `holo::design` uses), so the OS can read LBA 0, 16..=19, 256, N-257 and N-1 cleanly
/// and `cd9660` claims the disc immediately without minutes of retry timeouts.
pub fn write_system_area(image: &mut [u8], label: &str, total_sectors: u32) {
    let sectors = (image.len() / SECTOR_BYTES).min(total_sectors as usize);
    let head = SYSTEM_HEAD_SECTORS.min(sectors);
    image[..head * SECTOR_BYTES].fill(0);
    if sectors > FS_LAST_LBA {
        write_volume(image, label, total_sectors);
    }
    let tail_start = sectors.saturating_sub(SYSTEM_TAIL_SECTORS).max(head);
    image[tail_start * SECTOR_BYTES..sectors * SECTOR_BYTES].fill(0);
}

/// Fill `out` with the exact host bytes `write_system_area` writes at `lba`, returning
/// `true` if `lba` belongs to the system area and `false` otherwise.
pub fn system_sector(lba: u32, label: &str, total_sectors: u32, out: &mut [u8; SECTOR_BYTES]) -> bool {
    if !is_system_sector(lba, total_sectors) {
        return false;
    }
    let i = lba as usize;
    if (16..=FS_LAST_LBA).contains(&i) && total_sectors as usize > FS_LAST_LBA {
        let mut vol = vec![0u8; (FS_LAST_LBA + 1) * SECTOR_BYTES];
        write_volume(&mut vol, label, total_sectors);
        out.copy_from_slice(&vol[i * SECTOR_BYTES..(i + 1) * SECTOR_BYTES]);
    } else {
        out.fill(0);
    }
    true
}

/// Sectors per write. 4 MiB keeps the write count low and still lets progress move.
const BATCH_SECTORS: usize = 2048;

/// Deterministic random fill: timing reads need ordinary data, single-byte textures make
/// drives retry. Each sector has its own stream, so a sector can be produced on its own.
fn fill_sector(s: &mut [u8], lba: usize) {
    fill_random(s, lba)
}

/// One sector of the calibration image of `total_sectors`, exactly as
/// `generate_calibration_iso` writes it, so a burned disc can be checked against it.
pub fn calibration_sector(lba: u32, total_sectors: u32, out: &mut [u8; SECTOR_BYTES]) {
    let lba = lba as usize;
    if lba <= FS_LAST_LBA {
        let mut head = vec![0u8; (FS_LAST_LBA + 1) * SECTOR_BYTES];
        for (i, s) in head.chunks_mut(SECTOR_BYTES).enumerate() {
            fill_random(s, i);
        }
        write_volume(&mut head, CALIBRATION_VOLUME, total_sectors);
        out.copy_from_slice(&head[lba * SECTOR_BYTES..(lba + 1) * SECTOR_BYTES]);
    } else {
        fill_random(out, lba);
    }
}

fn fill_random(s: &mut [u8], lba: usize) {
    let mut g = SplitMix64(0x2026_1104_0000_0000 ^ lba as u64);
    for w in s.chunks_exact_mut(8) {
        w.copy_from_slice(&g.next_u64().to_le_bytes());
    }
}

fn megabytes_per_s(bytes: u64, elapsed: std::time::Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return 0.0;
    }
    bytes as f64 / 1e6 / secs
}

/// Write the calibration image, reporting progress as it goes.
///
/// The file is written straight through rather than through a mapping. Faulting in 4.7 GB
/// through a mapping was measured at 87 s on a nearly full system drive, where writing the
/// same bytes in order took a few seconds, and a mapping shows no progress while it fills.
pub fn generate_calibration_iso(
    path: &Path,
    total_sectors: u32,
    progress: &(dyn Fn(CarveProgress) + Sync),
) -> Result<(), String> {
    if total_sectors < 32 {
        return Err("扇区数太少".into());
    }
    let total = total_sectors as usize;
    let size = total as u64 * SECTOR_BYTES as u64;
    let file = create_sized(path, size)?;
    let began = std::time::Instant::now();
    let mut out = std::io::BufWriter::with_capacity(4 << 20, file);

    // The descriptors sit in the first sectors, over random data.
    let head_len = (FS_LAST_LBA + 1) * SECTOR_BYTES;
    let mut head = vec![0u8; head_len];
    for (i, s) in head.chunks_mut(SECTOR_BYTES).enumerate() {
        fill_sector(s, i);
    }
    write_volume(&mut head, CALIBRATION_VOLUME, total_sectors);

    let mut batch = vec![0u8; BATCH_SECTORS * SECTOR_BYTES];
    let mut done = 0usize;
    let mut last_percent = -1i32;
    while done < total {
        let n = BATCH_SECTORS.min(total - done);
        let buf = &mut batch[..n * SECTOR_BYTES];
        for (k, s) in buf.chunks_mut(SECTOR_BYTES).enumerate() {
            let lba = done + k;
            match head.get(lba * SECTOR_BYTES..(lba + 1) * SECTOR_BYTES) {
                Some(h) => s.copy_from_slice(h),
                None => fill_sector(s, lba),
            }
        }
        out.write_all(buf).map_err(|e| format!("写入校准镜像失败：{e}"))?;
        done += n;
        let percent = (done * 100 / total) as i32;
        if percent != last_percent {
            last_percent = percent;
            progress(CarveProgress {
                percent: percent as f64,
                mb_per_s: megabytes_per_s((done * SECTOR_BYTES) as u64, began.elapsed()),
                max_sector_dark_ratio: 0.0,
            });
        }
    }
    out.flush().map_err(|e| format!("写入校准镜像失败：{e}"))?;
    out.get_ref().sync_all().map_err(|e| format!("写入校准镜像失败：{e}"))?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::preset::{cd_test_preset, reference_preset};
    use crate::spiral::Spiral;

    /// Shared verifier used by every builder's tests: checks plain head (0..512), consistent
    /// ISO 9660 volume + terminator at 16..=19, and plain tail (N-272..N).
    pub(crate) fn assert_system_area(image: &[u8], expected_label: &str, total_sectors: u32) {
        let total = total_sectors as usize;
        assert_eq!(image.len(), total * SECTOR_BYTES);
        let sec = |lba: usize| &image[lba * SECTOR_BYTES..(lba + 1) * SECTOR_BYTES];

        for lba in 0..16 {
            assert!(sec(lba).iter().all(|&b| b == 0), "LBA {lba} must be plain zero");
        }

        // LBA 16: Primary Volume Descriptor.
        let pvd = sec(16);
        assert_eq!(pvd[0], 1, "PVD type code");
        assert_eq!(&pvd[1..6], b"CD001", "PVD standard identifier");
        assert_eq!(pvd[6], 1, "PVD version");
        let label = std::str::from_utf8(&pvd[40..72]).unwrap().trim_end();
        assert_eq!(label, expected_label, "PVD volume label");
        assert_eq!(u32::from_le_bytes(pvd[80..84].try_into().unwrap()), total_sectors, "PVD volume space size LE");
        assert_eq!(u32::from_be_bytes(pvd[84..88].try_into().unwrap()), total_sectors, "PVD volume space size BE");
        assert_eq!(u16::from_le_bytes(pvd[128..130].try_into().unwrap()), SECTOR_BYTES as u16, "PVD block size LE");
        assert_eq!(u16::from_be_bytes(pvd[130..132].try_into().unwrap()), SECTOR_BYTES as u16, "PVD block size BE");
        assert_eq!(u32::from_le_bytes(pvd[132..136].try_into().unwrap()), 10, "PVD path table size");
        assert_eq!(u32::from_le_bytes(pvd[140..144].try_into().unwrap()), 18, "PVD L-path table LBA");
        assert_eq!(pvd[156], 34, "PVD root dir record length");
        assert_eq!(u32::from_le_bytes(pvd[158..162].try_into().unwrap()), 19, "PVD root extent LE");
        assert_eq!(u32::from_be_bytes(pvd[162..166].try_into().unwrap()), 19, "PVD root extent BE");
        assert_eq!(pvd[156 + 25], 0x02, "PVD root directory flag");
        assert_eq!(pvd[881], 1, "PVD file structure version");

        // LBA 17: Volume Descriptor Set Terminator (stops ISO/UDF VRS probing).
        let term = sec(17);
        assert_eq!(term[0], 255, "VDST type code");
        assert_eq!(&term[1..6], b"CD001", "VDST standard identifier");
        assert_eq!(term[6], 1, "VDST version");
        assert!(term[7..].iter().all(|&b| b == 0), "VDST unused bytes");

        // LBA 18: L-Path Table pointing at root LBA 19.
        let path = sec(18);
        assert_eq!(path[0], 1);
        assert_eq!(u32::from_le_bytes(path[2..6].try_into().unwrap()), 19);
        assert_eq!(u16::from_le_bytes(path[6..8].try_into().unwrap()), 1);

        // LBA 19: Root Directory ('.' and '..' self-entries).
        let root = sec(19);
        assert_eq!(root[0], 34);
        assert_eq!(u32::from_le_bytes(root[2..6].try_into().unwrap()), 19);
        assert_eq!(root[33], 0);
        assert_eq!(root[34], 34);
        assert_eq!(u32::from_le_bytes(root[36..40].try_into().unwrap()), 19);
        assert_eq!(root[34 + 33], 1);

        for lba in (FS_LAST_LBA + 1)..SYSTEM_HEAD_SECTORS.min(total) {
            assert!(sec(lba).iter().all(|&b| b == 0), "LBA {lba} must be plain zero");
        }
        let tail_start = total.saturating_sub(SYSTEM_TAIL_SECTORS).max(SYSTEM_HEAD_SECTORS.min(total));
        for lba in tail_start..total {
            assert!(sec(lba).iter().all(|&b| b == 0), "tail LBA {lba} must be plain zero");
        }
        // Explicitly check UDF AVDP anchor sectors when the image is large enough.
        if total >= SYSTEM_HEAD_SECTORS + SYSTEM_TAIL_SECTORS {
            assert!(sec(256).iter().all(|&b| b == 0), "UDF primary AVDP (LBA 256) must be plain");
            assert!(sec(total - 257).iter().all(|&b| b == 0), "UDF backup AVDP (N-257) must be plain");
            assert!(sec(total - 1).iter().all(|&b| b == 0), "UDF backup AVDP (N-1) must be plain");
        }
    }

    #[test]
    fn descriptors_are_well_formed_and_random_fill_survives() {
        let dir = std::env::temp_dir().join(format!("disc-core-iso-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.iso");
        generate_calibration_iso(&p, 1024, &|_| {}).unwrap();
        let d = std::fs::read(&p).unwrap();
        assert_eq!(d.len(), 1024 * 2048);
        assert_eq!(&d[16 * 2048 + 1..16 * 2048 + 6], b"CD001");
        assert_eq!(&d[16 * 2048 + 40..16 * 2048 + 56], b"DISCPROBE_RANDOM");
        assert_eq!(d[17 * 2048], 255);
        assert_ne!(&d[0..64], &d[2048..2048 + 64]);
        assert!(d[0..2048].iter().any(|&b| b != 0));
        // Every sector can be produced on its own, which is how a burned disc is checked.
        let mut one = [0u8; 2048];
        for lba in [0u32, 16, 17, 19, 20, 512, 1023] {
            calibration_sector(lba, 1024, &mut one);
            assert_eq!(&one[..], &d[lba as usize * 2048..(lba as usize + 1) * 2048], "LBA {lba}");
        }
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn write_system_area_and_single_sector_agree() {
        let total = 1024u32;
        let mut img = vec![0xA5u8; total as usize * SECTOR_BYTES];
        write_system_area(&mut img, HOLO_VOLUME, total);
        assert_system_area(&img, HOLO_VOLUME, total);
        // Middle sectors are untouched.
        assert!(img[512 * SECTOR_BYTES..(1024 - SYSTEM_TAIL_SECTORS) * SECTOR_BYTES].iter().all(|&b| b == 0xA5));
        let mut one = [0xFFu8; SECTOR_BYTES];
        for lba in 0..total {
            let expected = is_system_sector(lba, total);
            assert_eq!(system_sector(lba, HOLO_VOLUME, total, &mut one), expected);
            if expected {
                assert_eq!(&one[..], &img[lba as usize * SECTOR_BYTES..(lba as usize + 1) * SECTOR_BYTES], "LBA {lba}");
            }
        }
    }

    #[test]
    fn system_area_radial_width_is_well_under_a_tenth_millimetre() {
        let dvd = Spiral::new(reference_preset().spiral_params()).unwrap();
        let dvd_head_mm = dvd.radius_of_lba(SYSTEM_HEAD_SECTORS as u32) - dvd.radius_of_lba(0);
        let n = reference_preset().total_sectors;
        let dvd_tail_mm = dvd.radius_of_lba(n) - dvd.radius_of_lba(n - SYSTEM_TAIL_SECTORS as u32);
        assert!(dvd_head_mm > 0.012 && dvd_head_mm < 0.014, "DVD head band: {dvd_head_mm:.6} mm");
        assert!(dvd_tail_mm > 0.002 && dvd_tail_mm < 0.004, "DVD tail band: {dvd_tail_mm:.6} mm");

        let cd = Spiral::new(cd_test_preset().spiral_params()).unwrap();
        let cd_head_mm = cd.radius_of_lba(SYSTEM_HEAD_SECTORS as u32) - cd.radius_of_lba(0);
        let cd_n = cd_test_preset().total_sectors;
        let cd_tail_mm = cd.radius_of_lba(cd_n) - cd.radius_of_lba(cd_n - SYSTEM_TAIL_SECTORS as u32);
        assert!(cd_head_mm > 0.070 && cd_head_mm < 0.085, "CD head band: {cd_head_mm:.6} mm");
        assert!(cd_tail_mm > 0.015 && cd_tail_mm < 0.025, "CD tail band: {cd_tail_mm:.6} mm");
    }
}