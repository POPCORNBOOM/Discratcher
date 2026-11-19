//! Prints the first bytes of a sector of a DVD-RAM, to see what the drive hands
//! the host at a given place of the disc.

use disc_core::drive;

fn main() {
    if let Err(e) = run() {
        println!("FAILED: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let lba: u32 = args.next().ok_or("usage: dvd_ram_dump <lba> [vendor]")?.parse().map_err(|_| "bad lba")?;
    let vendor = args.next().unwrap_or_else(|| "PIONEER".to_string());
    let drives = drive::list_drives();
    let target = drives.iter().find(|d| d.vendor == vendor).ok_or("no drive")?;
    let mut handle = drive::open_timed(&target.id)?;
    let mut sector = [0u8; 2048];
    match handle.read_sector(lba, &mut sector) {
        Ok(()) => {
            print!("lba {lba}: ");
            for b in sector.iter().take(48) {
                print!("{b:02x} ");
            }
            println!();
            let zeros = sector.iter().filter(|b| **b == 0).count();
            println!("  zero bytes: {zeros} of 2048");
        }
        Err(e) => println!("lba {lba}: read failed: {e}"),
    }
    Ok(())
}
