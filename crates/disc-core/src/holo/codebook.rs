//! A library of sync-frame byte runs, each choice free and each putting its diffracted
//! light into one narrow band of spatial frequency.
//!
//! An entry is 91 bytes: a full first-half frame. A second-half frame takes only the
//! first 81, which is why the band is judged on those 81. All bytes are 88 or more and
//! no position lets the drive swap a code word (see `efm`), so the marks are known.
//!
//! The usable band is set by the run lengths: on the choice-free EFM+ graph (`bytes >= 88`,
//! zero drive swap points), Karp's cycle bounds are strictly 2.0 to 5.0 transitions per
//! byte (0.469..1.172 per µm at `bit_um = 0.1333`, with harmonics/split lines reaching
//! 1.25 per µm). Simulated annealing (`build`) covers 0.42..1.12 per µm for the 12-entry
//! calibration swatch; `designer_extend` appends zero-prefix 1–3 byte periodic choice-free
//! cycles whose energy sits almost entirely in discrete lines (purity up to 0.93, DSV <= 7,
//! matching 81B and 91B spectra) with near-zero broadband floor, giving the hologram
//! designer dark off-band options (e.g. `0x83` dark for `g_t >= 0.35`, `0xA7` dark for
//! `|g_t| <= 0.48`) and high-frequency bright lines (`1.17..1.25` per µm) without altering
//! the 12-entry swatch codebook.

use super::efm;
use crate::rng::SplitMix64;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

pub const ENTRY_BYTES: usize = 91;
pub const CORE_BYTES: usize = 81;
pub const F_LOW: f64 = 0.42;
pub const F_HIGH: f64 = 1.12;
/// Half width of the band an entry aims its light into, per µm. About ±0.6° of view.
pub const HALF_BAND: f64 = 0.03;
/// Bounds of the band all energy is compared against.
const EVAL_LO: f64 = 0.15;
const EVAL_HI: f64 = 3.5;
const EVAL_DF: f64 = 0.01;
/// Above this running DSV the run gets penalised: the drive cannot steer it back
/// inside a choice-free frame, and a large one may upset tracking.
pub const DSV_SOFT: i32 = 60;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub bytes: Vec<u8>,
    /// Centre of the band, per µm.
    pub f0: f64,
    /// Share of the 0.15..3.5 per µm energy that lands inside the band.
    pub purity: f64,
    pub dsv_peak: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Codebook {
    pub bit_um: f64,
    pub entries: Vec<Entry>,
}

/// Band powers on the evaluation grid for a run's marks.
fn band_power(trans: &[u32], bit_um: f64) -> Vec<f64> {
    let n = ((EVAL_HI - EVAL_LO) / EVAL_DF) as usize + 1;
    let mut cur: Vec<(f64, f64)> = trans
        .iter()
        .map(|&x| {
            let a = -std::f64::consts::TAU * EVAL_LO * x as f64 * bit_um;
            (a.cos(), a.sin())
        })
        .collect();
    let step: Vec<(f64, f64)> = trans
        .iter()
        .map(|&x| {
            let a = -std::f64::consts::TAU * EVAL_DF * x as f64 * bit_um;
            (a.cos(), a.sin())
        })
        .collect();
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let f = EVAL_LO + k as f64 * EVAL_DF;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, c) in cur.iter_mut().enumerate() {
            if i % 2 == 0 {
                re += c.0;
                im += c.1;
            } else {
                re -= c.0;
                im -= c.1;
            }
            let s = step[i];
            *c = (c.0 * s.0 - c.1 * s.1, c.0 * s.1 + c.1 * s.0);
        }
        out.push((re * re + im * im) / (f * f));
    }
    out
}

/// Is the whole run choice free from a SYNC code on?
pub fn choice_free(bytes: &[u8]) -> bool {
    efm::encode_after_sync(bytes).choices.is_empty()
}

struct Scored {
    cost: f64,
    purity: f64,
    dsv_peak: i32,
}

fn score(bytes: &[u8], f0: f64, bit_um: f64) -> Option<Scored> {
    let e = efm::encode_after_sync(bytes);
    if !e.choices.is_empty() {
        return None;
    }
    let core = &e.codes[..CORE_BYTES];
    let p = band_power(&efm::transitions(core), bit_um);
    let total: f64 = p.iter().sum();
    let inside: f64 = p
        .iter()
        .enumerate()
        .filter(|(k, _)| ((EVAL_LO + *k as f64 * EVAL_DF) - f0).abs() <= HALF_BAND)
        .map(|(_, v)| v)
        .sum();
    let purity = inside / total.max(1e-12);
    let (_, peak) = efm::dsv(&efm::nrzi(&e.codes));
    let cost = -purity + 0.004 * (peak - DSV_SOFT).max(0) as f64;
    Some(Scored { cost, purity, dsv_peak: peak })
}

