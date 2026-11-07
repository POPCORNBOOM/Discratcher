//! Windows backend: IOCTL_SCSI_PASS_THROUGH_DIRECT. Compiled in checks only; not
//! exercised on hardware.

use super::{mmc, OpticalDriveInfo, TimedDrive, TimedRead};
use std::ffi::c_void;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;

const IOCTL_SCSI_PASS_THROUGH_DIRECT: u32 = 0x0004_D014;
const SCSI_IOCTL_DATA_IN: u8 = 1;
const SCSI_IOCTL_DATA_UNSPECIFIED: u8 = 2;

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
    fn exec(&mut self, cdb: &[u8], data: Option<&mut [u8]>) -> Result<(), String> {
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
                timeout_value: 30,
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
            return Err(format!("DeviceIoControl 失败：{}", std::io::Error::last_os_error()));
        }
        if r.sptd.scsi_status != 0 {
            return Err(format!("SCSI 状态 {:#04x}，sense {:02X}/{:02X}/{:02X}", r.sptd.scsi_status, r.sense[2] & 15, r.sense[12], r.sense[13]));
        }
        Ok(())
    }
}

impl TimedDrive for WinDrive {
    fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
        let mut b = vec![0u8; 2048];
        self.exec(&mmc::read12(lba, 1, fua), Some(&mut b))?;
        Ok(TimedRead { t_end_ns: self.epoch.elapsed().as_nanos() as f64 })
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
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(format!("打不开 {id}，需要管理员权限：{}", std::io::Error::last_os_error()));
    }
    Ok(Box::new(WinDrive { h, epoch: Instant::now() }))
}

pub fn list() -> Vec<OpticalDriveInfo> {
    (0..8)
        .filter_map(|i| {
            let id = format!(r"\\.\CdRom{i}");
            open(&id).ok().map(|_| OpticalDriveInfo { id, media_type: "None".into(), ..Default::default() })
        })
        .collect()
}
