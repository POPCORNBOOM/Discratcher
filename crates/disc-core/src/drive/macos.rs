//! macOS backend.
//!
//! IOKit SCSI tasks give FUA reads and SET CD SPEED, but need exclusive access, which the
//! kernel refuses to some USB drives or while a disc is mounted. `/dev/rdiskN` is the
//! fallback: `F_NOCACHE` skips the OS cache only, so the drive's own cache has to be
//! evicted with distant reads, which `supports_fua() == false` tells the ladder to do.

use super::{derive_write_speeds, mmc, OpticalDriveInfo, TimedDrive, TimedRead};
use std::ffi::{c_char, c_int, c_void, CString};
use std::os::fd::AsRawFd;
use std::process::Command;
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
    t: Transport,
    epoch: Instant,
}

impl TimedDrive for MacDrive {
    fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
        let mut buf = vec![0u8; 2048];
        match &mut self.t {
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

    fn capacity(&mut self) -> Result<u32, String> {
        match &mut self.t {
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
        matches!(self.t, Transport::Scsi(_))
    }

    fn set_read_speed(&mut self, kbps: u16) -> Result<(), String> {
        match &mut self.t {
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

pub fn open(id: &str) -> Result<Box<dyn TimedDrive>, String> {
    if id.is_empty() {
        return Err("没有选择光驱".into());
    }
    // The disc can be remounted, or the drive briefly held, between the two steps. This
    // deserves patience: the fallback below cannot bypass the drive's cache as well.
    let mut scsi_err = String::new();
    for attempt in 0..OPEN_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(OPEN_RETRY_WAIT);
        }
        unmount(id);
        match Scsi::open(Some(id)) {
            Ok(s) => return Ok(Box::new(MacDrive { t: Transport::Scsi(s), epoch: Instant::now() })),
            Err(e) => scsi_err = e,
        }
    }
    let path = format!("/dev/r{id}");
    let f = std::fs::File::open(&path).map_err(|e| format!("{scsi_err}；{path} 也打不开：{e}"))?;
    unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) };
    Ok(Box::new(MacDrive { t: Transport::Raw(f), epoch: Instant::now() }))
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
                if speed_int > 0 {
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
                        if let Ok(v) = val_str.parse::<f32>() {
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
    let is_blank = free > 0 && used == 0;
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
        total_sectors: free.max(used),
        is_blank,
        simulated: false,
        write_speeds,
        max_write_speed,
        supported: false,
    })
}

pub fn list() -> Vec<OpticalDriveInfo> {
    let Ok(out) = Command::new("drutil").arg("status").output() else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut drive = parse_drutil(&text);
    let reported_max = parse_speed_multiplier(&field(&text, "Max Write")).or_else(|| parse_speed_multiplier(&field(&text, "Cur Write")));
    if let Some(ref mut d) = drive {
        if d.media_present && d.is_blank {
            if let Ok(xml_out) = Command::new("drutil").args(["status", "-xml"]).output() {
                let xml = String::from_utf8_lossy(&xml_out.stdout);
                let (xml_speeds, xml_max) = parse_burn_speeds_xml(&xml);
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
