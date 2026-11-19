//! macOS backend.
//!
//! IOKit SCSI tasks give FUA reads and SET CD SPEED, but need exclusive access, which the
//! kernel refuses to some USB drives or while a disc is mounted. `/dev/rdiskN` is the
//! fallback: `F_NOCACHE` skips the OS cache only, so the drive's own cache has to be
//! evicted with distant reads, which `supports_fua() == false` tells the ladder to do.

use super::{derive_write_speeds, is_rewritable, mmc, OpticalDriveInfo, TimedDrive, TimedRead};
use std::ffi::{c_char, c_int, c_void, CString};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

extern "C" {
    fn ds_open(bsd: *const c_char, err: *mut c_int) -> *mut c_void;
    fn ds_close(h: *mut c_void);
    fn ds_exec(
        h: *mut c_void,
        cdb: *const u8,
        cdb_len: u8,
        direction: c_int,
        buf: *mut u8,
        len: u32,
        timeout_ms: u32,
        status: *mut u8,
        sense: *mut u8,
        t_end_ns: *mut f64,
    ) -> c_int;
}

const OPEN_ATTEMPTS: usize = 4;
const OPEN_RETRY_WAIT: Duration = Duration::from_secs(1);

pub struct Scsi(*mut c_void);

// The handle is only ever used from the thread that owns it.
unsafe impl Send for Scsi {}

impl Drop for Scsi {
    fn drop(&mut self) {
        unsafe { ds_close(self.0) }
    }
}

impl Scsi {
    pub fn open(bsd: Option<&str>) -> Result<Self, String> {
        let name = bsd.map(|b| CString::new(b).unwrap());
        let mut err = 0;
        let h = unsafe { ds_open(name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()), &mut err) };
        if h.is_null() {
            return Err(format!("IOKit 打开失败：0x{:08X}", err as u32));
        }
        Ok(Self(h))
    }

    /// Returns (completion ns). Fails on any non-GOOD status, with its sense data.
    pub fn exec(&mut self, cdb: &[u8], data: Option<&mut [u8]>) -> Result<f64, String> {
        let (dir, ptr, len) = match data {
            Some(d) => (2, d.as_mut_ptr(), d.len() as u32),
            None => (0, std::ptr::null_mut(), 0),
        };
        let (mut status, mut sense, mut t) = (0u8, [0u8; 3], 0f64);
        let rc = unsafe {
            ds_exec(self.0, cdb.as_ptr(), cdb.len() as u8, dir, ptr, len, 30_000, &mut status, sense.as_mut_ptr(), &mut t)
        };
        if rc != 0 {
            return Err(format!("SCSI 任务失败：0x{:08X}", rc as u32));
        }
        if status != 0 {
            return Err(format!(
                "SCSI 状态 {status:#04x}，sense {:02X}/{:02X}/{:02X}",
                sense[0], sense[1], sense[2]
            ));
        }
        Ok(t)
    }
}

/// `true` when the read bypasses the cache per spec of the transport.
enum Transport {
    Scsi(Scsi),
    Raw(std::fs::File),
}

pub struct MacDrive {
    /// Empty only while the handle is being released, so the disc can be mounted back
    /// after the drive is no longer held.
    t: Option<Transport>,
    epoch: Instant,
    /// A disc that was on the desktop when this session took the drive: the unmount is
    /// this app's doing and is undone on release. Left unmounted, the disc cannot be
    /// ejected from the Finder, and eject then looks like it does nothing.
    remount: Option<String>,
}

impl MacDrive {
    fn t(&mut self) -> &mut Transport {
        self.t.as_mut().expect("驱动已经关闭")
    }
}

impl Drop for MacDrive {
    fn drop(&mut self) {
        let Some(bsd) = self.remount.take() else { return };
        // Release the drive first: exclusive access refuses a mount as well.
        self.t = None;
        let _ = Command::new("diskutil").args(["mountDisk", &bsd]).output();
    }
}

