//! 8-to-16 modulation (ECMA-267 clause 20, 22 and Annex G), only as far as needed to know
//! exactly which marks a byte run becomes.
//!
//! The drive may pick between two code words in two cases (clause 22): b) any byte below
//! 88 has a substitution code word; c) a byte of 88 or more, when the State is 1 or 4, may
//! take its code word from the other of those two States if the run-length limit allows.
//! Which one it picks depends on DSV bookkeeping across the whole stream, with ties left
//! to the drive. A run with neither case is "choice free": its marks are fixed by the
//! bytes alone, up to an overall polarity, which no diffraction spectrum can see.

use super::efm_tables::{MAIN, SUBST};

/// The State after any SYNC code: every SYNC code ends in a ONE.
pub const STATE_AFTER_SYNC: u8 = 1;

#[inline]
fn leading_zeros(c: u16) -> u32 {
    c.leading_zeros()
}

#[inline]
fn trailing_zeros(c: u16) -> u32 {
    c.trailing_zeros()
}

/// Code word and next State for `byte` in `state` (1..=4) from the main table.
#[inline]
pub fn main_code(byte: u8, state: u8) -> (u16, u8) {
    MAIN[byte as usize][state as usize - 1]
}

/// Code word from the substitution table, for bytes below 88.
#[inline]
pub fn subst_code(byte: u8, state: u8) -> Option<(u16, u8)> {
    SUBST.get(byte as usize).map(|r| r[state as usize - 1])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub codes: Vec<u16>,
    /// State each byte was encoded in.
    pub states: Vec<u8>,
    /// Indices where the drive has a choice (case b or c).
    pub choices: Vec<usize>,
    pub end_state: u8,
}

/// Encode with the main table in the prescribed States, noting every position where a
/// drive could legally do otherwise. `prev_trailing` is the trailing ZEROs of what came
/// before: 0 after a SYNC code.
pub fn encode(bytes: &[u8], start_state: u8, prev_trailing: u32) -> Encoded {
    let mut s = start_state;
    let mut tz = prev_trailing;
    let mut out = Encoded { codes: Vec::with_capacity(bytes.len()), states: Vec::with_capacity(bytes.len()), choices: Vec::new(), end_state: s };
    for (j, &b) in bytes.iter().enumerate() {
        let (c, n) = main_code(b, s);
        if b < 88 {
            out.choices.push(j);
        } else if s == 1 || s == 4 {
            let alt = main_code(b, 5 - s).0;
            let zeros = tz + leading_zeros(alt);
            if (2..=10).contains(&zeros) {
                out.choices.push(j);
            }
        }
        out.codes.push(c);
        out.states.push(s);
        tz = trailing_zeros(c);
        s = n;
    }
    out.end_state = s;
    out
}

/// Encode a run that starts right after a SYNC code.
pub fn encode_after_sync(bytes: &[u8]) -> Encoded {
    encode(bytes, STATE_AFTER_SYNC, 0)
}

/// NRZI: each ONE in the code words flips the written level. Returns one level per
/// channel bit, `true` for a mark, starting from a space.
pub fn nrzi(codes: &[u16]) -> Vec<bool> {
    let mut level = false;
    let mut out = Vec::with_capacity(codes.len() * 16);
    for &c in codes {
        for k in (0..16).rev() {
            if (c >> k) & 1 == 1 {
                level = !level;
            }
            out.push(level);
        }
    }
    out
}

/// Positions (in channel bits) of every level change.
pub fn transitions(codes: &[u16]) -> Vec<u32> {
    let mut out = Vec::new();
    for (w, &c) in codes.iter().enumerate() {
        for k in 0..16u32 {
            if (c >> (15 - k)) & 1 == 1 {
                out.push(w as u32 * 16 + k);
            }
        }
    }
    out
}

/// Running digital sum (+1 per mark bit, -1 per space bit): (final, largest magnitude).
pub fn dsv(levels: &[bool]) -> (i32, i32) {
    let (mut d, mut peak) = (0i32, 0i32);
    for &l in levels {
        d += if l { 1 } else { -1 };
        peak = peak.max(d.abs());
    }
    (d, peak)
}

/// Does every gap between ONEs, including across code words, hold 2..=10 ZEROs?
pub fn rll_ok(codes: &[u16]) -> bool {
    let t = transitions(codes);
    t.windows(2).all(|w| (3..=11).contains(&(w[1] - w[0])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_obey_their_own_run_length_rules() {
        // Every code word leaving any state, followed by every code word allowed in its
        // next state, keeps 2..=10 ZEROs between ONEs.
        for b in 0..256usize {
            for s in 1..=4u8 {
                let mut firsts = vec![main_code(b as u8, s)];
                firsts.extend(subst_code(b as u8, s));
                for (c, n) in firsts {
                    for b2 in 0..=255u8 {
                        let mut seconds = vec![main_code(b2, n).0];
                        seconds.extend(subst_code(b2, n).map(|x| x.0));
                        for c2 in seconds {
                            assert!(rll_ok(&[c, c2]), "{b} s{s} then {b2} s{n}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn states_two_and_three_are_told_apart_by_x15_and_x3() {
        for b in 0..256 {
            let c2 = MAIN[b][1].0;
            let c3 = MAIN[b][2].0;
            assert_eq!(c2 & 0x8008, 0, "{b}");
            assert_ne!(c3 & 0x8008, 0, "{b}");
        }
    }

    /// Hans Chiu's "Blue [sub_free] (Init: S1)" sequence, transcribed from his screenshot.
    /// His plot colours each byte by its State; the first row there reads
    /// 1 3 2 1 1 1 1 3 3 2 1 1 2 1, which these tables reproduce.
    #[test]
    fn reproduces_a_published_state_sequence() {
        let blue: [u8; 14] = [0x58, 0xBB, 0xC6, 0x66, 0x67, 0x6F, 0x94, 0xCB, 0xBF, 0xFC, 0x67, 0xC8, 0xCE, 0x58];
        let e = encode_after_sync(&blue);
        assert_eq!(e.states, [1, 3, 2, 1, 1, 1, 1, 3, 3, 2, 1, 1, 2, 1]);
        assert!(e.choices.is_empty(), "{:?}", e.choices);
        assert!(rll_ok(&e.codes));
    }

    #[test]
    fn low_bytes_always_offer_a_choice() {
        let e = encode_after_sync(&[0x10, 0x92, 0x92]);
        assert_eq!(e.choices[0], 0);
    }

    #[test]
    fn nrzi_and_dsv() {
        let e = encode_after_sync(&[0x92; 8]);
        let lv = nrzi(&e.codes);
        assert_eq!(lv.len(), 128);
        let (_, peak) = dsv(&lv);
        assert!(peak <= 16, "0x92 is an 8T square wave: {peak}");
    }
}
