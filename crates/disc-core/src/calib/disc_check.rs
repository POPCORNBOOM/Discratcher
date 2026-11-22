//! Checks around a calibration: what disc is in the drive, whether a burned calibration
//! disc holds what was written, and whether a measured geometry is a DVD at all.
//! Everything here only reads.

use crate::drive::{list_drives, open_reader, open_timed, DiscUse, TimedDrive};
use crate::ecma267::DEFAULT_TOTAL_SECTORS;
use crate::media::Media;
use crate::iso9660::{calibration_sector, CALIBRATION_VOLUME, FS_LAST_LBA};
use crate::preset::DiscGeometryPreset;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

/// Sectors read across the disc to show it was written end to end.
pub const SPOT_CHECKS: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscKind {
    None,
    /// Not a single-layer DVD or a CD.
    Unsupported,
    Blank,
    /// Carries the calibration volume label: it can be measured without burning.
    Calibration,
    /// Written with something else; calibrating needs another disc.
    Other,
    /// Its first sectors do not read: a bad burn, or one the drive has not settled on yet.
    Unreadable,
}

/// What the drive holds, decided from its listing and, for a written disc, the volume
/// descriptor at LBA 16.
pub fn inspect(id: &str) -> Result<DiscKind, String> {
    let d = list_drives().into_iter().find(|d| d.id == id).ok_or_else(|| format!("找不到光驱 {id}"))?;
    match d.disc {
        DiscUse::None => return Ok(DiscKind::None),
        DiscUse::Unsupported => return Ok(DiscKind::Unsupported),
        DiscUse::Blank => return Ok(DiscKind::Blank),
        DiscUse::Erasable | DiscUse::Written => {}
    }
    // A DVD's raw node counts in the 2048-byte sectors this reader addresses, so one can
    // be read without taking it off the desktop. A CD's counts in 2352, so a CD goes
    // through the timing path, which was written for its layout.
    let opened = if Media::of_type(&d.media_type) == Some(Media::Dvd) { open_reader(id) } else { open_timed(id) };
    let mut b = [0u8; 2048];
    let read = opened.and_then(|mut drive| drive.read_sector(16, &mut b));
    // Unreadable is its own answer, never "another disc": right after a burn the drive takes
    // a while to read the disc, and a bad burn on a rewritable disc is simply burned again.
    Ok(match read {
        Ok(()) => match parse_pvd(&b) {
            Some((label, _)) if label == CALIBRATION_VOLUME => DiscKind::Calibration,
            _ => DiscKind::Other,
        },
        Err(_) => DiscKind::Unreadable,
    })
}

/// Sectors in the calibration image this disc was burned from, when it is one.
pub fn calibration_sectors(drive: &mut dyn TimedDrive) -> Option<u32> {
    volume(drive).filter(|(label, _)| label == CALIBRATION_VOLUME).map(|v| v.1)
}

/// Label and size from the primary volume descriptor, or None when LBA 16 holds none.
fn volume(drive: &mut dyn TimedDrive) -> Option<(String, u32)> {
    let mut b = [0u8; 2048];
    drive.read_sector(16, &mut b).ok()?;
    parse_pvd(&b)
}