impl TimedDrive for MacDrive {
    fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
        let mut buf = vec![0u8; 2048];
        match self.t() {
            Transport::Scsi(s) => {
                let t_end_ns = s.exec(&mmc::read12(lba, 1, fua), Some(&mut buf))?;
                Ok(TimedRead { t_end_ns })
            }
            Transport::Raw(f) => {
                use std::os::unix::fs::FileExt;
                f.read_exact_at(&mut buf, lba as u64 * 2048).map_err(|e| format!("读取 LBA {lba} 失败：{e}"))?;
                Ok(TimedRead { t_end_ns: self.epoch.elapsed().as_nanos() as f64 })
            }
        }
    }

    fn read_sector(&mut self, lba: u32, buf: &mut [u8; 2048]) -> Result<(), String> {
        match self.t() {
            Transport::Scsi(s) => s.exec(&mmc::read12(lba, 1, false), Some(&mut buf[..])).map(|_| ()),
            Transport::Raw(f) => {
                use std::os::unix::fs::FileExt;
                f.read_exact_at(buf, lba as u64 * 2048).map_err(|e| format!("读取 LBA {lba} 失败：{e}"))
            }
        }
    }

    fn read_raw_sector(&mut self, lba: u32, buf: &mut [u8; 2352]) -> Result<(), String> {
        match self.t() {
            Transport::Scsi(s) => s.exec(&mmc::read_cd_raw(lba), Some(&mut buf[..])).map(|_| ()),
            Transport::Raw(_) => Err("这个通道读不了原始扇区".into()),
        }
    }

    fn capacity(&mut self) -> Result<u32, String> {
        match self.t() {
            Transport::Scsi(s) => {
                let mut b = [0u8; 8];
                s.exec(&mmc::read_capacity(), Some(&mut b))?;
                Ok(mmc::parse_capacity(&b))
            }
            Transport::Raw(f) => {
                let mut bytes: u64 = 0;
                let mut block: u32 = 0;
                // DKIOCGETBLOCKCOUNT and DKIOCGETBLOCKSIZE: file metadata reports 0 on a device.
                const COUNT: libc::c_ulong = 0x4008_6419;
                const SIZE: libc::c_ulong = 0x4004_6418;
                let rc = unsafe {
                    libc::ioctl(f.as_raw_fd(), COUNT, &mut bytes) | libc::ioctl(f.as_raw_fd(), SIZE, &mut block)
                };
                if rc != 0 {
                    return Err("读不到光盘容量".into());
                }
                Ok((bytes * block.max(512) as u64 / 2048) as u32)
            }
        }
    }

    fn supports_fua(&self) -> bool {
        matches!(self.t.as_ref(), Some(Transport::Scsi(_)))
    }

    fn set_read_speed(&mut self, kbps: u16) -> Result<(), String> {
        match self.t() {
            Transport::Scsi(s) => s.exec(&mmc::set_speed(kbps), None).map(|_| ()),
            Transport::Raw(_) => Ok(()),
        }
    }

    fn host_wait(&mut self, ns: u64) {
        std::thread::sleep(Duration::from_nanos(ns));
    }
}

/// Release the disc so exclusive access can succeed. Harmless on an unmounted disc.
fn unmount(bsd: &str) {
    let _ = Command::new("diskutil").args(["unmountDisk", bsd]).output();
}

