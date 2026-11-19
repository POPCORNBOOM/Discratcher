//! Windows backend: IOCTL_SCSI_PASS_THROUGH_DIRECT.
//!
//! There is no drutil to ask here, so the drive is questioned with the same MMC
//! commands the other transports send: INQUIRY for the model, GET CONFIGURATION for
//! what is loaded, READ DISC INFORMATION for whether it is blank, READ CAPACITY for
//! its size. SPTI carries FUA and SET CD SPEED, so a timed read is byte-for-byte the
//! command macOS sends over IOKit.

use super::{mmc, OpticalDriveInfo, TimedDrive, TimedRead};
use std::ffi::c_void;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// Where a drive can be found: the class driver names every optical device, whether
/// or not it has a letter or a disc.
const DEVICE_PREFIX: &str = r"\\.\CdRom";
/// Enough device numbers for any machine; missing ones fail to open at once.
const DEVICE_SCAN: u32 = 16;

/// SPTI is refused on a read-only handle, so the drive has to be opened for writing
/// even to ask it a question.
const DEVICE_ACCESS: u32 = GENERIC_READ | GENERIC_WRITE;

const IOCTL_SCSI_PASS_THROUGH_DIRECT: u32 = 0x0004_D014;
/// Answers which drive a handle belongs to, whatever name it was opened under.
const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002D_1080;
/// Asks whether the medium changed, without reading the medium: the commands that do
/// read it spin the drive up, and the page asks every three seconds.
const IOCTL_STORAGE_CHECK_VERIFY: u32 = 0x002D_0801;
const SCSI_IOCTL_DATA_IN: u8 = 1;
const SCSI_IOCTL_DATA_UNSPECIFIED: u8 = 2;
/// Long enough to spin a disc up and seek, short enough that a wedged USB bridge
/// cannot hold the whole drive list hostage.
const TIMEOUT_S: u32 = 20;
/// ERROR_NOT_READY, ERROR_MEDIUM_NOT_PRESENT.
const MEDIUM_ABSENT: [i32; 2] = [21, 55];
const ACCESS_DENIED: i32 = 5;
/// Sense key 6: a freshly opened drive answers once with UNIT ATTENTION.
const UNIT_ATTENTION: u8 = 6;

const TEST_UNIT_READY: [u8; 6] = [0x00; 6];
const INQUIRY: [u8; 6] = [0x12, 0, 0, 0, 36, 0];
/// GET CONFIGURATION with RT=1: the current profile of the loaded medium.
const GET_CURRENT_PROFILE: [u8; 10] = [0x46, 0x01, 0, 0, 0, 0, 0, 0, 8, 0];
const READ_DISC_INFORMATION: [u8; 10] = [0x51, 0, 0, 0, 0, 0, 0, 0, 34, 0];
/// READ DISC STRUCTURE answers with a structure a couple of kilobytes long; the
/// maker and the media code sit well past the header.
const DISC_STRUCTURE_BYTES: usize = 4096;
/// Some USB bridges never answer this command at all (measured on a GUD1N over
/// USB). The drive list is polled every three seconds and the full probe runs
/// whenever the medium changed, so a bridge that swallows the command must not
/// hold the list for as long as the general timeout lasts. A few seconds is
/// enough for a drive that answers at all: the disc is awake by then.
const DISC_STRUCTURE_TIMEOUT_S: u32 = 5;

/// STORAGE_DEVICE_NUMBER, filled in by IOCTL_STORAGE_GET_DEVICE_NUMBER.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct StorageDeviceNumber {
    device_type: u32,
    device_number: u32,
    partition_number: u32,
}

#[repr(C)]
struct Sptd {
    length: u16,
    scsi_status: u8,
    path_id: u8,
    target_id: u8,
    lun: u8,
    cdb_length: u8,
    sense_info_length: u8,
    data_in: u8,
    data_transfer_length: u32,
    timeout_value: u32,
    data_buffer: *mut c_void,
    sense_info_offset: u32,
    cdb: [u8; 16],
}

#[repr(C)]
struct SptdSense {
    sptd: Sptd,
    sense: [u8; 32],
}

