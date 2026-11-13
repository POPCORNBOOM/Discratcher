//! What differs between the disc families: how long a sector is on the track, where a
//! timed read ends, and where the calibration ladder starts looking.

use crate::{ecma130, ecma267};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Media {
    #[default]
    Dvd,
    Cd,
}

/// Where the ladder starts and what it may assume before anything is measured.
pub struct LadderShape {
    pub nominal_ri_mm: f64,
    pub nominal_pitch_mm: f64,
    pub sigma_ri_mm: f64,
    pub sigma_pitch_mm: f64,
    pub read_speed_kbps: u16,
    /// Spindle speeds the settling search covers.
    pub rpm: (f64, f64),
    /// Longest wait drawn before a period is known: one slowest-plausible turn.
    pub slowest_turn_ns: f64,
    pub min_spacing_sectors: u32,
    /// A disc with fewer sectors than this was not burned with a calibration image.
    pub min_sectors: u32,
    /// First rungs, in turns from the reference.
    pub near_turns: &'static [f64],
    /// Closest a near rung may sit to the reference, in sectors.
    pub near_min_sectors: u32,
}

impl Media {
    /// The family a loaded disc belongs to, or None for one Discratcher cannot carve.
    /// A closed CD-R answers as a CD-ROM (seen on the GUD1N straight after a burn), so
    /// that name has to pass or a calibration disc could never be measured. Nothing is
    /// burned on one: it is never blank.
    pub fn of_type(media_type: &str) -> Option<Self> {
        match media_type.trim() {
            "DVD-R" | "DVD+R" | "DVD-RW" | "DVD+RW" => Some(Self::Dvd),
            "CD-R" | "CD-RW" | "CD-ROM" => Some(Self::Cd),
            _ => None,
        }
    }

    pub fn is_dvd(&self) -> bool {
        *self == Self::Dvd
    }

    pub const fn sector_bits(self) -> u32 {
        match self {
            Self::Dvd => ecma267::SECTOR_CHANNEL_BITS,
            Self::Cd => ecma130::SECTOR_CHANNEL_BITS,
        }
    }

    /// Assumed channel bit length. Timing only measures radius and pitch in units of it.
    pub fn bit_mm(self) -> f64 {
        match self {
            Self::Dvd => ecma267::NOMINAL_BIT_LENGTH_NM * 1e-6,
            Self::Cd => ecma130::NOMINAL_BIT_LENGTH_NM * 1e-6,
        }
    }

    /// Sectors the drive decodes together; reads and rungs are aligned to it.
    pub const fn block(self) -> u32 {
        match self {
            Self::Dvd => 16,
            Self::Cd => 1,
        }
    }

    pub fn align(self, lba: u32) -> u32 {
        lba / self.block() * self.block()
    }

    /// Channel bit where a timed read of `lba` is taken to end.
    ///
    /// DVD: the end of the ECC block, because the drive must decode the whole block.
    /// CD: the end of the sector plus the 108 frames its last bytes are delayed by CIRC.
    /// Source: `read_end_bits` in disc-probe `cd.py`. A drive may really finish a constant
    /// number of frames later; timing cannot see that (see `DiscGeometryPreset`).
    pub fn completion_bit(self, lba: u32) -> f64 {
        match self {
            Self::Dvd => ((lba / 16 + 1) * 16) as f64 * ecma267::SECTOR_CHANNEL_BITS as f64,
            Self::Cd => {
                ((lba as f64 + 1.0) * ecma130::FRAMES_PER_SECTOR as f64 + ecma130::MAX_DELAY_FRAMES as f64)
                    * ecma130::FRAME_BITS as f64
            }
        }
    }

    pub fn ladder(self) -> LadderShape {
        match self {
            Self::Dvd => LadderShape {
                nominal_ri_mm: 24.1,
                nominal_pitch_mm: 0.74e-3,
                sigma_ri_mm: 0.6,
                sigma_pitch_mm: 0.02e-3,
                // 4x, the one speed the method has been validated at on hardware.
                read_speed_kbps: 5540,
                rpm: (1500.0, 12_000.0),
                slowest_turn_ns: 60e9 / 1200.0,
                min_spacing_sectors: 2000,
                min_sectors: 500_000,
                near_turns: &[1.5, -1.5, 3.0, -3.0, 6.0, -6.0, 12.0, -12.0, 24.0, -24.0],
                near_min_sectors: 32,
            },
            // CD-R blanks differ far more than DVDs do (1.2 to 1.4 m/s, 1.5 to 1.7 µm), so
            // the priors are wide and the first rung sits half a turn out. 8x is where
            // disc-probe's CD runs were clean; at 16x its LG drive read the outer edge late.
            Self::Cd => LadderShape {
                nominal_ri_mm: 24.9,
                nominal_pitch_mm: 1.5e-3,
                sigma_ri_mm: 2.0,
                sigma_pitch_mm: 0.2e-3,
                read_speed_kbps: 1411,
                rpm: (400.0, 6000.0),
                slowest_turn_ns: 60e9 / 600.0,
                min_spacing_sectors: 600,
                min_sectors: 50_000,
                near_turns: &[0.5, -0.5, 1.5, -1.5, 3.0, -3.0, 6.0, -6.0, 12.0, -12.0, 24.0, -24.0],
                near_min_sectors: 4,
            },
        }
    }

    /// (what, low, high, unit) a measured geometry must fall in, for pitch and inner
    /// radius. DVD: ECMA-267 pitch 0.74 ± 0.03 µm and a data zone starting near 24 mm.
    /// CD: ECMA-130 pitch 1.6 ± 0.1 µm, widened because the bit length is assumed.
    pub fn spec(self) -> [(f64, f64); 2] {
        match self {
            Self::Dvd => [(0.71, 0.77), (23.6, 24.6)],
            Self::Cd => [(1.3, 1.8), (22.5, 27.0)],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dvd => "DVD",
            Self::Cd => "CD",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        assert_eq!(Media::of_type(" DVD-R "), Some(Media::Dvd));
        assert_eq!(Media::of_type("CD-R"), Some(Media::Cd));
        assert_eq!(Media::of_type("CD-ROM"), Some(Media::Cd));
        for m in ["BD-R", "DVD+R DL", "DVD-RAM", "DVD-ROM", "None", ""] {
            assert_eq!(Media::of_type(m), None, "{m}");
        }
    }

    #[test]
    fn completion_points() {
        assert_eq!(Media::Dvd.completion_bit(17), 32.0 * 38_688.0);
        assert_eq!(Media::Cd.completion_bit(0), 206.0 * 588.0);
        assert_eq!(Media::Cd.sector_bits(), 98 * 588);
        assert_eq!(serde_json::to_string(&Media::Cd).unwrap(), "\"cd\"");
    }
}