/// The kernel's mount table. Cheap, and it never touches the drive.
fn mount_table() -> String {
    Command::new("mount").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Whether a volume of this disc is mounted, judged by the source the kernel lists for it
/// (`/dev/disk8s0 on /Volumes/UNTITLED`).
pub fn parse_mount(table: &str, bsd: &str) -> bool {
    table.lines().any(|l| {
        l.split_whitespace()
            .next()
            .and_then(|src| src.strip_prefix("/dev/"))
            .and_then(|src| src.strip_prefix(bsd))
            .is_some_and(|slice| slice.starts_with('s'))
    })
}

/// Whether `drutil status -xml` marks the disc itself blank, as `<blank/>` right after
/// `<mediaInfo>`: the same flag as the text output, without a word to translate. A
/// track's own `<blank/>` sits deeper and must not count.
pub fn xml_is_blank(xml: &str) -> bool {
    xml.split("<mediaInfo>").skip(1).any(|rest| rest.trim_start().starts_with("<blank/>"))
}

/// A CD's whole-disc node hands out 2352-byte raw sectors; its data track is slice 0.
fn raw_file(id: &str) -> Result<std::fs::File, String> {
    let track = format!("/dev/r{id}s0");
    let path = if std::path::Path::new(&track).exists() { track } else { format!("/dev/r{id}") };
    let f = std::fs::File::open(&path).map_err(|e| format!("{path} 打不开：{e}"))?;
    unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) };
    Ok(f)
}

pub fn open(id: &str) -> Result<Box<dyn TimedDrive>, String> {
    if id.is_empty() {
        return Err("没有选择光驱".into());
    }
    // Unmounting takes the disc off the desktop, and only this app can put it back.
    let remount = parse_mount(&mount_table(), id).then(|| id.to_string());
    // The disc can be remounted, or the drive briefly held, between the two steps. This
    // deserves patience: the fallback below cannot bypass the drive's cache as well.
    let mut scsi_err = String::new();
    for attempt in 0..OPEN_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(OPEN_RETRY_WAIT);
        }
        unmount(id);
        match Scsi::open(Some(id)) {
            Ok(s) => {
                return Ok(Box::new(MacDrive {
                    t: Some(Transport::Scsi(s)),
                    epoch: Instant::now(),
                    remount: remount.clone(),
                }))
            }
            Err(e) => scsi_err = e,
        }
    }
    let f = raw_file(id).map_err(|e| format!("{scsi_err}；{e}"))?;
    Ok(Box::new(MacDrive { t: Some(Transport::Raw(f)), epoch: Instant::now(), remount }))
}

/// `diskutil` takes a mounted disc off the desktop first; `drutil` reaches a blank disc
/// and one left unmounted.
pub fn eject(id: &str) -> Result<(), String> {
    let by_diskutil = !id.is_empty() && Command::new("diskutil").args(["eject", id]).output().is_ok_and(|o| o.status.success());
    if by_diskutil || drutil(&["eject"]).is_some_and(|o| o.status.success()) {
        Ok(())
    } else if stuck() {
        Err(STUCK_MESSAGE.into())
    } else {
        Err("光驱没有弹出，可能有别的程序正在使用这张光盘".into())
    }
}

/// Read the disc without taking it away from the system: no unmount, no exclusive access.
/// What the app reads to tell a calibration disc from another must not stop the user from
/// ejecting it, and must not hold the drive the 2 s poll after a burn keeps asking.
pub fn open_reader(id: &str) -> Result<Box<dyn TimedDrive>, String> {
    if id.is_empty() {
        return Err("没有选择光驱".into());
    }
    Ok(Box::new(MacDrive { t: Some(Transport::Raw(raw_file(id)?)), epoch: Instant::now(), remount: None }))
}

/// Value after `key:` on any line of `drutil status` or `drutil info` output.
fn field(text: &str, key: &str) -> String {
    text.lines()
        .find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim_start_matches(':').trim().to_string()))
        // Columns share a line ("Type: DVD-R      Name: /dev/disk8"); two spaces end a value.
        .map(|v| v.split("  ").next().unwrap_or_default().to_string())
        .unwrap_or_default()
}

pub fn parse_speed_multiplier(s: &str) -> Option<u8> {
    let digits: String = s.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u8>().ok()
}