/// Why a command did not come back clean. Which way it failed matters: an empty tray
/// and an unsupported command are both errors, but only one of them means no disc.
#[derive(Clone, Copy, Debug, Default)]
struct Failure {
    /// GetLastError, when the ioctl refused the command outright.
    win32: Option<i32>,
    scsi_status: u8,
    sense_key: u8,
    asc: u8,
    ascq: u8,
}

impl Failure {
    fn medium_absent(&self) -> bool {
        self.win32.is_some_and(|c| MEDIUM_ABSENT.contains(&c)) || (self.sense_key == 0x02 && self.asc == 0x3A)
    }

    fn retryable(&self) -> bool {
        self.sense_key == UNIT_ATTENTION
    }

    /// Named in the message: a sense code alone says nothing about which step failed.
    fn text(&self, op: u8) -> String {
        let name = command_name(op);
        match self.win32 {
            Some(c) if c == ACCESS_DENIED => format!("{name} 被拒绝：需要以管理员身份运行"),
            Some(c) => format!("{name} 失败：{}", std::io::Error::from_raw_os_error(c)),
            None => format!(
                "{name} 失败：SCSI 状态 {:#04x}，sense {:02X}/{:02X}/{:02X}",
                self.scsi_status, self.sense_key, self.asc, self.ascq
            ),
        }
    }
}

fn command_name(op: u8) -> &'static str {
    match op {
        0x00 => "TEST UNIT READY",
        0x12 => "INQUIRY",
        0x25 => "READ CAPACITY",
        0x46 => "GET CONFIGURATION",
        0x51 => "READ DISC INFORMATION",
        0xA8 => "READ(12)",
        0xBE => "READ CD",
        0xBB => "SET CD SPEED",
        0xAD => "READ DISC STRUCTURE",
        _ => "SCSI 命令",
    }
}

pub struct WinDrive {
    h: HANDLE,
    epoch: Instant,
}

unsafe impl Send for WinDrive {}

impl Drop for WinDrive {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.h) };
    }
}