/// Random choice-free start, built byte by byte.
fn random_start(rng: &mut SplitMix64) -> Vec<u8> {
    loop {
        let mut v: Vec<u8> = Vec::with_capacity(ENTRY_BYTES);
        let mut stuck = false;
        while v.len() < ENTRY_BYTES {
            let mut placed = false;
            for _ in 0..64 {
                let b = 88 + (rng.next_u64() % 168) as u8;
                v.push(b);
                if choice_free(&v) {
                    placed = true;
                    break;
                }
                v.pop();
            }
            if !placed {
                stuck = true;
                break;
            }
        }
        if !stuck {
            return v;
        }
    }
}

/// Simulated annealing towards one band.
pub fn search(f0: f64, bit_um: f64, iters: usize, seed: u64) -> Entry {
    let mut rng = SplitMix64(seed);
    let mut cur = random_start(&mut rng);
    let mut cs = score(&cur, f0, bit_um).expect("start is choice free");
    let (mut best, mut bs) = (cur.clone(), score(&cur, f0, bit_um).unwrap());
    for it in 0..iters {
        let temp = 0.05 * (1.0 - it as f64 / iters as f64) + 1e-4;
        let mut cand = cur.clone();
        let flips = 1 + (rng.next_u64() % 3) as usize;
        for _ in 0..flips {
            let j = (rng.next_u64() % ENTRY_BYTES as u64) as usize;
            cand[j] = 88 + (rng.next_u64() % 168) as u8;
        }
        let Some(s) = score(&cand, f0, bit_um) else { continue };
        if s.cost < cs.cost || rng.unit() < ((cs.cost - s.cost) / temp).exp() {
            cur = cand;
            cs = s;
            if cs.cost < bs.cost {
                best = cur.clone();
                bs = Scored { cost: cs.cost, purity: cs.purity, dsv_peak: cs.dsv_peak };
            }
        }
    }
    Entry { bytes: best, f0, purity: bs.purity, dsv_peak: bs.dsv_peak }
}

/// Band centres spread evenly in log frequency, which is close to even in view angle.
pub fn band_centres(n: usize) -> Vec<f64> {
    (0..n).map(|k| F_LOW * (F_HIGH / F_LOW).powf(k as f64 / (n - 1).max(1) as f64)).collect()
}

pub fn build(n: usize, bit_um: f64, iters: usize, seed: u64) -> Codebook {
    let entries = band_centres(n)
        .into_par_iter()
        .enumerate()
        .map(|(k, f0)| search(f0, bit_um, iters, seed ^ (k as u64).wrapping_mul(0x9E37_79B9)))
        .collect();
    Codebook { bit_um, entries }
}

/// Zero-prefix 1–3 byte periodic choice-free EFM+ cycles spanning the full reachable
/// spatial-frequency band (`0.469..1.250` per µm at `bit_um = 0.1333`), ordered from
/// lowest to highest fundamental frequency.
///
/// Every motif starts directly in SYNC State 1 with `prev_trailing = 0`, returns to
/// `(State 1, trailing_zeros = 0)` at the end of its cycle, uses only bytes `>= 0x58`,
/// offers zero drive swap choices on both 81-byte and 91-byte sync frames, and stays well
/// below `DSV_SOFT` (`dsv_peak <= 7`). Because energy sits in discrete spectral lines
/// rather than broadband noise:
/// - `[0x83]` (`0.469 /µm`, 8T+8T) is dark (`Y <= 0.03`) for `g_t >= 0.35`,
/// - `[0xA7]` (`1.172 /µm`) is dark (`Y <= 0.02`) for `|g_t| <= 0.48` and bright at high `g_t`,
/// - `[0x62, 0x9C, 0x70]` and `[0x65, 0xA5, 0xBC]` (`1.22..1.25 /µm`) rescue far-right views,
/// - intermediate 1–3B cycles cover `0.55..1.09 /µm` with high peak-to-floor contrast.
pub const PERIODIC_MOTIFS: &[&[u8]] = &[
    &[0x83],             // 0.469 /um (2 ones/B)  -- dark-low for g_t >= 0.35
    &[0xC0, 0xCD, 0x6B], // 0.547 /um (7 ones/3B)
    &[0xB2, 0xFF],       // 0.586 /um (5 ones/2B)
    &[0xFF, 0x90, 0x73], // 0.625 /um (8 ones/3B)
    &[0x6A],             // 0.703 /um (3 ones/B)
    &[0x58, 0x69, 0xB9], // 0.781 /um (10 ones/3B)
    &[0x81, 0xFE],       // 0.820 /um (7 ones/2B)
    &[0x58, 0x64, 0x67], // 0.859 /um (11 ones/3B)
    &[0x6F],             // 0.938 /um (4 ones/B)
    &[0xF2, 0xE4, 0x5B], // 1.016 /um (13 ones/3B)
    &[0x9E, 0xEE],       // 1.055 /um (9 ones/2B)
    &[0xE2, 0xA5, 0xF8], // 1.094 /um (14 ones/3B)
    &[0xA7],             // 1.172 /um (5 ones/B)  -- dark-high for |g_t| <= 0.48 & bright v5..v6
    &[0x62, 0x9C, 0x70], // 1.220 /um (15 ones/3B) -- bright v6..v7
    &[0x65, 0xA5, 0xBC], // 1.250 /um (14 ones/3B) -- bright v7 & dark-high
];