pub fn parse_burn_speeds_xml(xml: &str) -> (Vec<u8>, Option<u8>) {
    let mut speeds = Vec::new();
    let mut max_speed = None;

    if let Some(rest) = xml.split("maxSpeed=\"").nth(1) {
        if let Some(val_str) = rest.split('"').next() {
            if let Ok(v) = val_str.parse::<f32>() {
                let speed_int = v.round() as u8;
                // drutil gives kB/s here for a CD ("4233.6"): that is not a multiple.
                if speed_int > 0 && v <= 64.0 {
                    max_speed = Some(speed_int);
                }
            }
        }
    }

    for part in xml.split("<burnSpeed") {
        if let Some(attr) = part.split('>').next() {
            for key in &["dvdXFactor=\"", "cdXFactor=\"", "factor=\"", "speed=\""] {
                if let Some(rest) = attr.split(key).nth(1) {
                    if let Some(val_str) = rest.split('"').next() {
                        // A CD's factor reads "16x", a DVD's "4.0".
                        if let Ok(v) = val_str.trim_end_matches('x').parse::<f32>() {
                            let speed_int = v.round() as u8;
                            if speed_int > 0 && !speeds.contains(&speed_int) {
                                speeds.push(speed_int);
                            }
                        }
                    }
                }
            }
        }
    }
    speeds.sort_unstable();
    (speeds, max_speed)
}

pub fn parse_drutil(text: &str) -> Option<OpticalDriveInfo> {
    let lines: Vec<&str> = text.lines().collect();
    let header = lines.iter().position(|l| l.trim_start().starts_with("Vendor"))?;
    let row = lines.get(header + 1)?;
    let cols: Vec<&str> = row.split_whitespace().collect();
    if cols.len() < 2 {
        return None;
    }
    let vendor = cols[0].to_string();
    let revision = cols[cols.len() - 1].to_string();
    let product = cols[1..cols.len() - 1].join(" ");
    let media_type = field(text, "Type");
    let media_present = !media_type.is_empty() && !media_type.contains("No Media");
    // Device line reads "/dev/disk8" when a disc is in.
    let device = text
        .split_whitespace()
        .find_map(|w| w.strip_prefix("/dev/"))
        .map(str::to_string)
        .unwrap_or_default();
    // "Space Free: 00:00:00 blocks: 2295104 / 4.70GB / 4.38GiB"
    let blocks = |key: &str| -> u32 {
        text.lines()
            .find(|l| l.trim_start().starts_with(key))
            .and_then(|l| l.split("blocks:").nth(1)?.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    };
    let (free, used) = (blocks("Space Free"), blocks("Space Used"));
    // drutil reads READ DISC INFORMATION itself and reports the result on this line, where
    // "blank" means nothing was ever written. The block counts cannot stand in for it: a
    // formatted DVD-RW and an erased CD-RW report no free blocks and no used ones either,
    // and counting them said "written" for a disc with nothing on it.
    let writability = field(text, "Writability");
    let is_blank = if writability.is_empty() { used == 0 } else { writability.contains("blank") };
    // A rewritable disc is erased before it is written, so all of it can be used: drutil
    // reports the written part as used and sometimes nothing free, and the disc would
    // look disc-sized only if what is on it happens to fill it. "Overwritable" is the
    // whole formatted surface, which is what an image has to be sized to.
    let total_sectors = if is_rewritable(&media_type) {
        match free.max(used).max(blocks("Overwritable")) {
            0 => mmc::nominal_sectors(&media_type),
            n => n,
        }
    } else {
        free.max(used)
    };
    let max_speed_text = parse_speed_multiplier(&field(text, "Max Write"))
        .or_else(|| parse_speed_multiplier(&field(text, "Cur Write")));
    let (write_speeds, max_write_speed) = if media_present {
        derive_write_speeds(&media_type, is_blank, Vec::new(), max_speed_text)
    } else {
        (Vec::new(), 0)
    };

    Some(OpticalDriveInfo {
        id: device,
        vendor,
        product,
        revision,
        media_present,
        media_type: if media_present { media_type } else { "None".into() },
        media_id: field(text, "Media ID"),
        total_sectors,
        is_blank,
        write_speeds,
        max_write_speed,
        supported: false,
        reusable: false,
        unsupported_reason: None,
    })
}

/// drutil answers in well under a second. When the system loses track of a drive (a held
/// drive unplugged, say) it waits in the kernel for good, and not even SIGKILL ends it.
const DRUTIL_TIMEOUT: Duration = Duration::from_secs(8);

/// A drutil that never came back. While it waits, every new one would wait with it.
static STUCK: Mutex<Option<Child>> = Mutex::new(None);

pub const STUCK_MESSAGE: &str = "系统的光驱服务没有响应。重新插拔光驱，还不行就重启电脑";

/// Whether the system's optical drive service has stopped answering. Replugging the drive
/// sometimes clears it; otherwise only a restart does.
pub fn stuck() -> bool {
    let mut held = STUCK.lock().unwrap();
    if let Some(c) = held.as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            return true;
        }
    }
    *held = None;
    false
}

