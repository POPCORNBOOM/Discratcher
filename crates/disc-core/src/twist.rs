//! Twist chart: a picture that finds a CD drive's along-track constant by eye.
//!
//! Timing gives a CD's spiral up to one unknown: how many frames after the model's end
//! point the drive really reports a read (see `DiscGeometryPreset::along_track_frames`).
//! Being wrong by `d` frames turns every point by `d * L / r`, with `L` one frame of
//! track, so a radial line comes out bent. disc-probe settles the number by burning a
//! grid and scanning it. This chart needs no scanner: each of its lines is drawn already
//! bent for one candidate value, so on the burned disc exactly the line whose value is
//! right comes out straight, and the number printed at its end is the answer.

use crate::ecma130::FRAME_BITS;
use crate::preset::DiscGeometryPreset;
use std::f64::consts::TAU;
use std::path::Path;

pub const LINES: usize = 24;
pub const FIRST_FRAMES: f64 = -30.0;
/// Between neighbouring lines. One step bends a line by about 3 degrees across the disc.
pub const STEP_FRAMES: f64 = 15.0;
const LINE_MM: f64 = 0.5;
/// Size of one dot of the digits.
const DOT_MM: f64 = 0.5;
/// Room kept outside the lines for the numbers.
const LABEL_BAND_MM: f64 = 4.5;

/// 3 x 5 digits and a minus sign, one row per entry, top first.
const GLYPHS: [(char, [u8; 5]); 11] = [
    ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
    ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
    ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
    ('3', [0b111, 0b001, 0b111, 0b001, 0b111]),
    ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
    ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
    ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
    ('7', [0b111, 0b001, 0b010, 0b010, 0b010]),
    ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
    ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
    ('-', [0b000, 0b000, 0b111, 0b000, 0b000]),
];

/// The value line `i` is drawn for.
pub fn line_frames(i: usize) -> f64 {
    FIRST_FRAMES + STEP_FRAMES * i as f64
}

/// Is the dot at (`along`, `up`) millimetres part of `text`? `along` runs in reading
/// direction from the text's middle, `up` from its baseline.
fn text_dot(text: &str, along: f64, up: f64) -> bool {
    let cells = text.len() as f64 * 4.0 - 1.0;
    let (col, row) = ((along / DOT_MM + cells / 2.0).floor(), 4.0 - (up / DOT_MM).floor());
    if !(0.0..cells).contains(&col) || !(0.0..5.0).contains(&row) {
        return false;
    }
    let (index, x) = (col as usize / 4, col as usize % 4);
    let Some(glyph) = text.chars().nth(index).and_then(|c| GLYPHS.iter().find(|g| g.0 == c)) else { return false };
    x < 3 && glyph.1[row as usize] >> (2 - x) & 1 == 1
}

struct Chart {
    ro: f64,
    inner: f64,
    outer: f64,
    /// One frame of track, mm.
    frame_mm: f64,
    current: f64,
}

impl Chart {
    fn new(p: &DiscGeometryPreset) -> Self {
        let outer = p.outer_radius_mm - LABEL_BAND_MM;
        Self {
            ro: p.outer_radius_mm,
            inner: (p.inner_radius_mm + 0.8).min(outer),
            outer,
            frame_mm: FRAME_BITS as f64 * p.channel_bit_length_nm * 1e-6,
            current: p.along_track_frames.unwrap_or(0.0),
        }
    }

    /// Angle at which line `i` is drawn at radius `r`. Every line meets its number at the
    /// outer end; further in it leads or trails by the turn its candidate value implies.
    fn line_angle(&self, i: usize, r: f64) -> f64 {
        i as f64 * TAU / LINES as f64 + (line_frames(i) - self.current) * self.frame_mm * (1.0 / r - 1.0 / self.outer)
    }