/// Build the verified 91-byte periodic choice-free entries for `bit_um`.
pub fn periodic_entries(bit_um: f64) -> Vec<Entry> {
    PERIODIC_MOTIFS
        .iter()
        .map(|motif| {
            let bytes: Vec<u8> = (0..ENTRY_BYTES).map(|i| motif[i % motif.len()]).collect();
            let e = efm::encode_after_sync(&bytes);
            debug_assert!(e.choices.is_empty(), "periodic motif {motif:02X?} must be choice free");
            let (_, dsv_peak) = efm::dsv(&efm::nrzi(&e.codes));
            let p = band_power(&efm::transitions(&e.codes[..CORE_BYTES]), bit_um);
            let total: f64 = p.iter().sum();
            let (best_k, _) = p
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap_or((0, &0.0));
            let f0 = EVAL_LO + best_k as f64 * EVAL_DF;
            let inside: f64 = p
                .iter()
                .enumerate()
                .filter(|(k, _)| ((EVAL_LO + *k as f64 * EVAL_DF) - f0).abs() <= HALF_BAND)
                .map(|(_, v)| v)
                .sum();
            let purity = inside / total.max(1e-12);
            Entry { bytes, f0, purity, dsv_peak }
        })
        .collect()
}

/// Return a designer codebook containing all entries of `base` (unchanged at `0..base.entries.len()`)
/// followed by the high-purity periodic choice-free entries from `periodic_entries`.
pub fn designer_extend(base: &Codebook) -> Codebook {
    let mut entries = base.entries.clone();
    entries.extend(periodic_entries(base.bit_um));
    Codebook { bit_um: base.bit_um, entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_a_clean_band() {
        let e = search(0.83, 0.1333, 4000, 7);
        assert_eq!(e.bytes.len(), ENTRY_BYTES);
        assert!(e.bytes.iter().all(|&b| b >= 88));
        assert!(choice_free(&e.bytes));
        assert!(e.purity > 0.25, "purity {}", e.purity);
    }

    #[test]
    fn whole_band_is_reachable() {
        let cb = build(6, 0.1333, 20000, 1);
        for e in &cb.entries {
            assert!(e.purity > 0.25, "f0 {} purity {}", e.f0, e.purity);
            assert!(e.dsv_peak < 200, "f0 {} dsv {}", e.f0, e.dsv_peak);
        }
    }

    #[test]
    fn periodic_entries_satisfy_all_efm_and_dsv_constraints() {
        let entries = periodic_entries(0.1333);
        assert_eq!(entries.len(), PERIODIC_MOTIFS.len());
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(e.bytes.len(), ENTRY_BYTES, "entry {i} length");
            assert!(e.bytes.iter().all(|&b| b >= 0x58), "entry {i} byte below 0x58");
            // Verify both 91-byte (first-half) and 81-byte (second-half) sync frames by encoding.
            let enc91 = efm::encode_after_sync(&e.bytes);
            assert!(enc91.choices.is_empty(), "entry {i} has 91B choices: {:?}", enc91.choices);
            assert!(efm::rll_ok(&enc91.codes), "entry {i} violates 91B RLL");
            let (_, dsv91) = efm::dsv(&efm::nrzi(&enc91.codes));
            assert_eq!(dsv91, e.dsv_peak);
            assert!(dsv91 <= DSV_SOFT, "entry {i} 91B dsv {dsv91} > {DSV_SOFT}");

            let enc81 = efm::encode_after_sync(&e.bytes[..CORE_BYTES]);
            assert!(enc81.choices.is_empty(), "entry {i} has 81B choices: {:?}", enc81.choices);
            assert!(efm::rll_ok(&enc81.codes), "entry {i} violates 81B RLL");
            let (_, dsv81) = efm::dsv(&efm::nrzi(&enc81.codes));
            assert!(dsv81 <= DSV_SOFT, "entry {i} 81B dsv {dsv81} > {DSV_SOFT}");

            assert!(e.purity > 0.45, "entry {i} f0={} purity={}", e.f0, e.purity);
        }
    }
}