/// Runs drutil, giving up after `DRUTIL_TIMEOUT`. Its output is a few kB, well inside what
/// the pipe holds, so waiting for exit before reading cannot deadlock.
fn drutil(args: &[&str]) -> Option<Output> {
    if stuck() {
        return None;
    }
    let mut child = Command::new("drutil").args(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let began = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if began.elapsed() < DRUTIL_TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                *STUCK.lock().unwrap() = Some(child);
                return None;
            }
        }
    }
}

pub fn list() -> Vec<OpticalDriveInfo> {
    let Some(out) = drutil(&["status"]) else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut drive = parse_drutil(&text);
    let reported_max = parse_speed_multiplier(&field(&text, "Max Write")).or_else(|| parse_speed_multiplier(&field(&text, "Cur Write")));
    if let Some(ref mut d) = drive {
        if d.media_present && (d.is_blank || is_rewritable(&d.media_type)) {
            if let Some(xml_out) = drutil(&["status", "-xml"]) {
                let xml = String::from_utf8_lossy(&xml_out.stdout);
                let (xml_speeds, xml_max) = parse_burn_speeds_xml(&xml);
                // The framework's own flag, in case the text it printed was in another
                // language than the one this parser reads.
                d.is_blank |= xml_is_blank(&xml);
                let (speeds, max_speed) = derive_write_speeds(
                    &d.media_type,
                    d.is_blank,
                    xml_speeds,
                    // Not d.max_write_speed: that may be the low guess made when nothing was reported.
                    xml_max.or(reported_max),
                );
                if !speeds.is_empty() {
                    d.write_speeds = speeds;
                    d.max_write_speed = max_speed;
                }
            }
        }
    }
    drive.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_drive() {
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: No Media Inserted\n";
        let d = parse_drutil(t).unwrap();
        assert_eq!((d.vendor.as_str(), d.product.as_str(), d.revision.as_str()), ("HL-DT-ST", "DVDRAM GUD1N", "1.01"));
        assert!(!d.media_present);
        assert_eq!(d.media_type, "None");
        assert!(d.write_speeds.is_empty());
        assert_eq!(d.max_write_speed, 0);
    }

    #[test]
    fn parses_blank_disc() {
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: DVD-R                Name: /dev/disk8\n       Sessions: 0                  Tracks: 0 \n   Overwritable:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n     Space Free:  510:00:44         blocks:  2295104 /   4.70GB /   4.38GiB\n     Space Used:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n    Writability: appendable, blank, overwritable\n";
        let d = parse_drutil(t).unwrap();
        assert_eq!(d.id, "disk8");
        assert_eq!(d.media_type, "DVD-R");
        assert_eq!(d.total_sectors, 2_295_104);
        assert!(d.is_blank && d.media_present);
        // Nothing reported the speed, so it must not be assumed fast.
        assert_eq!(d.max_write_speed, 4);
        assert_eq!(d.write_speeds, vec![2, 4]);
    }

    #[test]
    fn parses_disc_with_max_speed() {
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: DVD-R                Name: /dev/disk8\n      Cur Write: 8x DVD           Sessions: 0 \n      Max Write: 8x DVD             Tracks: 0 \n     Space Free:  510:00:44         blocks:  2295104 /   4.70GB /   4.38GiB\n     Space Used:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n    Writability: appendable, blank, overwritable\n";
        let d = parse_drutil(t).unwrap();
        assert_eq!(d.max_write_speed, 8);
        assert_eq!(d.write_speeds, vec![2, 4, 6, 8]);
    }

    #[test]
    fn parses_xml_burn_speed_list() {
        let xml = r#"
        <deviceStatus>
            <writeSpeed maxSpeed="8" currentSpeed="8"/>
            <mediaInfo>
                <mediaType value="DVD-R"/>
                <burnSpeedList>
                    <burnSpeed dvdXFactor="2.0"/>
                    <burnSpeed dvdXFactor="4.0"/>
                    <burnSpeed dvdXFactor="8.0"/>
                </burnSpeedList>
            </mediaInfo>
        </deviceStatus>
        "#;
        let (speeds, max_speed) = parse_burn_speeds_xml(xml);
        assert_eq!(speeds, vec![2, 4, 8]);
        assert_eq!(max_speed, Some(8));
    }

    /// What drutil printed for a blank CD-R in the GUD1N.
    #[test]
    fn parses_cd_burn_speeds() {
        let xml = r#"<writeSpeed maxSpeed="4233.6" currentSpeed="4233.6"/>
            <burnSpeedList>
                <burnSpeed value="1764" cdXFactor="10x"/>
                <burnSpeed value="2822" cdXFactor="16x"/>
                <burnSpeed value="4233" cdXFactor="24x"/>
            </burnSpeedList>"#;
        assert_eq!(parse_burn_speeds_xml(xml), (vec![10, 16, 24], None));
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: CD-R                 Name: /dev/disk9\n   Write Speeds: 10x, 16x, 24x\n   Overwritable:   79:57:68         blocks:   359843 / 736.96MB / 702.82MiB\n     Space Free:   79:57:68         blocks:   359843 / 736.96MB / 702.82MiB\n     Space Used:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n    Writability: appendable, blank, overwritable\n";
        let d = parse_drutil(t).unwrap();
        assert_eq!((d.media_type.as_str(), d.total_sectors, d.is_blank), ("CD-R", 359_843, true));
    }

    /// What drutil prints for a DVD+RW that has been burned before: the written part is
    /// used, nothing is free, and only "Overwritable" still describes the whole disc.
    #[test]
    fn sizes_a_used_dvd_plus_rw_to_the_disc_not_to_its_data() {
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: DVD+RW               Name: /dev/disk8\n       Sessions: 1                  Tracks: 1 \n   Overwritable:  510:00:44         blocks:  2295104 /   4.70GB /   4.38GiB\n     Space Free:    0:00:00         blocks:        0 /   0.00MB /   0.00MiB\n     Space Used:  445:12:00         blocks:  2000000 /   4.10GB /   3.81GiB\n    Writability: overwritable\n";
        let d = parse_drutil(t).unwrap();
        assert_eq!((d.media_type.as_str(), d.is_blank), ("DVD+RW", false));
        assert_eq!(d.total_sectors, 2_295_104);
        // A rewritable disc is offered the slow end, not nothing.
        if crate::drive::erases_before_burn() {
            assert_eq!((d.write_speeds.clone(), d.max_write_speed), (vec![2, 4], 4));
        }
    }

    /// A used CD-RW, and one drutil could not size at all until it is erased again.
    #[test]
    fn sizes_a_used_cd_rw_to_the_disc_not_to_its_data() {
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: CD-RW                Name: /dev/disk9\n   Overwritable:   79:57:68         blocks:   359843 / 736.96MB / 702.82MiB\n     Space Free:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n     Space Used:   79:57:68         blocks:   359843 / 736.96MB / 702.82MiB\n    Writability: overwritable\n";
        let d = parse_drutil(t).unwrap();
        assert_eq!((d.media_type.as_str(), d.is_blank), ("CD-RW", false));
        assert_eq!(d.total_sectors, 359_843);

        // Nothing reported: the nominal size of the disc, never zero.
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: CD-RW                Name: /dev/disk9\n   Overwritable:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n     Space Free:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n     Space Used:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n    Writability: overwritable\n";
        let d = parse_drutil(t).unwrap();
        assert_eq!(d.total_sectors, 359_849);
    }

    /// A formatted, never-written DVD-RW: drutil reports no free blocks and no used ones,
    /// and the disc still holds nothing. The block counts said this fresh disc was written.
    #[test]
    fn a_formatted_blank_dvd_rw_is_blank() {
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: DVD-RW               Name: /dev/disk8\n       Sessions: 0                  Tracks: 0 \n   Overwritable:  510:00:44         blocks:  2295104 /   4.70GB /   4.38GiB\n     Space Free:    0:00:00         blocks:        0 /   0.00MB /   0.00MiB\n     Space Used:    0:00:00         blocks:        0 /   0.00MB /   0.00MiB\n    Writability: overwritable, blank\n";
        let d = parse_drutil(t).unwrap();
        assert!(d.is_blank);
        assert_eq!(d.total_sectors, 2_295_104);
    }

    /// An erased CD-RW, likewise: nothing free, nothing used, nothing on it.
    #[test]
    fn an_erased_cd_rw_is_blank() {
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: CD-RW                Name: /dev/disk9\n   Overwritable:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n     Space Free:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n     Space Used:   00:00:00         blocks:        0 /   0.00MB /   0.00MiB\n    Writability: overwritable, blank\n";
        let d = parse_drutil(t).unwrap();
        assert!(d.is_blank);
        assert_eq!(d.total_sectors, 359_849);
    }

    /// A disc with something on it never carries the blank flag.
    #[test]
    fn a_written_disc_is_not_blank() {
        let t = " Vendor   Product           Rev \n HL-DT-ST DVDRAM GUD1N      1.01\n\n           Type: DVD-R                Name: /dev/disk8\n       Sessions: 1                  Tracks: 1 \n     Space Free:  421:00:00         blocks:  1900000 /   3.90GB /   3.63GiB\n     Space Used:   89:00:44         blocks:   395104 / 772.47MB / 736.64MiB\n    Writability: appendable\n";
        let d = parse_drutil(t).unwrap();
        assert!(!d.is_blank);
    }

    /// The XML marks the disc, not a track on it: a written disc has a `<blank/>` inside
    /// its trackinfo and must still read as written.
    #[test]
    fn the_xml_blank_flag_is_the_disc_not_a_track() {
        let blank = "<mediaInfo>\n<blank/>\n<usedSpace msf=\"00:00:00\"/>\n</mediaInfo>";
        assert!(xml_is_blank(blank));
        let written = "<mediaInfo>\n<usedSpace msf=\"01:00:00\"/>\n<trackInfoList><trackinfo><blank/></trackinfo></trackInfoList>\n</mediaInfo>";
        assert!(!xml_is_blank(written));
        assert!(!xml_is_blank("<statusdoc/>"));
    }

    /// The mount table says which disc the desktop has, and so which one an unmount is
    /// taking away from the user.
    #[test]
    fn the_mount_table_names_the_discs_on_the_desktop() {
        let t = "/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)\n/dev/disk8s0 on /Volumes/UNTITLED (cd9660, local, nodev, nosuid, read-only, noowners)\n";
        assert!(parse_mount(t, "disk8"));
        assert!(!parse_mount(t, "disk4"));
        assert!(!parse_mount(t, "disk80"));
        assert!(!parse_mount("", "disk8"));
    }

    #[test]
    fn exclusive_inquiry_on_real_drive_when_requested() {
        if std::env::var("DISC_HW").is_err() {
            return;
        }
        let mut s = Scsi::open(None).expect("open");
        let mut b = [0u8; 96];
        s.exec(&[0x12, 0, 0, 0, 96, 0], Some(&mut b)).expect("inquiry");
        let id = String::from_utf8_lossy(&b[8..32]).to_string();
        eprintln!("INQUIRY: {id}");
        assert!(id.contains("GUD1N"), "{id}");
    }
}