impl WinDrive {
    fn exec_once(&mut self, cdb: &[u8], data: Option<&mut [u8]>, timeout_s: u32) -> Result<(), Failure> {
        let (dir, ptr, len) = match data {
            Some(d) => (SCSI_IOCTL_DATA_IN, d.as_mut_ptr() as *mut c_void, d.len() as u32),
            None => (SCSI_IOCTL_DATA_UNSPECIFIED, std::ptr::null_mut(), 0),
        };
        let mut c = [0u8; 16];
        c[..cdb.len()].copy_from_slice(cdb);
        let mut r = SptdSense {
            sptd: Sptd {
                length: std::mem::size_of::<Sptd>() as u16,
                scsi_status: 0,
                path_id: 0,
                target_id: 0,
                lun: 0,
                cdb_length: cdb.len() as u8,
                sense_info_length: 32,
                data_in: dir,
                data_transfer_length: len,
                timeout_value: timeout_s,
                data_buffer: ptr,
                sense_info_offset: std::mem::size_of::<Sptd>() as u32,
                cdb: c,
            },
            sense: [0; 32],
        };
        let mut got = 0u32;
        let size = std::mem::size_of::<SptdSense>() as u32;
        let p = &mut r as *mut SptdSense as *mut c_void;
        let ok = unsafe {
            DeviceIoControl(self.h, IOCTL_SCSI_PASS_THROUGH_DIRECT, p, size, p, size, &mut got, std::ptr::null_mut())
        };
        if ok == 0 {
            return Err(Failure { win32: Some(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)), ..Default::default() });
        }
        if r.sptd.scsi_status != 0 {
            return Err(Failure {
                win32: None,
                scsi_status: r.sptd.scsi_status,
                sense_key: r.sense[2] & 15,
                asc: r.sense[12],
                ascq: r.sense[13],
            });
        }
        Ok(())
    }

    fn exec(&mut self, cdb: &[u8], data: Option<&mut [u8]>) -> Result<(), Failure> {
        self.exec_with_timeout(cdb, data, TIMEOUT_S)
    }

    /// The same, with a timeout of its own: a command that some bridges never
    /// answer must not hold the drive list for as long as the general timeout.
    fn exec_with_timeout(
        &mut self,
        cdb: &[u8],
        mut data: Option<&mut [u8]>,
        timeout_s: u32,
    ) -> Result<(), Failure> {
        let first = self.exec_once(cdb, data.as_deref_mut(), timeout_s);
        match &first {
            Err(f) if f.retryable() => self.exec_once(cdb, data.as_deref_mut(), timeout_s),
            _ => first,
        }
    }

    /// Vendor, product and revision. Best effort: a bridge that will not answer leaves
    /// the model strings empty, but the drive is still listed.
    fn inquiry(&mut self) -> Result<(String, String, String), Failure> {
        let mut b = [0u8; 36];
        self.exec(&INQUIRY, Some(&mut b))?;
        Ok(mmc::parse_inquiry(&b))
    }

    /// TEMPORARY: which error the cheap check actually returns.
    pub fn debug_check(&mut self) -> i32 {
        let mut got = 0u32;
        let ok = unsafe {
            DeviceIoControl(self.h, IOCTL_STORAGE_CHECK_VERIFY, std::ptr::null(), 0, std::ptr::null_mut(), 0, &mut got, std::ptr::null_mut())
        };
        if ok != 0 {
            return 0;
        }
        std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
    }

    /// Same medium as the last full report? This ioctl reads no sectors, so poll it as
    /// often as the page likes without the drive spinning up.
    fn media_event(&mut self) -> MediaEvent {
        let mut got = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.h,
                IOCTL_STORAGE_CHECK_VERIFY,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut got,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            return MediaEvent::Unchanged;
        }
        match std::io::Error::last_os_error().raw_os_error().unwrap_or(0) {
            // ERROR_NOT_READY, ERROR_MEDIUM_NOT_PRESENT.
            21 | 55 => MediaEvent::Absent,
            _ => MediaEvent::Changed,
        }
    }

    /// True when a disc is in the drive. An empty tray answers TEST UNIT READY with
    /// "medium not present"; when a drive words it some other way READ CAPACITY is
    /// the last word, because no medium can answer it.
    fn media_present(&mut self) -> bool {
        match self.exec(&TEST_UNIT_READY, None) {
            Ok(()) => true,
            Err(f) if f.medium_absent() => false,
            Err(_) => self.read_capacity().is_ok(),
        }
    }

    /// Last addressable sector plus one, counted the way the disc counts them.
    fn read_capacity(&mut self) -> Result<u32, Failure> {
        let mut b = [0u8; 8];
        self.exec(&mmc::read_capacity(), Some(&mut b))?;
        Ok(mmc::parse_capacity(&b))
    }

    fn current_profile(&mut self) -> Option<u16> {
        let mut b = [0u8; 8];
        self.exec(&GET_CURRENT_PROFILE, Some(&mut b)).ok()?;
        Some(u16::from_be_bytes([b[6], b[7]]))
    }

    fn disc_status(&mut self) -> Option<u8> {
        let mut b = [0u8; 34];
        self.exec(&READ_DISC_INFORMATION, Some(&mut b)).ok()?;
        Some(mmc::disc_status(&b))
    }

    /// The disc's own identification code, out of the structure the drive reads
    /// for itself. It is the one number that says which disc this is, so a
    /// calibration can be looked up by the disc in the drive instead of by hand.
    /// A drive that will not answer leaves the code empty.
    ///
    /// A DVD-R keeps its code in a table of records and a DVD+R in ADIP, and the
    /// kind of disc in the drive says which of the two shapes to expect.
    ///
    /// A DVD+R is asked for ADIP first, which is where the standard keeps it.
    /// A drive that knows no ADIP format of its own says the field is invalid
    /// and hands out the physical format instead, with a copy of the same block
    /// in it, so that is asked next.
    fn media_id(&mut self, media_type: &str) -> Option<String> {
        let dvd_plus = media_type.starts_with("DVD+");
        for &format in mmc::media_id_formats(media_type) {
            let mut b = vec![0u8; DISC_STRUCTURE_BYTES];
            if self
                .exec_with_timeout(
                    &mmc::read_disc_structure(format, DISC_STRUCTURE_BYTES as u16),
                    Some(&mut b[..]),
                    DISC_STRUCTURE_TIMEOUT_S,
                )
                .is_err()
            {
                continue;
            }
            let id = if dvd_plus { mmc::parse_adip_id(&b) } else { mmc::parse_medium_id(&b) };
            if id.is_some() {
                return id;
            }
        }
        None
    }

    /// Everything the rest of the app expects, from as few commands as will answer.
    fn info(&mut self, id: &str) -> OpticalDriveInfo {
        let (vendor, product, revision) = self.inquiry().unwrap_or_default();
        let media_present = self.media_present();
        let mut info = OpticalDriveInfo {
            id: id.to_string(),
            vendor,
            product,
            revision,
            media_present,
            media_type: "None".into(),
            ..Default::default()
        };
        if !media_present {
            return info;
        }
        if let Some(profile) = self.current_profile().and_then(mmc::profile_media_type) {
            info.media_type = profile.to_string();
        }
        // A disc whose status cannot be read counts as written: never offer to burn
        // over something that may already hold a recording.
        info.is_blank = self.disc_status().is_some_and(|s| s == 0);
        // Blank media can report no capacity at all until something is written to it,
        // and the nominal size at least matches the disc in the tray.
        let reported = self.read_capacity().unwrap_or(0);
        info.total_sectors = if reported < 2 { mmc::nominal_sectors(&info.media_type) } else { reported };
        // The disc names itself, so the app can tell which disc is in the drive
        // without the user typing a code. A blank disc carries that code just as
        // a written one does: it is stamped on the disc, so the calibration of
        // the disc about to be burned can be found before anything is written.
        // Not every drive answers: a USB bridge that never returns the structure
        // leaves it empty, and a preset can still be picked by hand.
        info.media_id = self.media_id(&info.media_type).unwrap_or_default();
        //
        // Both sides of the pair matter: a 16x disc in an 8x drive is written at 8x.
        let raw = imapi_write_speeds(id, (info.total_sectors, info.is_blank));
        let (speeds, max) = speeds_to_offer(&raw, &info.media_type);
        info.write_speeds = speeds;
        info.max_write_speed = max;
        info
    }
}

