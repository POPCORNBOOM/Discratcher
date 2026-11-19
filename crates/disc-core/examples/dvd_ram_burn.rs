//! Writes an image to a DVD-RAM the way the burning stack does, and reads it back.
//!
//! The write goes through drive::windows::write_dvd_ram, the same call the
//! burning stack makes for a DVD-RAM, so what this proves is what the program
//! would do.

use disc_core::drive;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

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
            "drive {} vendor={} product={} media={} present={} sectors={} supported={:?} speeds={:?}",
            d.id, d.vendor, d.product, d.media_type, d.media_present, d.total_sectors, d.supported, d.write_speeds
        );
    }

    let mut args = std::env::args().skip(1);
    let iso: PathBuf = args.next().ok_or("usage: dvd_ram_burn <iso> [vendor]")?.into();
    let vendor = args.next().unwrap_or_else(|| "PIONEER".to_string());
    let target = drives
        .iter()
        .find(|d| d.vendor == vendor)
        .ok_or_else(|| format!("no drive from {vendor}"))?;
    println!("using {} ({}) media={} present={}", target.id, target.vendor, target.media_type, target.media_present);
    println!("image {} bytes", std::fs::metadata(&iso).map_err(|e| e.to_string())?.len());

    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    println!("write starts at {:?}", started.elapsed());
    let result = drive::windows::write_dvd_ram(&target.id, &iso, &cancel, &mut |written, total| {
        println!("  [{:?}] written {written} of {total} bytes", started.elapsed());
    });
    println!("write returned after {:?}: {:?}", started.elapsed(), result.as_ref().err());
    result?;

    println!("== reading back ==");
    let sectors = std::fs::metadata(&iso).map_err(|e| e.to_string())?.len() / 2048;
    let probes = [0u64, 16, 1000, 100_000, sectors / 2, sectors - 1];
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
                if got == want {
                    same += 1;
                    println!("  sector {lba}: identical");
                } else {
                    println!("  sector {lba}: DIFFERENT");
                }
            }
            Err(e) => println!("  sector {lba}: read failed: {e}"),
        }
    }
    println!("verdict: {same} of {checked} sectors read back identical to the image");
    Ok(())
}
