//! Drive access. Measurement code talks to `TimedDrive`, so the ladder runs
//! unchanged on real hardware and on the simulator.

pub mod burner;
pub mod sim;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod imapi;
#[cfg(target_os = "windows")]
pub mod windows;

use serde::{Deserialize, Serialize};

pub const SIM_DRIVE_ID: &str = "sim://gud1n";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpticalDriveInfo {
    pub id: String,
    pub vendor: String,
    pub product: String,
    pub revision: String,
    pub media_present: bool,
    pub media_type: String,
    pub media_id: String,
    pub total_sectors: u32,
    pub is_blank: bool,
    #[serde(default)]
    pub simulated: bool,
    #[serde(default)]
    pub write_speeds: Vec<u8>,
    #[serde(default)]
    pub max_write_speed: u8,
    /// The loaded disc is one Discratcher can calibrate and carve. Filled in by
    /// `list_drives`, so no backend can forget it.
    #[serde(default)]
    pub supported: bool,
}

/// Single-layer DVD only: calibration and carving count 38,688 channel bits a sector,
/// and a CD or Blu-ray disc would take the burn and come out as noise. Anything not
/// named here, including a type nobody could read, is refused.
pub fn is_supported_media(media_type: &str) -> bool {
    matches!(media_type.trim(), "DVD-R" | "DVD+R" | "DVD-RW" | "DVD+RW")
}