pub fn parse_pvd(b: &[u8; 2048]) -> Option<(String, u32)> {
    if b[0] != 1 || &b[1..6] != b"CD001" {
        return None;
    }
    let label = String::from_utf8_lossy(&b[40..72]).trim().to_string();
    let sectors = u32::from_le_bytes([b[80], b[81], b[82], b[83]]);
    Some((label, sectors))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscCheck {
    pub sectors: u32,
    /// The sectors read back did not come from Discratcher's image, consistently: a disc
    /// burned by another tool with the same label, such as disc-probe. Timing only needs
    /// readable sectors, so it is still measured.
    pub foreign: bool,
    /// Spots that would not read at all, scattered rather than a missing tail: dust or a
    /// smudge, which the measurement steps around on its own.
    pub unreadable: u32,
}

/// LBAs to read: evenly from just past the file system to the last sector.
pub fn spot_lbas(sectors: u32) -> Vec<u32> {
    let first = FS_LAST_LBA as u32 + 1;
    let last = sectors.saturating_sub(1);
    if last <= first {
        return vec![first.min(last)];
    }
    (0..SPOT_CHECKS).map(|i| first + ((last - first) as u64 * i as u64 / (SPOT_CHECKS - 1) as u64) as u32).collect()
}

/// Reads a burned calibration disc back before it is timed. A disc that was not burned
/// to the end, was burned from an image made for another disc, or does not read, fails
/// here with the reason instead of measuring into nonsense.
pub fn check_calibration_disc(
    drive: &mut dyn TimedDrive,
    media: Media,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<DiscCheck, String> {
    let (label, sectors) = volume(drive).ok_or("这张盘没有可识别的文件系统，不是校准盘")?;
    if label != CALIBRATION_VOLUME {
        return Err(format!("这张盘的卷标是「{label}」，不是校准盘"));
    }
    if let Some(e) = short_image(sectors, media) {
        return Err(e);
    }
    if media == Media::Cd {
        check_cd_layout(drive, sectors / 2)?;
    }
    if let Ok(capacity) = drive.capacity() {
        if capacity + 16 < sectors {
            return Err(format!(
                "这张盘只写了 {capacity} 个扇区，校准镜像有 {sectors} 个（约 {}%）：刻录没有完成",
                capacity as u64 * 100 / sectors as u64
            ));
        }
    }
    let lbas = spot_lbas(sectors);
    let (mut matched, mut read) = (0usize, [0u8; 2048]);
    let mut expected = [0u8; 2048];
    // Where each unreadable spot was, and the first error, to name it if it decides.
    let mut failed: Vec<usize> = Vec::new();
    let mut first_error = String::new();
    for (i, &lba) in lbas.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        match drive.read_sector(lba, &mut read) {
            Ok(()) => {
                calibration_sector(lba, sectors, &mut expected);
                if read == expected {
                    matched += 1;
                }
            }
            Err(e) => {
                if failed.is_empty() {
                    first_error = e;
                }
                failed.push(i);
                // Two misses in a row may be where the burn stopped. Ask the last spot at
                // once: if it is unreadable too, the tail is missing, and every spot in
                // between would only hold the drive in minutes of retries (the GUD1N spends
                // two to five on each), long enough for the system to give up on it.
                let last = lbas.len() - 1;
                if i + 1 < last && failed.len() >= 2 && failed[failed.len() - 2] + 1 == i && drive.read_sector(lbas[last], &mut read).is_err() {
                    let at = lbas[i - 1];
                    return Err(format!(
                        "从盘面约 {}% 处（LBA {at}）往后都读不出来，可能没刻完：{first_error}",
                        at as u64 * 100 / sectors as u64
                    ));
                }
            }
        }
        progress((i + 1) as f64 * 100.0 / lbas.len() as f64);
    }
    // A burn that stopped short leaves everything from some point on unreadable: the last
    // spots fail together. Dust fails one spot here and there, and the measurement skips
    // whatever it cannot read, so only a missing tail or a disc that hardly reads at all
    // stops the calibration here.
    let tail = failed.iter().rev().zip((0..lbas.len()).rev()).take_while(|(f, i)| *f == i).count();
    if tail >= 2 {
        let at = lbas[lbas.len() - tail];
        return Err(format!(
            "从盘面约 {}% 处（LBA {at}）往后都读不出来，可能没刻完：{first_error}",
            at as u64 * 100 / sectors as u64
        ));
    }
    if failed.len() * 4 > lbas.len() {
        return Err(format!(
            "抽查的 {} 个位置里有 {} 个读不出来，先擦干净盘面再试：{first_error}",
            lbas.len(),
            failed.len()
        ));
    }
    let mut c = verdict(sectors, matched, lbas.len() - failed.len())?;
    c.unreadable = failed.len() as u32;
    Ok(c)
}

/// A calibration image far smaller than a DVD was made for another disc (issue #8: a
/// CD-sized image on a DVD) and leaves most of the disc unmeasured.
fn short_image(sectors: u32, media: Media) -> Option<String> {
    if media == Media::Cd {
        return (sectors < media.ladder().min_sectors).then(|| format!("这张校准盘只有 {sectors} 个扇区，太少了，测不了整张盘"));
    }
    (sectors < DEFAULT_TOTAL_SECTORS / 100 * 99).then(|| {
        format!(
            "这张校准盘只有 {sectors} 个扇区，约为 DVD 的 {}%：镜像是按别的光盘大小生成的，测不了整张盘。请用一张空白 DVD 重新标定",
            sectors as u64 * 100 / DEFAULT_TOTAL_SECTORS as u64
        )
    })
}

fn verdict(sectors: u32, matched: usize, read: usize) -> Result<DiscCheck, String> {
    match matched {
        m if m == read => Ok(DiscCheck { sectors, foreign: false, unreadable: 0 }),
        0 => Ok(DiscCheck { sectors, foreign: true, unreadable: 0 }),
        m => Err(format!("抽查的 {read} 个扇区里有 {} 个内容不对，这张盘可能刻坏了", read - m)),
    }
}

/// A CD's sectors must sit where the carver will assume they do. Each burning path
/// picks Mode 1 or Mode 2 Form 1 by itself, the two put user data 8 bytes apart, and a
/// picture carved for the wrong one comes out as noise (disc-probe's first CD rings
/// disc). Transports that cannot read a raw sector skip the check.
fn check_cd_layout(drive: &mut dyn TimedDrive, lba: u32) -> Result<(), String> {
    let mut raw = [0u8; crate::ecma130::RAW_BYTES];
    if drive.read_raw_sector(lba, &mut raw).is_err() {
        return Ok(());
    }
    let expected = crate::drive::cd_user_offset();
    match crate::ecma130::user_offset_of_raw(&raw) {
        Some(found) if found == expected => Ok(()),
        Some(found) => Err(format!(
            "这台电脑把 CD 刻成了用户数据从第 {found} 字节开始的扇区，程序按第 {expected} 字节计算，刻出的图案会变成噪点。请到 GitHub 反馈这个问题"
        )),
        None => Err("这张 CD 不是数据盘（Mode 1 或 Mode 2 Form 1），无法标定".into()),
    }
}

/// A measured geometry outside what the disc family can be means the measurement went
/// wrong, however small its residual. Pitch and inner radius come from `Media::spec`;
/// a DVD's data zone must also end before 58.5 mm, with some room for discs measured so
/// far (24.17 mm and 57.88 mm on CMC MAG. AM3). CDs come in too many lengths for that.
pub fn spec_problem(p: &DiscGeometryPreset) -> Option<String> {
    let [pitch, inner] = p.media.spec();
    let mut checks = vec![("轨距", p.base_pitch_um, pitch.0, pitch.1, "µm"), ("内圈半径", p.inner_radius_mm, inner.0, inner.1, "mm")];
    if p.media == Media::Dvd {
        checks.push(("外圈半径", p.outer_radius_mm, 57.3, 58.5, "mm"));
    }
    let family = p.media.name();
    checks.iter().find(|(_, v, lo, hi, _)| !(lo..=hi).contains(&v)).map(|(name, v, lo, hi, unit)| {
        format!("测出的{name}是 {v:.4} {unit}，超出 {family} 的范围（{lo}–{hi} {unit}），测量可能出错了。请重新测量这张盘")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::sim::SimDrive;
    use crate::drive::TimedRead;
    use crate::preset::reference_preset;

    #[test]
    fn pvd_of_the_calibration_image_is_recognised() {
        let mut b = [0u8; 2048];
        calibration_sector(16, DEFAULT_TOTAL_SECTORS, &mut b);
        assert_eq!(parse_pvd(&b), Some((CALIBRATION_VOLUME.to_string(), DEFAULT_TOTAL_SECTORS)));
        calibration_sector(20, DEFAULT_TOTAL_SECTORS, &mut b);
        assert_eq!(parse_pvd(&b), None);
    }

    #[test]
    fn spot_checks_span_the_whole_disc() {
        let l = spot_lbas(DEFAULT_TOTAL_SECTORS);
        assert_eq!(l.len(), SPOT_CHECKS as usize);
        assert_eq!(l[0], FS_LAST_LBA as u32 + 1);
        assert_eq!(*l.last().unwrap(), DEFAULT_TOTAL_SECTORS - 1);
        assert!(l.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_freshly_burned_disc_passes() {
        let mut d = SimDrive::demo();
        let mut seen = 0.0;
        let c = check_calibration_disc(&mut d, Media::Dvd, &AtomicBool::new(false), &mut |p| seen = p).unwrap();
        assert_eq!(c.sectors, DEFAULT_TOTAL_SECTORS);
        assert!(!c.foreign);
        assert_eq!(seen, 100.0);
    }

    #[test]
    fn a_cd_sized_image_is_refused() {
        // Issue #8: 702 MB written to a DVD.
        assert!(short_image(359_849, Media::Dvd).unwrap().contains("别的光盘"));
        assert!(short_image(DEFAULT_TOTAL_SECTORS, Media::Dvd).is_none());
        assert!(short_image(359_779, Media::Cd).is_none());
    }

    #[test]
    fn content_decides_between_ours_foreign_and_damaged() {
        assert!(!verdict(10, 32, 32).unwrap().foreign);
        assert!(verdict(10, 0, 32).unwrap().foreign);
        assert!(verdict(10, 30, 32).unwrap_err().contains("2 个"));
    }

    /// A disc that stops reading partway, as an unfinished burn does.
    /// Dust: two spots far apart do not read, and everything else does.
    struct Dusty(SimDrive, Vec<u32>);
    impl TimedDrive for Dusty {
        fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
            self.0.read_timed(lba, fua)
        }
        fn capacity(&mut self) -> Result<u32, String> {
            self.0.capacity()
        }
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; 2048]) -> Result<(), String> {
            if self.1.contains(&lba) {
                return Err("SCSI 状态 0x02，sense 03/11/00".into());
            }
            self.0.read_sector(lba, buf)
        }
        fn supports_fua(&self) -> bool {
            true
        }
        fn set_read_speed(&mut self, _: u16) -> Result<(), String> {
            Ok(())
        }
        fn host_wait(&mut self, _: u64) {}
    }

    #[test]
    fn a_little_dust_does_not_stop_the_calibration() {
        let spots = spot_lbas(DEFAULT_TOTAL_SECTORS);
        let mut d = Dusty(SimDrive::demo(), vec![spots[1], spots[17]]);
        let c = check_calibration_disc(&mut d, Media::Dvd, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(c.unreadable, 2);
        assert!(!c.foreign);
        // A disc that hardly reads at all is still turned away.
        let mut d = Dusty(SimDrive::demo(), spots.iter().step_by(3).copied().collect());
        let e = check_calibration_disc(&mut d, Media::Dvd, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.contains("擦干净"), "{e}");
    }

    struct Unfinished(SimDrive, u32);
    impl TimedDrive for Unfinished {
        fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
            self.0.read_timed(lba, fua)
        }
        fn capacity(&mut self) -> Result<u32, String> {
            self.0.capacity()
        }
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; 2048]) -> Result<(), String> {
            if lba >= self.1 {
                return Err("介质错误".into());
            }
            self.0.read_sector(lba, buf)
        }
        fn supports_fua(&self) -> bool {
            true
        }
        fn set_read_speed(&mut self, _: u16) -> Result<(), String> {
            Ok(())
        }
        fn host_wait(&mut self, _: u64) {}
    }

    /// Counts the reads that failed, which on a real drive cost minutes each.
    struct Counting(Unfinished, u32);
    impl TimedDrive for Counting {
        fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
            self.0.read_timed(lba, fua)
        }
        fn capacity(&mut self) -> Result<u32, String> {
            self.0.capacity()
        }
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; 2048]) -> Result<(), String> {
            let r = self.0.read_sector(lba, buf);
            self.1 += r.is_err() as u32;
            r
        }
        fn supports_fua(&self) -> bool {
            true
        }
        fn set_read_speed(&mut self, _: u16) -> Result<(), String> {
            Ok(())
        }
        fn host_wait(&mut self, _: u64) {}
    }

    #[test]
    fn a_missing_tail_is_found_without_reading_all_of_it() {
        let mut d = Counting(Unfinished(SimDrive::demo(), DEFAULT_TOTAL_SECTORS * 2 / 3), 0);
        let e = check_calibration_disc(&mut d, Media::Dvd, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.contains("往后都读不出来"), "{e}");
        assert_eq!(d.1, 3, "two misses and the last spot");
    }

    #[test]
    fn an_unreadable_stretch_names_where_it_is() {
        let mut d = Unfinished(SimDrive::demo(), DEFAULT_TOTAL_SECTORS / 2);
        let e = check_calibration_disc(&mut d, Media::Dvd, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.contains("读不出来"), "{e}");
    }

    #[test]
    fn the_reference_geometry_is_a_dvd_and_a_wild_one_is_not() {
        let mut p = reference_preset();
        assert_eq!(spec_problem(&p), None);
        p.base_pitch_um = 0.81;
        assert!(spec_problem(&p).unwrap().contains("轨距"));
        p = reference_preset();
        p.inner_radius_mm = 22.0;
        assert!(spec_problem(&p).unwrap().contains("内圈半径"));
    }
}
