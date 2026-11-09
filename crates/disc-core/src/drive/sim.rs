//! A simulated drive on a virtual clock. It proves program logic only: real drives add
//! cache timing, whole-turn rereads and speed changes that no simulator settles.

use super::{OpticalDriveInfo, TimedDrive, TimedRead, SIM_DRIVE_ID};
use crate::calib::solver::completion_bit;
use crate::ecma267::DEFAULT_TOTAL_SECTORS;
use crate::preset::reference_preset;
use crate::rng::SplitMix64;
use crate::spiral::{Spiral, SpiralParams};
use std::f64::consts::TAU;

pub struct SimDrive {
    spiral: Spiral,
    pub period_ns: f64,
    pub jitter_deg: f64,
    pub retry_chance: f64,
    pub fua: bool,
    now_ns: f64,
    head_lba: u32,
    cached_block: Option<u32>,
    rng: SplitMix64,
    pace_us: u64,
}

pub fn drive_info() -> OpticalDriveInfo {
    OpticalDriveInfo {
        id: SIM_DRIVE_ID.into(),
        vendor: "Simulated".into(),
        product: "虚拟光驱".into(),
        media_present: true,
        media_type: "DVD-R".into(),
        media_id: "SIM.DISC".into(),
        total_sectors: DEFAULT_TOTAL_SECTORS,
        simulated: true,
        write_speeds: vec![2, 4, 8, 16],
        max_write_speed: 16,
        ..Default::default()
    }
}

impl SimDrive {
    pub fn new(params: SpiralParams, seed: u64) -> Self {
        Self {
            spiral: Spiral::new(params).expect("sim geometry"),
            period_ns: 25.474e6,
            jitter_deg: 0.6,
            retry_chance: 0.01,
            fua: true,
            now_ns: 1e9,
            head_lba: 0,
            cached_block: None,
            rng: SplitMix64(seed),
            pace_us: 0,
        }
    }

    /// Disc that matches the shipped preset, so a demo run lands somewhere recognisable.
    pub fn demo(paced: bool) -> Self {
        let mut d = Self::new(reference_preset().spiral_params(), 0x5EED);
        d.pace_us = if paced { 25 } else { 0 };
        d
    }

}

impl TimedDrive for SimDrive {
    fn read_timed(&mut self, lba: u32, fua: bool) -> Result<TimedRead, String> {
        let block = lba / 16;
        let hit = !(fua && self.fua) && self.cached_block == Some(block);
        if hit {
            self.now_ns += 1.1e6;
            return Ok(TimedRead { t_end_ns: self.now_ns });
        }
        // Seek cost grows with distance, then wait for the block's end to reach the head.
        let dist = lba.abs_diff(self.head_lba) as f64 / DEFAULT_TOTAL_SECTORS as f64;
        let earliest = self.now_ns + 3e6 + 90e6 * dist.sqrt();
        let target = self.spiral.theta_at_bit(completion_bit(lba));
        let phase_ns = (target / TAU).rem_euclid(1.0) * self.period_ns;
        let wait = (phase_ns - earliest).rem_euclid(self.period_ns);
        let mut end = earliest + wait;
        if self.rng.unit() < self.retry_chance {
            end += self.period_ns * 2.0 + self.rng.unit() * self.period_ns;
        }
        end += self.rng.gauss() * self.jitter_deg / 360.0 * self.period_ns;
        self.now_ns = end.max(self.now_ns);
        self.head_lba = lba;
        self.cached_block = Some(block);
        if self.pace_us > 0 {
            std::thread::sleep(std::time::Duration::from_micros(self.pace_us));
        }
        Ok(TimedRead { t_end_ns: end })
    }

    fn capacity(&mut self) -> Result<u32, String> {
        Ok(self.spiral.params.total_sectors)
    }

    /// The simulated disc is a freshly burned calibration disc.
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; 2048]) -> Result<(), String> {
        let total = self.spiral.params.total_sectors;
        if lba >= total {
            return Err(format!("LBA {lba} 超出光盘"));
        }
        crate::iso9660::calibration_sector(lba, total, buf);
        Ok(())
    }

    fn supports_fua(&self) -> bool {
        self.fua
    }

    fn set_read_speed(&mut self, _kbps: u16) -> Result<(), String> {
        Ok(())
    }

    fn host_wait(&mut self, ns: u64) {
        self.now_ns += ns as f64;
    }
}
