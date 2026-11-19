//! Writes a short pattern to a DVD-RAM and reads it back, to find out what the drive
//! does with the data it is given, and whether the phase of the scrambler is right.
//!
//! The pattern is written twice: once plain, and once the way the carver writes it,
//! with the scrambler of the format applied. What comes back says which of the two
//! the drive expects, and with which pre-set number.

use disc_core::drive;
use disc_core::ecma330;
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

const SECTOR: usize = 2048;

/// A byte pattern of our own: not one of the scrambler's streams.
fn pattern(lba: u32, sector: &mut [u8]) {
    for (i, b) in sector.iter_mut().enumerate() {
        *b = ((lba as usize * 37 + i * 11 + 5) & 0xFF) as u8;
    }
}

/// The pattern over a whole image, before anything is scrambled.
fn plain_image(sectors: u32) -> Vec<u8> {
    let mut buf = vec![0u8; sectors as usize * SECTOR];
    for lba in 0..sectors {
        let start = lba as usize * SECTOR;
        pattern(lba, &mut buf[start..start + SECTOR]);
    }
    buf
}

/// The same image the way the carver writes it: the format's scrambler on it.
fn carved_image(sectors: u32) -> Vec<u8> {
    let lut = ecma330::build_scrambler_lut();
    let mut buf = plain_image(sectors);
    for lba in 0..sectors {
        let start = lba as usize * SECTOR;
        ecma330::xor_sector(&lut, lba, &mut buf[start..start + SECTOR]);
    }
    buf
}

/// Writes the image through the same path the burning stack uses.
fn write_image(id: &str, path: &Path, image: &[u8]) -> Result<(), String> {
    fs::write(path, image).map_err(|e| format!("cannot write the image: {e}"))?;
    let cancel = AtomicBool::new(false);
    drive::windows::write_dvd_ram(id, path, &cancel, &mut |written, total| {
        println!("  written {written} of {total} bytes");
    })
}

/// Reads the sectors back, one at a time, as any reader of the disc would.
fn read_back(drive: &mut dyn drive::TimedDrive, sectors: u32) -> Vec<u8> {
    let mut buf = vec![0u8; sectors as usize * SECTOR];
    for lba in 0..sectors {
        let start = lba as usize * SECTOR;
        let mut sector = [0u8; SECTOR];
        match drive.read_sector(lba, &mut sector) {
            Ok(()) => buf[start..start + SECTOR].copy_from_slice(&sector),
            Err(e) => {
                println!("  read of sector {lba} failed: {e}");
                break;
            }
        }
    }
    buf
}

/// The pre-set numbers whose stream turns the read-back back into the plain pattern.
fn fitting_presets(back: &[u8], plain: &[u8], lut: &ecma330::ScramblerLut, sectors: u32) -> Vec<usize> {
    let mut out = Vec::new();
    for k in 0..16 {
        let mut ok = true;
        for lba in 0..sectors {
            let start = lba as usize * SECTOR;
            for i in 0..SECTOR {
                if back[start + i] ^ plain[start + i] != lut[k][i] {
                    ok = false;
                    break;
                }
            }
            if !ok {
                break;
            }
        }
        if ok {
            out.push(k);
        }
    }
    out
}

/// A stream that is the right one but starts at another byte of it.
fn shifted_preset(back: &[u8], plain: &[u8], lut: &ecma330::ScramblerLut) -> Option<(usize, usize)> {
    for k in 0..16 {
        for s in 0..SECTOR {
            let mut ok = true;
            for i in 0..SECTOR {
                if back[i] ^ plain[i] != lut[k][(i + s) % SECTOR] {
                    ok = false;
                    break;
                }
            }
            if ok {
                return Some((k, s));
            }
        }
    }
    None
}

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
    let target = drives
        .iter()
        .find(|d| d.media_type == "DVD-RAM" && d.media_present)
        .or_else(|| drives.iter().find(|d| d.media_type == "DVD-RAM"))
        .ok_or("no DVD-RAM drive found")?;
    println!("using {}", target.id);

    let sectors: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(64);
    let path = std::env::temp_dir().join("dvd-ram-probe.iso");
    let plain = plain_image(sectors);
    let carved = carved_image(sectors);
    let lut = ecma330::build_scrambler_lut();

    println!("== writing the pattern plain ==");
    write_image(&target.id, &path, &plain)?;
    let mut handle = drive::open_timed(&target.id)?;
    let back = read_back(handle.as_mut(), sectors);
    if back == plain {
        println!("read back: exactly what was written");
    } else {
        let fits = fitting_presets(&back, &plain, &lut, sectors);
        println!("read back: not what was written; pre-sets that fit: {fits:?}");
        if fits.is_empty() {
            println!("  a shifted stream would fit: {:?}", shifted_preset(&back, &plain, &lut));
        }
    }

    println!("== writing the pattern as the carver writes it ==");
    write_image(&target.id, &path, &carved)?;
    let mut handle = drive::open_timed(&target.id)?;
    let back = read_back(handle.as_mut(), sectors);
    if back == plain {
        println!("read back: the plain pattern: the phase of the scrambler is right");
    } else {
        println!("read back: not the plain pattern");
        let fits = fitting_presets(&back, &plain, &lut, sectors);
        println!("  pre-sets that fit: {fits:?} (ours for LBA 0 is {})", ecma330::preset_index(0));
        if fits.is_empty() {
            println!("  a shifted stream would fit: {:?}", shifted_preset(&back, &plain, &lut));
        }
    }
    Ok(())
}