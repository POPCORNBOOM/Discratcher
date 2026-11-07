//! Drive access. Measurement code talks to `TimedDrive`, so the ladder runs
//! unchanged on real hardware and on the simulator.

pub mod burner;
pub mod sim;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
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

/// Shared MMC command blocks, so the three transports stay byte-for-byte alike.
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
    }
}
