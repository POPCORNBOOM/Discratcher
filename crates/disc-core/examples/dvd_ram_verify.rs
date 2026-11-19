//! Reads a DVD-RAM back and compares it with an image, to see whether what the
//! drive hands the host is what was on the disc.

use disc_core::drive;
use std::io::{Read, Seek};
use std::path::PathBuf;

fn main() {
    if let Err(e) = run() {
        println!("FAILED: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let drives = drive::list_drives();
    for d in &drives {
        println!(
            "drive {} vendor={} product={} media={} present={} sectors={}",
            d.id, d.vendor, d.product, d.media_type, d.media_present, d.total_sectors
        );
    }
    let mut args = std::env::args().skip(1);
    let iso: PathBuf = args.next().ok_or("usage: dvd_ram_verify <iso> [vendor]")?.into();
    let vendor = args.next().unwrap_or_else(|| "PIONEER".to_string());
    let target = drives
        .iter()
        .find(|d| d.vendor == vendor)
        .ok_or_else(|| format!("no drive from {vendor}"))?;
    println!("using {} ({})", target.id, target.vendor);

    let sectors = std::fs::metadata(&iso).map_err(|e| e.to_string())?.len() / 2048;
    let mut probes: Vec<u64> = vec![0, 16, 17, 1000, 1001, 100_000, sectors / 2, sectors - 1];
    for i in 0..24u64 {
        probes.push(sectors * i / 24);
    }
    probes.sort_unstable();
    probes.dedup();
    let mut handle = drive::open_timed(&target.id)?;
    let mut file = std::fs::File::open(&iso).map_err(|e| e.to_string())?;
    let mut same = 0usize;
    let mut checked = 0usize;
    for lba in probes {
        let lba = lba as u32;
        if lba as u64 >= sectors {
            continue;
        }
        let mut got = [0u8; 2048];
        let mut want = [0u8; 2048];
        file.seek(std::io::SeekFrom::Start(lba as u64 * 2048)).map_err(|e| e.to_string())?;
        file.read_exact(&mut want).map_err(|e| e.to_string())?;
        match handle.read_sector(lba, &mut got) {
            Ok(()) => {
                checked += 1;
                let same_bytes = got == want;
                let first_diff = got.iter().zip(want.iter()).position(|(a, b)| a != b);
                if same_bytes {
                    same += 1;
                    println!("  sector {lba}: identical");
                } else {
                    println!(
                        "  sector {lba}: DIFFERENT, first differing byte at {first_diff:?} of 2048"
                    );
                }
            }
            Err(e) => println!("  sector {lba}: read failed: {e}"),
        }
    }
    println!("verdict: {same} of {checked} sectors read back identical to the image");
    Ok(())
}