impl TimedDrive for WinDrive {
    fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
        let mut b = vec![0u8; 2048];
        self.exec(&mmc::read12(lba, 1, fua), Some(&mut b)).map_err(|f| f.text(0xA8))?;
        Ok(TimedRead { t_end_ns: self.epoch.elapsed().as_nanos() as f64 })
    }

    fn read_sector(&mut self, lba: u32, buf: &mut [u8; 2048]) -> Result<(), String> {
        self.exec(&mmc::read12(lba, 1, false), Some(&mut buf[..])).map_err(|f| f.text(0xA8))
    }

    fn read_raw_sector(&mut self, lba: u32, buf: &mut [u8; 2352]) -> Result<(), String> {
        self.exec(&mmc::read_cd_raw(lba), Some(&mut buf[..])).map_err(|f| f.text(0xBE))
    }

    fn capacity(&mut self) -> Result<u32, String> {
        self.read_capacity().map_err(|f| f.text(0x25))
    }

    fn supports_fua(&self) -> bool {
        true
    }

    fn set_read_speed(&mut self, kbps: u16) -> Result<(), String> {
        self.exec(&mmc::set_speed(kbps), None).map_err(|f| f.text(0xBB))
    }

    fn host_wait(&mut self, ns: u64) {
        std::thread::sleep(Duration::from_nanos(ns));
    }
}

fn open_handle(id: &str, access: u32) -> Result<WinDrive, i32> {
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    Ok(WinDrive { h, epoch: Instant::now() })
}

pub fn open(id: &str) -> Result<Box<dyn TimedDrive>, String> {
    let d = open_handle(id, DEVICE_ACCESS).map_err(|code| {
        if code == ACCESS_DENIED {
            format!("打不开 {id}：权限不足，需要以管理员身份运行")
        } else {
            format!("打不开 {id}：{}", std::io::Error::from_raw_os_error(code))
        }
    })?;
    Ok(Box::new(d))
}

