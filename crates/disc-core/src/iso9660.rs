//! Minimal valid ISO 9660 volume (empty root) plus random-fill calibration images.
//!
//! The filesystem lets the OS recognise and unmount the disc and satisfies burners that
//! refuse a bare image. It occupies LBA 16..=19, inside the innermost millimetre.

use crate::carver::{create_sized, CarveProgress};
use crate::ecma267::SECTOR_BYTES;
use crate::rng::SplitMix64;
use std::io::Write;
use std::path::Path;

pub const CALIBRATION_VOLUME: &str = "DISCPROBE_RANDOM";
pub const FS_LAST_LBA: usize = 19;

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

/// Sectors per write. 4 MiB keeps the write count low and still lets progress move.
const BATCH_SECTORS: usize = 2048;

/// Deterministic random fill: timing reads need ordinary data, single-byte textures make
/// drives retry. Each sector has its own stream, so a sector can be produced on its own.
fn fill_sector(s: &mut [u8], lba: usize) {
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
mod tests {
    use super::*;

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
        std::fs::remove_file(&p).unwrap();
    }
}