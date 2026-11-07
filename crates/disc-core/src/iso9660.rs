//! Minimal valid ISO 9660 volume (empty root) plus random-fill calibration images.
//!
//! The filesystem lets the OS recognise and unmount the disc and satisfies burners that
//! refuse a bare image. It occupies LBA 16..=19, inside the innermost millimetre.

use crate::carver::create_mapped;
use crate::ecma267::SECTOR_BYTES;
use crate::rng::SplitMix64;
use rayon::prelude::*;
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

/// Deterministic random fill: timing reads need ordinary data, single-byte textures make
/// drives retry. Each sector has its own stream, so the build parallelises.
pub fn generate_calibration_iso(path: &Path, total_sectors: u32) -> Result<(), String> {
    if total_sectors < 32 {
        return Err("扇区数太少".into());
    }
    let mut map = create_mapped(path, total_sectors as u64 * SECTOR_BYTES as u64)?;
    map.par_chunks_mut(SECTOR_BYTES).enumerate().for_each(|(i, s)| {
        let mut g = SplitMix64(0x2026_1104_0000_0000 ^ i as u64);
        for w in s.chunks_exact_mut(8) {
            w.copy_from_slice(&g.next_u64().to_le_bytes());
        }
    });
    write_volume(&mut map, CALIBRATION_VOLUME, total_sectors);
    map.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_are_well_formed_and_random_fill_survives() {
        let dir = std::env::temp_dir().join(format!("disc-core-iso-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.iso");
        generate_calibration_iso(&p, 1024).unwrap();
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