/// Write speeds Windows offers for this drive and the disc in it, in the units its burn
/// interface uses. IMAPI2 is the only place Windows reports the pair: the SCSI command the
/// other backends use answers with zeroed descriptors on this drive.
///
/// Cached, because the answer only changes when the media does and asking costs a process.
fn imapi_write_speeds(id: &str, media_key: (u32, bool)) -> Vec<u32> {
    static CACHE: Mutex<Option<(String, (u32, bool), Vec<u32>)>> = Mutex::new(None);
    let mut cache = match CACHE.lock() {
        Ok(c) => c,
        Err(e) => e.into_inner(),
    };
    if let Some((cached_id, cached_key, speeds)) = cache.as_ref() {
        if cached_id == id && *cached_key == media_key {
            return speeds.clone();
        }
    }
    let speeds = match drive_letter(id) {
        Some(letter) => query_write_speeds(&format!("{letter}\\")),
        None => Vec::new(),
    };
    *cache = Some((id.to_string(), media_key, speeds.clone()));
    speeds
}

fn query_write_speeds(volume: &str) -> Vec<u32> {
    let script = format!(
        r#"$ProgressPreference = 'SilentlyContinue'
$ErrorActionPreference = 'Stop'
$target = '{volume}'
try {{
  $master = New-Object -ComObject IMAPI2.MsftDiscMaster2
  foreach ($entry in $master) {{
    $recorder = New-Object -ComObject IMAPI2.MsftDiscRecorder2
    $recorder.InitializeDiscRecorder($entry)
    if (@($recorder.VolumePathNames) -notcontains $target) {{ continue }}
    $format = New-Object -ComObject IMAPI2.MsftDiscFormat2Data
    $format.Recorder = $recorder
    if (-not $format.IsCurrentMediaSupported($recorder)) {{ break }}
    $raw = @($format.SupportedWriteSpeedDescriptors | ForEach-Object {{ [int]$_.WriteSpeed }})
    'SPEEDS:' + ($raw -join ',')
    break
  }}
}} catch {{ }}
'SPEEDS:'"#,
        volume = volume.replace('\'', "''"),
    );
    let encoded = {
        use base64::Engine;
        let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded])
        .output();
    let Ok(out) = out else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find_map(|l| l.trim().strip_prefix("SPEEDS:")).unwrap_or("");
    line.split(',')
        .filter_map(|v| v.trim().parse::<u32>().ok())
        .filter(|v| *v > 0)
        .collect()
}

/// Descriptor value to write speed multiple. One DVD speed is 692.5 sectors per second and
/// one CD speed is 75; the figure was confirmed against a timed burn (see the module tests).
fn multiples_per_x(media_type: &str) -> f32 {
    if media_type.to_ascii_uppercase().contains("CD") {
        75.0
    } else {
        692.5
    }
}

/// The speeds to offer, rounded to the multiples the page shows, with the drive's ceiling.
fn speeds_to_offer(raw: &[u32], media_type: &str) -> (Vec<u8>, u8) {
    let per_x = multiples_per_x(media_type);
    let mut speeds: Vec<u8> = raw
        .iter()
        .map(|v| (*v as f32 / per_x).round() as u8)
        .filter(|x| (1..=48).contains(x))
        .collect();
    speeds.sort_unstable();
    speeds.dedup();
    let max = speeds.iter().copied().max().unwrap_or(0);
    (speeds, max)
}

/// The drive letter Windows gave this drive, so a tool that wants one can be handed
/// it. The letter belongs to the drive, so it is there with a blank disc and even
/// with no disc at all.
pub fn drive_letter(id: &str) -> Option<String> {
    let target = device_number(id)?;
    (b'A'..=b'Z').find_map(|c| {
        let root = format!("{}:", c as char);
        (device_number(&format!(r"\\.\{root}"))? == target).then_some(root)
    })
}