    fn dark(&self, x_mm: f64, y_mm: f64) -> bool {
        let r = x_mm.hypot(y_mm);
        let theta = y_mm.atan2(x_mm);
        let arc = |angle: f64| (theta - angle + TAU / 2.0).rem_euclid(TAU) - TAU / 2.0;
        if (self.inner..=self.outer).contains(&r) {
            return (0..LINES).any(|i| (arc(self.line_angle(i, r)) * r).abs() < LINE_MM / 2.0);
        }
        if r > self.outer && r < self.ro {
            // The nearest line's number, read with its top towards the rim.
            let i = (theta / TAU * LINES as f64).round().rem_euclid(LINES as f64) as usize;
            let text = format!("{}", line_frames(i) as i64);
            return text_dot(&text, arc(i as f64 * TAU / LINES as f64) * r, r - self.outer - 1.0);
        }
        false
    }
}

/// The chart as a square of grey pixels spanning the disc's diameter, the way a layer
/// that fills the disc is sampled. It is drawn against the value the preset holds now,
/// so the numbers on it are always absolute.
pub fn chart(preset: &DiscGeometryPreset, size: u32) -> Vec<u8> {
    let c = Chart::new(preset);
    let mm = |i: u32| ((i as f64 + 0.5) / size as f64 - 0.5) * 2.0 * c.ro;
    let mut out = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            out.push(if c.dark(mm(x), mm(y)) { 0 } else { 255 });
        }
    }
    out
}

pub fn write_chart(preset: &DiscGeometryPreset, path: &Path) -> Result<(), String> {
    const SIZE: u32 = 4096;
    let img = image::GrayImage::from_raw(SIZE, SIZE, chart(preset, SIZE)).ok_or("chart buffer size")?;
    img.save(path).map_err(|e| format!("无法写入测试图：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spiral::Spiral;

    fn cd() -> DiscGeometryPreset {
        crate::preset::cd_test_preset()
    }

    #[test]
    fn digits_are_drawn() {
        assert!(text_dot("1", 0.0, 0.1), "the stem of a 1");
        assert!(!text_dot("1", -0.6, 1.2));
        assert!(text_dot("-30", -2.5, 1.2), "the minus sign sits mid-height");
        assert!(!text_dot("7", 5.0, 1.0));
    }

    /// The property the chart exists for, checked through the spiral itself: follow each
    /// drawn line onto a disc whose true constant is 105 frames. Only line "105" may come
    /// out radial, and its neighbours must bend opposite ways.
    #[test]
    fn only_the_true_line_comes_out_straight() {
        let preset = cd();
        let chart = Chart::new(&preset);
        let spiral = Spiral::new(preset.spiral_params()).unwrap();
        let truth_bits = 105.0 * FRAME_BITS as f64;
        // Real angle of the point the carver puts at design radius r, design angle a:
        // the byte there sits at stream position n, which the true disc holds at n - truth.
        let real_angle = |r: f64, a: f64| {
            let (n, th) = spiral.at_radius(r);
            let n = n + (a - th).rem_euclid(TAU) * r / spiral.params.bit_mm;
            spiral.locate(n - truth_bits).1 - spiral.locate(n).1 + a
        };
        let bend = |i: usize| {
            let (a, b) = (chart.inner + 1.0, chart.outer);
            (real_angle(a, chart.line_angle(i, a)) - real_angle(b, chart.line_angle(i, b))).to_degrees()
        };
        let straight = (0..LINES).find(|&i| line_frames(i) == 105.0).unwrap();
        assert!(bend(straight).abs() < 0.05, "{}", bend(straight));
        assert!(bend(straight - 1) * bend(straight + 1) < 0.0);
        assert!(bend(straight + 1).abs() > 2.0 && bend(straight + 1).abs() < 4.5, "{}", bend(straight + 1));
        // 206 frames, disc-probe's GH70N figure, is the 44 degree twist its notes report.
        let twist = (206.0 * chart.frame_mm * (1.0 / 25.0 - 1.0 / 58.0)).to_degrees();
        assert!((twist - 44.0).abs() < 1.0, "{twist}");
    }

    #[test]
    fn the_picture_has_lines_and_numbers() {
        let preset = cd();
        let px = chart(&preset, 512);
        let dark = px.iter().filter(|&&v| v == 0).count() as f64 / px.len() as f64;
        assert!(dark > 0.01 && dark < 0.08, "{dark}");
        assert_eq!(px[256 * 512 + 256], 255, "the hub stays clear");
    }
}
