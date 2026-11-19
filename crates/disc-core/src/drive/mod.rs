//! Drive access. Measurement code talks to `TimedDrive`, so the ladder runs
//! unchanged on real hardware and on the tests' simulator.

pub mod burner;
#[cfg(test)]
pub mod sim;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod imapi;
#[cfg(target_os = "windows")]
pub mod windows;

use crate::media::Media;
use serde::{Deserialize, Serialize};

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
    pub write_speeds: Vec<u8>,
    #[serde(default)]
    pub max_write_speed: u8,
    /// The loaded disc is one Discratcher can calibrate and carve. Filled in by
    /// `list_drives`, so no backend can forget it.
    #[serde(default)]
    pub supported: bool,
    /// A written rewritable disc this platform's burner erases before it writes, so it
    /// can be used again. Filled in by `list_drives`, so no backend can forget it.
    #[serde(default)]
    pub reusable: bool,
}

/// Single-layer DVD and CD only: calibration and carving count the channel bits of
/// those two formats, and a Blu-ray or dual-layer disc would take the burn and come out
/// as noise. Anything not named, including a type nobody could read, is refused.
pub fn is_supported_media(media_type: &str) -> bool {
    Media::of_type(media_type).is_some()
}

/// Discs that can be written again, and so do not have to be new to be used.
pub fn is_rewritable(media_type: &str) -> bool {
    matches!(media_type.trim(), "CD-RW" | "DVD-RW" | "DVD+RW")
}

/// Whether this platform's burner erases a rewritable disc on its own before writing it.
/// hdiutil does; the others have no erase step wired in yet, so a written disc stays
/// unusable there rather than being burned over blindly.
pub fn erases_before_burn() -> bool {
    cfg!(target_os = "macos")
}

/// Where this platform's burner puts the 2048 user bytes inside a CD sector: IMAPI2 writes
/// Mode 2 Form 1 (found by disc-probe on a burned disc), hdiutil and the rest Mode 1.
/// A burned calibration disc is checked against this before anything is carved.
pub fn cd_user_offset() -> usize {
    if cfg!(target_os = "windows") {
        crate::ecma130::MODE2_FORM1_USER_OFFSET
    } else {
        crate::ecma130::MODE1_USER_OFFSET
    }
}

/// Refuses a burn on anything but a disc the app supports, whatever the page allowed, and
/// an image that does not fit the disc. Every image Discratcher makes covers the whole
/// disc, so one much smaller than the disc was made for another disc (issue #8).
pub fn check_burnable(id: &str, iso: &str) -> Result<(), String> {
    let d = list_drives().into_iter().find(|d| d.id == id).ok_or_else(|| format!("找不到光驱 {id}"))?;
    if !d.media_present {
        return Err("光驱里没有光盘".into());
    }
    if !d.supported {
        return Err(format!("目前只支持 DVD 和 CD 光盘，这张是 {}", d.media_type));
    }
    let image = std::fs::metadata(iso).map_err(|e| format!("读不到镜像：{e}"))?.len().div_ceil(2048);
    image_fits(image, d.total_sectors as u64)
}

pub fn image_fits(image: u64, disc: u64) -> Result<(), String> {
    if disc == 0 {
        return Ok(());
    }
    if image > disc {
        return Err(format!("镜像有 {image} 个扇区，光盘只有 {disc} 个，放不下"));
    }
    if image < disc / 100 * 99 {
        return Err(format!("镜像只有 {image} 个扇区，这张盘有 {disc} 个：镜像是按别的光盘生成的，请重新生成"));
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
    /// Read one sector's 2048 user bytes. Used to check a calibration disc before it is
    /// timed, never on the timing path.
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; 2048]) -> Result<(), String>;
    /// A CD sector as recorded, all 2352 bytes with sync and header (READ CD). Only
    /// pass-through transports can.
    fn read_raw_sector(&mut self, _lba: u32, _buf: &mut [u8; 2352]) -> Result<(), String> {
        Err("这个通道读不了原始扇区".into())
    }
    /// False when the transport cannot bypass the drive's own cache; callers then have to
    /// evict it with a distant read.
    fn supports_fua(&self) -> bool;
    /// Best effort. Drives may ignore it.
    fn set_read_speed(&mut self, kbps: u16) -> Result<(), String>;
    fn host_wait(&mut self, ns: u64);
}

