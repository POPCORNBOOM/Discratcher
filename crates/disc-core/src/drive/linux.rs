//! Linux backend: SG_IO. Compiled in CI-style checks only; not exercised on hardware.

use super::{mmc, OpticalDriveInfo, TimedDrive, TimedRead};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const SG_IO: libc::c_ulong = 0x2285;
const SG_DXFER_NONE: i32 = -1;
const SG_DXFER_FROM_DEV: i32 = -3;

#[repr(C)]
struct SgIoHdr {
    interface_id: i32,
    dxfer_direction: i32,
    cmd_len: u8,
    mx_sb_len: u8,
    iovec_count: u16,
    dxfer_len: u32,
    dxferp: *mut u8,
    cmdp: *const u8,
    sbp: *mut u8,
    timeout: u32,
    flags: u32,
    pack_id: i32,
    usr_ptr: *mut libc::c_void,
    status: u8,
    masked_status: u8,
    msg_status: u8,
    sb_len_wr: u8,
    host_status: u16,
    driver_status: u16,
    resid: i32,
    duration: u32,
    info: u32,
}

pub struct LinuxDrive {
    file: std::fs::File,
    epoch: Instant,
}

impl LinuxDrive {
    fn exec(&mut self, cdb: &[u8], data: Option<&mut [u8]>) -> Result<(), String> {
        let (dir, ptr, len) = match data {
            Some(d) => (SG_DXFER_FROM_DEV, d.as_mut_ptr(), d.len() as u32),
            None => (SG_DXFER_NONE, std::ptr::null_mut(), 0),
        };
        let mut sense = [0u8; 32];
        let mut h: SgIoHdr = unsafe { std::mem::zeroed() };
        h.interface_id = 'S' as i32;
        h.dxfer_direction = dir;
        h.cmd_len = cdb.len() as u8;
        h.mx_sb_len = sense.len() as u8;
        h.dxfer_len = len;
        h.dxferp = ptr;
        h.cmdp = cdb.as_ptr();
        h.sbp = sense.as_mut_ptr();
        h.timeout = 30_000;
        if unsafe { libc::ioctl(self.file.as_raw_fd(), SG_IO as _, &mut h) } != 0 {
            return Err(format!("SG_IO 失败：{}", std::io::Error::last_os_error()));
        }
        if h.status != 0 {
            return Err(format!("SCSI 状态 {:#04x}，sense {:02X}/{:02X}/{:02X}", h.status, sense[2] & 15, sense[12], sense[13]));
        }
        Ok(())
    }
}

impl TimedDrive for LinuxDrive {
    fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
        let mut b = vec![0u8; 2048];
        self.exec(&mmc::read12(lba, 1, fua), Some(&mut b))?;
        Ok(TimedRead { t_end_ns: self.epoch.elapsed().as_nanos() as f64 })
    }

    fn read_sector(&mut self, lba: u32, buf: &mut [u8; 2048]) -> Result<(), String> {
        self.exec(&mmc::read12(lba, 1, false), Some(&mut buf[..]))
    }

    fn read_raw_sector(&mut self, lba: u32, buf: &mut [u8; 2352]) -> Result<(), String> {
        self.exec(&mmc::read_cd_raw(lba), Some(&mut buf[..]))
    }

    fn capacity(&mut self) -> Result<u32, String> {
        let mut b = [0u8; 8];
        self.exec(&mmc::read_capacity(), Some(&mut b))?;
        Ok(mmc::parse_capacity(&b))
    }

    fn supports_fua(&self) -> bool {
        true
    }

    fn set_read_speed(&mut self, kbps: u16) -> Result<(), String> {
        self.exec(&mmc::set_speed(kbps), None)
    }

    fn host_wait(&mut self, ns: u64) {
        std::thread::sleep(Duration::from_nanos(ns));
    }
}

pub fn open(id: &str) -> Result<Box<dyn TimedDrive>, String> {
    let file = std::fs::File::open(id).map_err(|e| format!("打不开 {id}：{e}"))?;
    Ok(Box::new(LinuxDrive { file, epoch: Instant::now() }))
}

pub fn list() -> Vec<OpticalDriveInfo> {
    let Ok(dir) = std::fs::read_dir("/sys/class/block") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("sr") {
                return None;
            }
            let read = |f: &str| {
                std::fs::read_to_string(e.path().join("device").join(f)).unwrap_or_default().trim().to_string()
            };
            Some(OpticalDriveInfo {
                id: format!("/dev/{name}"),
                vendor: read("vendor"),
                product: read("model"),
                revision: read("rev"),
                media_type: "None".into(),
                ..Default::default()
            })
        })
        .collect()
}