/// Refuses a burn on anything but a DVD the app supports, whatever the page allowed.
pub fn check_burnable(id: &str) -> Result<(), String> {
    let d = list_drives().into_iter().find(|d| d.id == id).ok_or_else(|| format!("找不到光驱 {id}"))?;
    if !d.media_present {
        return Err("光驱里没有光盘".into());
    }
    if !d.supported {
        return Err(format!("目前只支持 DVD 光盘，这张是 {}", d.media_type));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TimedRead {
    /// Completion instant, nanoseconds on a monotonic clock.
    pub t_end_ns: f64,
}

pub trait TimedDrive: Send {
    /// Read one sector and report when it finished. `fua` forces media access where the
    /// transport supports it.
    fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String>;
    fn capacity(&mut self) -> Result<u32, String>;
    /// False when the transport cannot bypass the drive's own cache; callers then have to
    /// evict it with a distant read.
    fn supports_fua(&self) -> bool;
    /// Best effort. Drives may ignore it.
    fn set_read_speed(&mut self, kbps: u16) -> Result<(), String>;
    fn host_wait(&mut self, ns: u64);
}

pub fn list_drives() -> Vec<OpticalDriveInfo> {
    let mut v = native_list();
    // Development only: a user must never mistake the simulator for hardware.
    if cfg!(debug_assertions) {
        v.push(sim::drive_info());
    }
    for d in &mut v {
        d.supported = d.media_present && is_supported_media(&d.media_type);
    }
    v
}

fn native_list() -> Vec<OpticalDriveInfo> {
    #[cfg(target_os = "macos")]
    return macos::list();
    #[cfg(target_os = "linux")]
    return linux::list();
    #[cfg(target_os = "windows")]
    return windows::list();
    #[allow(unreachable_code)]
    Vec::new()
}

/// Open a drive for timing reads. `paced` slows the simulator so progress can be watched.
pub fn open_timed(id: &str, paced: bool) -> Result<Box<dyn TimedDrive>, String> {
    if id == SIM_DRIVE_ID {
        return Ok(Box::new(sim::SimDrive::demo(paced)));
    }
    #[cfg(target_os = "macos")]
    return macos::open(id);
    #[cfg(target_os = "linux")]
    return linux::open(id);
    #[cfg(target_os = "windows")]
    return windows::open(id);
    #[allow(unreachable_code)]
    Err("这个平台没有光驱后端".into())
}

/// Write speeds to offer for a blank disc. "offered" is what the drive itself
/// reported; "detected_max" is the drive's own limit, which wins over what the disc
/// claims: a 16x disc in an 8x drive is 8x. With nothing reported at all, guess low
/// rather than offer a speed nobody confirmed.
pub fn derive_write_speeds(
    media_type: &str,
    is_blank: bool,
    offered: Vec<u8>,
    detected_max: Option<u8>,
) -> (Vec<u8>, u8) {
    if !is_blank {
        return (Vec::new(), 0);
    }
    let is_cd = media_type.to_ascii_uppercase().contains("CD");
    let default_ladder = if is_cd { vec![4, 8, 10, 16, 24, 32, 40, 48] } else { vec![2, 4, 6, 8, 12, 16] };

    let max_speed = detected_max.or_else(|| offered.iter().copied().max()).unwrap_or(if is_cd { 16 } else { 4 });

    let mut speeds: Vec<u8> = offered.into_iter().filter(|&s| s <= max_speed).collect();
    if speeds.is_empty() {
        speeds = default_ladder.into_iter().filter(|&s| s <= max_speed).collect();
    }

    let effective_max = speeds.iter().copied().max().unwrap_or(max_speed);
    (speeds, effective_max)
}

/// CDB layouts and reply decoding shared by every transport, so the three backends
/// cannot drift apart, and so the parsing is testable without a drive.
pub mod mmc {
    pub fn read12(lba: u32, sectors: u32, fua: bool) -> [u8; 12] {
        let l = lba.to_be_bytes();
        let n = sectors.to_be_bytes();
        [0xA8, if fua { 0x08 } else { 0 }, l[0], l[1], l[2], l[3], n[0], n[1], n[2], n[3], 0, 0]
    }

    pub fn read_capacity() -> [u8; 10] {
        [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    }

    /// SET CD SPEED with plain rotation control, the mode every measured disc used.
    pub fn set_speed(kbps: u16) -> [u8; 12] {
        let k = kbps.to_be_bytes();
        [0xBB, 0, k[0], k[1], 0xFF, 0xFF, 0, 0, 0, 0, 0, 0]
    }

    /// Capacity from a READ CAPACITY reply.
    pub fn parse_capacity(b: &[u8; 8]) -> u32 {
        u32::from_be_bytes([b[0], b[1], b[2], b[3]]) + 1
    }

    /// Vendor, product and revision from a standard INQUIRY reply. The three fields
    /// are space padded, and not every drive pads them neatly.
    pub fn parse_inquiry(b: &[u8]) -> (String, String, String) {
        let field = |r: std::ops::Range<usize>| {
            let end = r.end.min(b.len());
            let start = r.start.min(end);
            String::from_utf8_lossy(&b[start..end]).trim().to_string()
        };
        (field(8..16), field(16..32), field(32..36))
    }

    /// What is loaded, from the current profile in a GET CONFIGURATION header. The
    /// drive answers with the profile of the medium, not of the drive.
    pub fn profile_media_type(profile: u16) -> Option<&'static str> {
        Some(match profile {
            0x0008 => "CD-ROM",
            0x0009 => "CD-R",
            0x000A => "CD-RW",
            0x0010 => "DVD-ROM",
            0x0011 => "DVD-R",
            0x0012 => "DVD-RAM",
            0x0013 | 0x0014 => "DVD-RW",
            0x001A => "DVD+RW",
            0x001B => "DVD+R",
            0x002B => "DVD+R DL",
            0x0040 => "BD-ROM",
            0x0041 | 0x0042 => "BD-R",
            0x0043 => "BD-RE",
            _ => return None,
        })
    }

    /// Disc status from READ DISC INFORMATION: 0 empty, 1 appendable, 2 finalised, 3 other
    /// (overwritable media). It is the low two bits of byte 2; bit 4 above it is the
    /// erasable flag, which every RW disc sets whether or not anything is on it.
    /// A status nobody can read counts as finalised, so an unknown disc is never
    /// mistaken for a blank one.
    pub fn disc_status(b: &[u8]) -> u8 {
        b.get(2).map_or(2, |v| v & 0x03)
    }

    /// Sectors a blank medium of this type holds. Some drives answer READ CAPACITY
    /// with nothing at all for a disc that has never been written, and one sector is
    /// worse than the nominal size: the calibration would plan its reads around it.
    pub fn nominal_sectors(media_type: &str) -> u32 {
        match media_type {
            "CD-R" | "CD-RW" | "CD-ROM" => 359_849,
            "DVD+R DL" => 4_173_824,
            m if m.starts_with("DVD") => 2_295_104,
            m if m.starts_with("BD") => 12_219_392,
            _ => 0,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cdb_layouts() {
            let c = read12(0x0011_2233, 1, true);
            assert_eq!(&c[..6], &[0xA8, 0x08, 0x00, 0x11, 0x22, 0x33]);
            assert_eq!(&c[6..10], &[0, 0, 0, 1]);
            assert_eq!(set_speed(5540)[..4], [0xBB, 0x00, 0x15, 0xA4]);
            assert_eq!(parse_capacity(&[0, 0x23, 0x05, 0x3F, 0, 0, 8, 0]), 2_295_104);
        }

        #[test]
        fn inquiry_fields_are_trimmed() {
            let mut b = [0u8; 36];
            b[..8].copy_from_slice(&[0x05, 0x80, 0x00, 0x32, 0x5B, 0x00, 0x00, 0x00]);
            b[8..16].copy_from_slice(b"HL-DT-ST");
            b[16..32].copy_from_slice(b"DVDRAM GUD1N    ");
            b[32..36].copy_from_slice(b"1.01");
            assert_eq!(parse_inquiry(&b), ("HL-DT-ST".into(), "DVDRAM GUD1N".into(), "1.01".into()));
            // A short reply must not panic either.
            assert_eq!(parse_inquiry(&b[..10]).0, "HL");
        }

        #[test]
        fn profiles_name_the_media_not_the_drive() {
            assert_eq!(profile_media_type(0x0011), Some("DVD-R"));
            assert_eq!(profile_media_type(0x0009), Some("CD-R"));
            assert_eq!(profile_media_type(0x002B), Some("DVD+R DL"));
            assert_eq!(profile_media_type(0x0013), Some("DVD-RW"));
            assert_eq!(profile_media_type(0x0043), Some("BD-RE"));
            assert_eq!(profile_media_type(0x0000), None);
        }

        #[test]
        fn disc_status_reads_the_two_bit_field() {
            assert_eq!(disc_status(&[0, 0, 0x00]), 0);
            assert_eq!(disc_status(&[0, 0, 0x01]), 1);
            // A finalised DVD-R: last session complete (bits 3-2) and disc complete.
            assert_eq!(disc_status(&[0, 0, 0x0E]), 2);
            // A blank CD-RW or DVD-RW sets the erasable bit and is still empty.
            assert_eq!(disc_status(&[0, 0, 0x10]), 0);
            assert_eq!(disc_status(&[0, 0, 0x1E]), 2);
            // A reply too short to hold the field must not read as blank.
            assert_eq!(disc_status(&[]), 2);
        }

        #[test]
        fn nominal_capacity_fills_in_for_a_blank_disc() {
            assert_eq!(nominal_sectors("DVD-R"), 2_295_104);
            assert_eq!(nominal_sectors("DVD+R DL"), 4_173_824);
            assert_eq!(nominal_sectors("CD-R"), 359_849);
            assert_eq!(nominal_sectors("None"), 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_layer_dvd_is_supported() {
        for m in ["DVD-R", "DVD+R", "DVD-RW", "DVD+RW", " DVD-R "] {
            assert!(is_supported_media(m), "{m}");
        }
        for m in ["CD-R", "CD-RW", "BD-R", "BD-RE", "DVD+R DL", "DVD-RAM", "DVD-ROM", "None", ""] {
            assert!(!is_supported_media(m), "{m}");
        }
    }

    #[test]
    fn write_speeds_are_guessed_low_when_nothing_was_reported() {
        assert_eq!(derive_write_speeds("DVD-R", true, Vec::new(), None), (vec![2, 4], 4));
        assert_eq!(derive_write_speeds("CD-R", true, Vec::new(), None), (vec![4, 8, 10, 16], 16));
        // A written disc is not offered a speed at all.
        assert_eq!(derive_write_speeds("DVD-R", false, vec![2, 4], Some(8)), (Vec::new(), 0));
        // Offered speeds above the drive limit are dropped.
        assert_eq!(derive_write_speeds("DVD-R", true, vec![2, 4, 8, 12, 16], Some(8)), (vec![2, 4, 8], 8));
    }
}