/// Whether the system stopped answering about drives, so an empty list says nothing about
/// what is plugged in. Only macOS is known to get into this state.
pub fn stuck() -> bool {
    #[cfg(target_os = "macos")]
    return macos::stuck();
    #[allow(unreachable_code)]
    false
}

pub fn list_drives() -> Vec<OpticalDriveInfo> {
    let mut v = native_list();
    for d in &mut v {
        d.supported = d.media_present && is_supported_media(&d.media_type);
        d.reusable = d.media_present && !d.is_blank && is_rewritable(&d.media_type) && erases_before_burn();
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

/// Open a drive for timing reads.
pub fn open_timed(id: &str) -> Result<Box<dyn TimedDrive>, String> {
    #[cfg(target_os = "macos")]
    return macos::open(id);
    #[cfg(target_os = "linux")]
    return linux::open(id);
    #[cfg(target_os = "windows")]
    return windows::open(id);
    #[allow(unreachable_code)]
    Err("这个平台没有光驱后端".into())
}

/// Open the tray. On macOS the drive's own button does nothing while a disc is mounted,
/// and the Finder cannot eject a disc that an interrupted session left unmounted.
pub fn eject(id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::eject(id);
    #[cfg(target_os = "linux")]
    return linux::eject(id);
    #[cfg(target_os = "windows")]
    return windows::eject(id);
    #[allow(unreachable_code)]
    Err("这个平台没有光驱后端".into())
}

/// Open a drive to read one disc, leaving the drive to the system where the platform can.
/// The timing path takes the drive exclusively on macOS, and with it the ability to eject
/// it, so a read that only decides what the disc is must not go through it.
pub fn open_reader(id: &str) -> Result<Box<dyn TimedDrive>, String> {
    #[cfg(target_os = "macos")]
    if let Ok(d) = macos::open_reader(id) {
        return Ok(d);
    }
    open_timed(id)
}

/// Whether the burn should ask the tool to erase first. Any rewritable disc, blank or not:
/// on a fresh one the same step is what formats it, and on a written one it is the only
/// way it can be written again. Nothing erases anything outside a burn the user started.
pub fn needs_erase(media_type: &str) -> bool {
    is_rewritable(media_type) && erases_before_burn()
}

/// Write speeds to offer for a blank disc, or for a rewritable one the burner erases
/// first. "offered" is what the drive itself reported; "detected_max" is the drive's own
/// limit, which wins over what the disc claims: a 16x disc in an 8x drive is 8x. With
/// nothing reported at all, guess low rather than offer a speed nobody confirmed.
pub fn derive_write_speeds(
    media_type: &str,
    is_blank: bool,
    offered: Vec<u8>,
    detected_max: Option<u8>,
) -> (Vec<u8>, u8) {
    let rewritable = is_rewritable(media_type) && erases_before_burn();
    if !is_blank && !rewritable {
        return (Vec::new(), 0);
    }
    let is_cd = media_type.to_ascii_uppercase().contains("CD");
    // An erased disc has no dye to read an optimum off, and the drive often reports
    // nothing for it, so a rewritable disc is only ever offered the slow, safe end.
    let default_ladder = if rewritable {
        if is_cd {
            vec![4]
        } else {
            vec![2, 4]
        }
    } else if is_cd {
        vec![4, 8, 10, 16, 24, 32, 40, 48]
    } else {
        vec![2, 4, 6, 8, 12, 16]
    };

    let max_speed = detected_max
        .or_else(|| offered.iter().copied().max())
        .unwrap_or(if rewritable || !is_cd { 4 } else { 16 });

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

    /// READ CD for one whole raw sector of any type: sync, headers, user data, EDC and ECC.
    pub fn read_cd_raw(lba: u32) -> [u8; 12] {
        let l = lba.to_be_bytes();
        [0xBE, 0, l[0], l[1], l[2], l[3], 0, 0, 1, 0xF8, 0, 0]
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

    /// READ DISC STRUCTURE. The command is twelve bytes long: the format code sits
    /// in byte 7 and the allocation length in bytes 8 and 9. A drive handed the
    /// length in the wrong place answers with an empty buffer instead of an error,
    /// which is indistinguishable from a disc that has nothing to say.
    pub fn read_disc_structure(format: u8, allocation: u16) -> [u8; 12] {
        [0xAD, 0, 0, 0, 0, 0, 0, format, (allocation >> 8) as u8, allocation as u8, 0, 0]
    }

    /// The structures to ask a disc for its code, in the order to ask them.
    ///
    /// The standard keeps a DVD+R's code in ADIP, which is format 0x11. A drive
    /// that knows no ADIP format of its own says the field is invalid and hands
    /// out the physical format instead, with a copy of the same block in it, so
    /// that is asked next. A DVD-R keeps its code in a table of records, and only
    /// the physical format has it.
    pub fn media_id_formats(media_type: &str) -> &'static [u8] {
        if media_type.starts_with("DVD+") {
            &[0x11, 0x00]
        } else {
            &[0x00]
        }
    }

    /// One record of the disc structure: an index, six characters of text and a
    /// terminator.
    const RECORD_BYTES: usize = 8;

    /// How long the maker's part of a DVD+R's own code is.
    const MAKER_BYTES: usize = 8;
    /// How long the media code part of it is.
    const MEDIA_CODE_BYTES: usize = 3;
    /// The ADIP block says how many of its bytes it uses, and that count is the
    /// last of them: the marker that tells a code from the bytes around it.
    const ADIP_BYTES_IN_USE: u8 = 0x38;
    /// The product revision in the same block is a small number.
    const ADIP_REVISION_MAX: u8 = 0x0F;

    /// The disc's own identification code, out of a READ DISC STRUCTURE answer.
    ///
    /// A DVD-R keeps its code in a table of fixed eight byte records. Two of
    /// them hold text, the maker and the media code, and together they spell the
    /// MID the DVD Forum has printed on recordable discs since 2000: a DVD-R
    /// from Mitsubishi reads "MCC 03" and "RG20" there and makes "MCC 03RG20",
    /// the string the calibration library files that disc under.
    ///
    /// A DVD+R keeps its code in ADIP instead, and `parse_adip_id` reads that.
    ///
    /// A drive that answers with something else has nothing to say, and the disc
    /// is then named by hand.
    pub fn parse_medium_id(b: &[u8]) -> Option<String> {
        (0..b.len())
            .filter(|at| is_record_table(b, *at, 1) || is_record_table(b, *at, 0))
            .find_map(|at| {
                let text = |index: usize| {
                    let start = at + (index - 1) * RECORD_BYTES;
                    text_field(b.get(start + 1..start + RECORD_BYTES)?)
                };
                let mid = format!("{}{}", text(3)?, text(4)?).trim_end().to_string();
                (!mid.is_empty()).then_some(mid)
            })
    }

    /// The code of a DVD+R, out of the ADIP the drive reports.
    ///
    /// A DVD+R does not keep its code in a table of records: it keeps it in
    /// ADIP, the address information written into the groove. The drive copies
    /// that into the structure it reports, and where it puts the copy depends
    /// on the drive: one that answers with ADIP itself puts the block at the
    /// front, one that knows no ADIP format of its own copies it into the
    /// media specific area of the structure it does report. A fixed offset
    /// would read the wrong bytes on such a drive, so the block is found by
    /// its shape instead: the maker in eight bytes padded with zeros, the
    /// media code in three, the product revision, and the count of the bytes
    /// the block uses.
    ///
    /// The maker is padded out with zeros, and DVDInfo prints that padding as
    /// one space, so the maker "RITEK" and the code "F16" read as "RITEK F16".
    ///
    /// A block that does not have that shape is not a code, and a disc whose
    /// code cannot be read is left unnamed rather than named wrongly.
    pub fn parse_adip_id(b: &[u8]) -> Option<String> {
        (0..b.len()).find_map(|at| {
            let maker = text_field(b.get(at..at + MAKER_BYTES)?)?;
            let code = text_field(b.get(at + MAKER_BYTES..at + MAKER_BYTES + MEDIA_CODE_BYTES)?)?;
            let revision = *b.get(at + MAKER_BYTES + MEDIA_CODE_BYTES)?;
            if *b.get(at + MAKER_BYTES + MEDIA_CODE_BYTES + 1)? != ADIP_BYTES_IN_USE {
                return None;
            }
            if revision > ADIP_REVISION_MAX {
                return None;
            }
            let maker = maker.trim_end().to_string();
            let code = code.trim_end().to_string();
            if maker.is_empty() || code.is_empty() {
                return None;
            }
            // A maker that fills its eight bytes stands next to the code; a padded
            // one is separated from it, the way DVDInfo prints it.
            let space = if maker.len() < MAKER_BYTES { " " } else { "" };
            Some(format!("{maker}{space}{code}"))
        })
    }

    /// Is a record table here, with its first record numbered `first`? The records
    /// count up one by one and every one of them ends in a zero, which no run of
    /// bytes does by accident.
    ///
    /// The numbers are what tells the tables apart. The run of zero bytes in front
    /// of the real one looks like a table of its own, one record off, and a
    /// table found that way would read the wrong fields.
    fn is_record_table(b: &[u8], at: usize, first: u8) -> bool {
        (0..4).all(|k| b.get(at + k * RECORD_BYTES) == Some(&first.wrapping_add(k as u8)))
            && (0..4).all(|k| b.get(at + k * RECORD_BYTES + RECORD_BYTES - 1) == Some(&0))
    }

    /// The text of one record: what is printed there, when it is text at all.
    fn text_field(field: &[u8]) -> Option<String> {
        let chars: Vec<u8> = field.iter().take_while(|&&c| c != 0).copied().collect();
        if chars.is_empty() || !chars.iter().all(|&c| (32..127).contains(&c)) {
            return None;
        }
        Some(String::from_utf8(chars).ok()?)
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
            assert_eq!(read_cd_raw(0x0102_0304), [0xBE, 0, 1, 2, 3, 4, 0, 0, 1, 0xF8, 0, 0]);
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

        /// A disc structure as a drive returns it: a header of zeros, then one
        /// record per field, each an index, six characters and a terminator.
        fn disc_structure(records: &[&[u8]]) -> Vec<u8> {
            let mut b = vec![0u8; 40 + records.len() * RECORD_BYTES];
            for (i, r) in records.iter().enumerate() {
                let at = 40 + i * RECORD_BYTES;
                b[at] = (i + 1) as u8;
                for (j, c) in r.iter().take(6).enumerate() {
                    b[at + 1 + j] = *c;
                }
            }
            b
        }

        #[test]
        fn the_mid_is_the_maker_and_the_media_code() {
            // What a Mitsubishi DVD-R answered: two binary fields, then the maker
            // and the media code, then more binary fields.
            let b = disc_structure(&[
                b"\x40\xc1\xfd\x9e\xd8\x52",
                b"\x85\x0d\x0e\x88\x9a\x80",
                b"MCC 03",
                b"RG20  ",
                b"\x88\x80",
                b"\x88\x80",
            ]);
            assert_eq!(parse_medium_id(&b).as_deref(), Some("MCC 03RG20"));
        }

        /// A DVD+R keeps its code in ADIP, a fixed layout instead of a table of
        /// numbered records, so there is no table to find in its structure.
        #[test]
        fn a_dvd_plus_disc_has_no_table_to_read() {
            let b = disc_structure(&[b"OPTODI", b"SCR016"]);
            assert_eq!(parse_medium_id(&b), None);
        }

        /// The structure a blank RITEK DVD+R answered with, byte for byte. It was
        /// read on an ASUS DRW-24D1ST, which knows no ADIP format of its own
        /// and copies the code into the structure it reports instead.
        const DVD_PLUS_STRUCTURE: &[u8] = &[
            0x08, 0x02, 0x00, 0x00, 0xA1, 0x0F, 0x02, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x26, 0x05, 0x3F,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x52, 0x49, 0x54, 0x45, 0x4B, 0x00, 0x00, 0x00, 0x46,
            0x31, 0x36, 0x01, 0x38, 0x23, 0x54, 0x37, 0x12, 0x02, 0x50, 0x6E, 0x02, 0x8C, 0x68, 0x16, 0x16,
            0x0B, 0x0B, 0x0A, 0x0A, 0x01, 0x1D, 0x1E, 0x0C, 0x0C, 0x12, 0x12, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x38, 0x38, 0x02, 0x2D, 0x62, 0x1E, 0x08, 0x2E, 0x23, 0x10,
            0x04, 0x1E, 0x1D, 0x01, 0x00, 0x00, 0x02, 0x2D, 0x62, 0x1E, 0x08, 0x2E, 0x23, 0x10, 0x04, 0x1E,
            0x1D, 0x01, 0x00, 0x00, 0x02, 0x00, 0x54, 0x70, 0x02, 0x38, 0x64, 0x24, 0x30, 0x18, 0x10, 0x10,
            0x01, 0x20, 0x00, 0x00, 0x00, 0x00, 0x02, 0x48, 0x60, 0x2A, 0x30, 0x18, 0x10, 0x10, 0x01, 0x20,
            0x00, 0x00, 0x00, 0x28, 0x03, 0x00, 0x54, 0xE0, 0x02, 0x41, 0x64, 0x24, 0x30, 0x18, 0x10, 0x10,
            0x01, 0x28, 0x00, 0x00, 0x00, 0x00, 0x02, 0xAF, 0x5B, 0x3E, 0x30, 0x18, 0x10, 0x10, 0x01, 0x28,
            0x00, 0x00, 0x00, 0x5A,
        ];

        /// The ADIP block a drive reports, from its type marker to the count of
        /// the bytes it uses. Read off a Pioneer DVR-221L with a blank RITEK
        /// DVD+R in it, and the same block in every answer that drive gave.
        const ADIP_BLOCK: &[u8] = &[
            0xA1, 0x0F, 0x02, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x26, 0x05, 0x3F,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07,
            0x52, 0x49, 0x54, 0x45, 0x4B, 0x00, 0x00, 0x00,
            0x46, 0x31, 0x36,
            0x01, 0x38,
        ];

        /// The ADIP answer of that drive, byte for byte: the block at the front,
        /// behind a four byte header. The maker sits nineteen bytes into the
        /// block, the media code eight further along.
        const PIONEER_ADIP_ANSWER: &[u8] = &[
            0x01, 0x02, 0x00, 0x00,
            0xA1, 0x0F, 0x02, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x26, 0x05, 0x3F,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07,
            0x52, 0x49, 0x54, 0x45, 0x4B, 0x00, 0x00, 0x00,
            0x46, 0x31, 0x36,
            0x01, 0x38,
            0x23, 0x54, 0x37, 0x12,
        ];

        /// The physical format answer of the same drive, with the same block
        /// copied into it.
        const PIONEER_PHYSICAL_ANSWER: &[u8] = &[
            0x08, 0x02, 0x00, 0x00,
            0x01, 0x0F, 0x02, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x26, 0x05, 0x3F,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07,
            0x52, 0x49, 0x54, 0x45, 0x4B, 0x00, 0x00, 0x00,
            0x46, 0x31, 0x36,
            0x01, 0x38,
            0x23, 0x54, 0x00, 0x00,
        ];

        #[test]
        fn a_dvd_plus_disc_names_itself_in_adip() {
            assert_eq!(parse_adip_id(DVD_PLUS_STRUCTURE).as_deref(), Some("RITEK F16"));
            // The same structure holds no table of records, so the DVD-R reader
            // finds nothing in it.
            assert_eq!(parse_medium_id(DVD_PLUS_STRUCTURE), None);
        }

        /// A drive that knows the ADIP format answers it, and the code is
        /// where the block says it is.
        #[test]
        fn the_adip_answer_of_a_drive_that_knows_the_format() {
            assert_eq!(parse_adip_id(PIONEER_ADIP_ANSWER).as_deref(), Some("RITEK F16"));
            // The same drive hands out the same block in the physical format.
            assert_eq!(parse_adip_id(PIONEER_PHYSICAL_ANSWER).as_deref(), Some("RITEK F16"));
        }

        /// Where the drive puts the block is up to the drive: one that copies
        /// it into the media specific area of the structure it reports puts it
        /// further along, and the code has to be found there too.
        #[test]
        fn the_adip_code_is_found_wherever_the_drive_puts_it() {
            let mut copied = vec![0u8; 16];
            copied.extend_from_slice(ADIP_BLOCK);
            assert_eq!(parse_adip_id(&copied).as_deref(), Some("RITEK F16"));
            // And further along again.
            let mut later = vec![0u8; 40];
            later.extend_from_slice(ADIP_BLOCK);
            assert_eq!(parse_adip_id(&later).as_deref(), Some("RITEK F16"));
        }

        /// A block that does not have the shape of a code is not a code, and a
        /// disc whose code cannot be read is left unnamed rather than named
        /// wrongly.
        #[test]
        fn a_block_that_does_not_fit_is_not_read_as_a_code() {
            // Without the count of the bytes the block uses.
            let mut no_count = PIONEER_ADIP_ANSWER.to_vec();
            no_count[35] = 0x00;
            assert_eq!(parse_adip_id(&no_count), None);

            // With a revision no block carries.
            let mut odd_revision = PIONEER_ADIP_ANSWER.to_vec();
            odd_revision[34] = 0x1F;
            assert_eq!(parse_adip_id(&odd_revision), None);

            // With a maker that is not text.
            let mut odd_maker = PIONEER_ADIP_ANSWER.to_vec();
            odd_maker[23] = 0x01;
            assert_eq!(parse_adip_id(&odd_maker), None);
        }

        /// A DVD+R is asked for ADIP first, because that is where the standard
        /// keeps its code, and for the physical format only after that.
        #[test]
        fn a_dvd_plus_disc_is_asked_for_adip_first() {
            assert_eq!(media_id_formats("DVD+R"), &[0x11, 0x00]);
            assert_eq!(media_id_formats("DVD+RW"), &[0x11, 0x00]);
            assert_eq!(media_id_formats("DVD+R DL"), &[0x11, 0x00]);
            // A DVD-R keeps its code in a table of records, and only the physical
            // format has one.
            assert_eq!(media_id_formats("DVD-R"), &[0x00]);
            assert_eq!(media_id_formats("CD-R"), &[0x00]);
        }

        #[test]
        fn a_structure_without_the_adip_code_has_no_mid() {
            assert_eq!(parse_adip_id(&[]), None);
            assert_eq!(parse_adip_id(&[0u8; 64]), None);
            // A DVD-R structure: the code is in the record table, not at byte 23.
            assert_eq!(parse_adip_id(&disc_structure(&[b"MCC 03", b"RG20  "])), None);
        }

        #[test]
        fn a_structure_without_the_table_has_no_mid() {
            assert_eq!(parse_medium_id(&[]), None);
            assert_eq!(parse_medium_id(&[0u8; 64]), None);
            // A table whose third and fourth records are binary carries no code.
            let b = disc_structure(&[b"MCC 03", b"RG20  ", b"\x88\x80", b"\x88\x80", b"\x88\x80"]);
            assert_eq!(parse_medium_id(&b), None);
        }

        #[test]
        fn the_structure_command_puts_its_length_where_the_drive_looks() {
            // Twelve bytes, the format in byte 7 and the length in bytes 8 and 9:
            // the wrong place comes back empty.
            let c = read_disc_structure(0x11, 0x1000);
            assert_eq!(c.len(), 12);
            assert_eq!(c[0], 0xAD);
            assert_eq!(c[7], 0x11);
            assert_eq!(&c[8..10], &[0x10, 0x00]);
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
    fn an_image_must_fill_the_disc_it_is_burned_to() {
        assert!(image_fits(2_295_104, 2_295_104).is_ok());
        assert!(image_fits(2_295_104, 2_297_888).is_ok());
        assert!(image_fits(2_295_104, 0).is_ok());
        assert!(image_fits(2_295_105, 2_295_104).unwrap_err().contains("放不下"));
        // Issue #8: a CD-sized calibration image on a DVD.
        assert!(image_fits(359_849, 2_295_104).unwrap_err().contains("别的光盘"));
    }

    #[test]
    fn only_single_layer_dvd_and_cd_are_supported() {
        for m in ["DVD-R", "DVD+R", "DVD-RW", "DVD+RW", " DVD-R ", "DVD-ROM", "CD-R", "CD-RW", "CD-ROM"] {
            assert!(is_supported_media(m), "{m}");
        }
        for m in ["BD-R", "BD-RE", "DVD+R DL", "DVD-RAM", "None", ""] {
            assert!(!is_supported_media(m), "{m}");
        }
    }

    #[test]
    fn only_rewritable_discs_can_be_written_again() {
        for m in ["CD-RW", "DVD-RW", "DVD+RW", " DVD-RW "] {
            assert!(is_rewritable(m), "{m}");
        }
        for m in ["CD-R", "DVD-R", "DVD+R", "DVD+R DL", "BD-RE", "CD-ROM", "None", ""] {
            assert!(!is_rewritable(m), "{m}");
        }
    }

    #[test]
    fn a_rewritable_disc_is_offered_the_slow_end_only() {
        // A written disc is only worth a speed where the burner erases it first.
        let expected = if erases_before_burn() { (vec![2, 4], 4) } else { (Vec::new(), 0) };
        assert_eq!(derive_write_speeds("DVD+RW", false, Vec::new(), None), expected);
        let cd_expected = if erases_before_burn() { (vec![4], 4) } else { (Vec::new(), 0) };
        assert_eq!(derive_write_speeds("CD-RW", false, Vec::new(), None), cd_expected);
        // A written disc that cannot be written again is still offered nothing.
        assert_eq!(derive_write_speeds("DVD-R", false, vec![2, 4], Some(8)), (Vec::new(), 0));
        // What the drive reported is kept, up to the drive's own limit.
        if erases_before_burn() {
            assert_eq!(derive_write_speeds("DVD+RW", false, vec![2, 4, 8], Some(4)), (vec![2, 4], 4));
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