/// The shell's own Eject, the one the drive's context menu runs.
pub fn eject(id: &str) -> Result<(), String> {
    let letter = drive_letter(id).ok_or_else(|| "找不到这个光驱的盘符".to_string())?;
    let script = format!("(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{letter}').InvokeVerb('Eject')");
    match Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err("光驱没有弹出，可能有别的程序正在使用这张光盘".into()),
        Err(e) => Err(format!("PowerShell 运行不了：{e}")),
    }
}

/// The identity behind a device name: same drive, same triple, however it was opened.
fn device_number(id: &str) -> Option<StorageDeviceNumber> {
    let d = open_handle(id, 0).ok()?;
    let mut n = StorageDeviceNumber::default();
    let mut got = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            d.h,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            std::ptr::null(),
            0,
            &mut n as *mut StorageDeviceNumber as *mut c_void,
            std::mem::size_of::<StorageDeviceNumber>() as u32,
            &mut got,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(n)
}

/// What the cheap check can say about the medium since the last full report.
enum MediaEvent {
    /// Same medium, still loaded.
    Unchanged,
    /// Nothing loaded, as far as the last report knows.
    Absent,
    /// Something was put in, taken out, or the drive cannot say: ask the drive properly.
    Changed,
}

/// The last report per drive. The page asks every three seconds, and re-reading the
/// medium that often makes the drive hunt audibly for nothing.
static REPORTS: Mutex<Vec<(String, OpticalDriveInfo)>> = Mutex::new(Vec::new());

/// Every optical drive on the machine, with whatever is in it. Device numbers can
/// have gaps and a drive without a disc has no letter at all, so the names are
/// walked rather than the volumes.
pub fn list() -> Vec<OpticalDriveInfo> {
    let mut reports = match REPORTS.lock() {
        Ok(r) => r,
        Err(e) => e.into_inner(),
    };
    let mut drives = Vec::new();
    for i in 0..DEVICE_SCAN {
        let id = format!("{DEVICE_PREFIX}{i}");
        let Ok(mut d) = open_handle(&id, DEVICE_ACCESS) else { continue };
        let slot = reports.iter().position(|(key, _)| *key == id);
        let cached = slot.map(|s| reports[s].1.clone());
        // Only a change is worth a full probe: everything else would just wake the drive.
        let info = match (d.media_event(), cached) {
            (MediaEvent::Unchanged, Some(info)) => info,
            (MediaEvent::Absent, Some(info)) if !info.media_present => info,
            _ => {
                let info = d.info(&id);
                match slot {
                    Some(s) => reports[s].1 = info.clone(),
                    None => reports.push((id.clone(), info.clone())),
                }
                info
            }
        };
        drives.push(info);
    }
    reports.retain(|(key, _)| drives.iter().any(|d| &d.id == key));
    drives
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one test that needs hardware: DISC_HW=1 cargo test -p disc-core. It runs with
    /// an empty tray too, because that report has to be right as well.
    #[test]
    fn real_drive_reports_what_is_in_it() {
        if std::env::var("DISC_HW").is_err() {
            return;
        }
        let drives = list();
        assert!(!drives.is_empty(), "没有找到光驱");
        let d = &drives[0];
        eprintln!("{d:#?}");
        assert!(!d.product.is_empty(), "INQUIRY 没有型号");
        assert!(device_number(&d.id).is_some(), "设备编号读不到");
        let letter = drive_letter(&d.id).expect("盘符查不到");
        eprintln!("drive letter: {letter}");
        assert_eq!(device_number(&format!(r"\\.\{letter}")), device_number(&d.id));
        for d in drives.iter().filter(|d| d.media_present) {
            assert!(d.total_sectors > 1000, "有盘却只有 {} 个扇区", d.total_sectors);
            // A DVD names itself, blank or written; a CD keeps no maker and media
            // code in this structure, so it has no code to read.
            if d.media_type.starts_with("DVD") {
                assert!(!d.media_id.is_empty(), "{} holds a disc whose MID cannot be read", d.product);
            }
        }
        if !d.media_present {
            assert_eq!(d.total_sectors, 0, "空仓不该报容量");
            assert_eq!(d.media_type, "None");
        }
    }
}