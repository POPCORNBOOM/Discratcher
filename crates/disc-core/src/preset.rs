//! Stamper geometry presets and the IPC shapes shared with the front end.

use crate::media::Media;
use crate::spiral::SpiralParams;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscGeometryPreset {
    pub id: String,
    pub drive_model: String,
    pub media_id: String,
    pub stamper_code: String,
    #[serde(default)]
    pub brand: String,
    /// Local only: when a real burn with this geometry last finished.
    #[serde(default)]
    pub burned_at: String,
    /// Left out of a DVD preset, so those read and write exactly as they always have.
    #[serde(default, skip_serializing_if = "Media::is_dvd")]
    pub media: Media,
    /// CD only. A drive reports a read some constant number of frames after the model's
    /// end point, and that constant cannot be told from a slightly different inner radius
    /// by timing (disc-probe, `docs/research/CD_GEOMETRY_INVESTIGATION.md`). Carving a
    /// picture with the wrong value twists it by about 0.21 degrees per frame between the
    /// inner and outer edge. Found once per drive with the twist chart; None until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub along_track_frames: Option<f64>,
    pub total_sectors: u32,
    pub channel_bit_length_nm: f64,
    pub inner_radius_mm: f64,
    pub outer_radius_mm: f64,
    pub pitch_linear: f64,
    pub pitch_quadratic: f64,
    pub pitch_cubic: f64,
    pub base_pitch_um: f64,
    pub rms_residual_deg: f64,
    pub measured_span_mm: (f64, f64),
    pub created_at: String,
}

impl DiscGeometryPreset {
    /// Channel bits to take off a byte's stream position before looking it up on the
    /// spiral: the measured spiral describes where reads ended, that many bits further on.
    pub fn along_track_bits(&self) -> f64 {
        match self.media {
            Media::Cd => self.along_track_frames.unwrap_or(0.0) * crate::ecma130::FRAME_BITS as f64,
            Media::Dvd => 0.0,
        }
    }

    pub fn spiral_params(&self) -> SpiralParams {
        SpiralParams {
            total_sectors: self.total_sectors,
            sector_bits: self.media.sector_bits(),
            bit_mm: self.channel_bit_length_nm * 1e-6,
            ri: self.inner_radius_mm,
            ro: self.outer_radius_mm,
            c1: self.pitch_linear,
            c2: self.pitch_quadratic,
            c3: self.pitch_cubic,
        }
    }
}

/// A CD-R shaped like the one disc-probe measured (24.84 mm, 1.5003 µm), for the tests.
#[cfg(test)]
pub(crate) fn cd_test_preset() -> DiscGeometryPreset {
    let total_sectors = 359_779;
    let bit = crate::ecma130::NOMINAL_BIT_LENGTH_NM * 1e-6;
    let (ri, pitch) = (24.84f64, 1.5003e-3);
    let length = total_sectors as f64 * crate::ecma130::SECTOR_CHANNEL_BITS as f64 * bit;
    DiscGeometryPreset {
        id: "test-cd".into(),
        media: Media::Cd,
        total_sectors,
        channel_bit_length_nm: bit * 1e6,
        inner_radius_mm: ri,
        outer_radius_mm: (ri * ri + length * pitch / std::f64::consts::PI).sqrt(),
        pitch_linear: 0.0,
        pitch_quadratic: 0.0,
        pitch_cubic: 0.0,
        base_pitch_um: pitch * 1e3,
        ..reference_preset()
    }
}

/// A measured disc for the virtual drive and the tests. Users never see it listed.
pub fn reference_preset() -> DiscGeometryPreset {
    DiscGeometryPreset {
        id: "reference-gud1n-cmc-mag-am3-0237-l110".into(),
        drive_model: "HL-DT-ST DVDRAM GUD1N 1.01".into(),
        media_id: "CMC MAG. AM3".into(),
        stamper_code: "CMDR47G-CFMWMO3-0237 L110".into(),
        brand: String::new(),
        burned_at: String::new(),
        media: Media::Dvd,
        along_track_frames: None,
        total_sectors: 2_295_104,
        channel_bit_length_nm: 133.3,
        inner_radius_mm: 24.173325346,
        outer_radius_mm: 57.878686008,
        pitch_linear: -1.857160e-5,
        pitch_quadratic: 1.191945e-4,
        pitch_cubic: -1.373621e-4,
        base_pitch_um: 0.7340606,
        rms_residual_deg: 0.402,
        measured_span_mm: (25.292, 56.914),
        created_at: "2026-11-06T21:53:00+08:00".into(),
    }
}
