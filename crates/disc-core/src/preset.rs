//! Stamper geometry presets and the IPC shapes shared with the front end.

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
    pub fn spiral_params(&self) -> SpiralParams {
        SpiralParams {
            total_sectors: self.total_sectors,
            bit_mm: self.channel_bit_length_nm * 1e-6,
            ri: self.inner_radius_mm,
            ro: self.outer_radius_mm,
            c1: self.pitch_linear,
            c2: self.pitch_quadratic,
            c3: self.pitch_cubic,
        }
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
