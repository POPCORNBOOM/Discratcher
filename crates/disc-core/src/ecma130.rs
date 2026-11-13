//! ECMA-130 (CD-ROM): where the bytes a host writes end up on the track.
//!
//! A drive wraps each 2048-byte sector into a 2352-byte raw sector, XORs bytes 12.. with a
//! fixed stream (Annex B), swaps each byte pair (clause 16) and feeds 24 bytes per frame
//! to the CIRC encoder (Annex C), which delays every byte by 3 to 108 frames. So one
//! sector's bytes are spread over two sectors' length of track, and carving has to place
//! each byte where it really lands.
//!
//! Written from the standard. The delay table was checked against the one in disc-probe's
//! `docs/CD_PHYSICAL_MODEL.md`, and the dark and light bytes are DiscImageStudio's.

pub const SECTOR_BYTES: usize = 2048;
pub const RAW_BYTES: usize = 2352;
pub const FRAME_BITS: u32 = 588;
pub const FRAMES_PER_SECTOR: u32 = 98;
pub const SECTOR_CHANNEL_BITS: u32 = FRAME_BITS * FRAMES_PER_SECTOR;
/// 1.2 m/s at 4.3218 Mbit/s. Real discs run 1.2 to 1.4 m/s; see `Media::bit_mm`.
pub const NOMINAL_BIT_LENGTH_NM: f64 = 1.2e9 / 4_321_800.0;
/// Longest CIRC delay, in frames.
pub const MAX_DELAY_FRAMES: u32 = 108;
/// Source: the two-level CD palette in DiscImageStudio (`0x0D` dark, `0xAA` light).
pub const DARK_BYTE: u8 = 0x0D;
pub const LIGHT_BYTE: u8 = 0xAA;

/// First user byte inside the raw sector. Mode 2 Form 1 puts an 8-byte subheader first.
pub const MODE1_USER_OFFSET: usize = 16;
pub const MODE2_FORM1_USER_OFFSET: usize = 24;

/// Annex B: x^15 + x + 1, preset to 1 after every sector's sync, low bit first. The 12
/// sync bytes are not scrambled, so they are zero here.
pub fn scramble_table() -> [u8; RAW_BYTES] {
    let mut out = [0u8; RAW_BYTES];
    let mut reg: u16 = 1;
    for byte in out.iter_mut().skip(12) {
        for bit in 0..8 {
            *byte |= ((reg & 1) as u8) << bit;
            reg = (reg >> 1) | (((reg ^ (reg >> 1)) & 1) << 14);
        }
    }
    out
}

/// XOR this over wanted recorded bytes to get the bytes a host writes. Its own inverse.
pub fn user_mask(user_offset: usize) -> [u8; SECTOR_BYTES] {
    let mut m = [0u8; SECTOR_BYTES];
    m.copy_from_slice(&scramble_table()[user_offset..user_offset + SECTOR_BYTES]);
    m
}

/// Where one of a frame's 24 input bytes goes: its symbol slot (0..32) in the recorded
/// frame, and how many frames later that frame is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub slot: u8,
    pub delay: u8,
}

/// Figure C.4 in closed form. The encoder lays the twelve words out as 0 4 8 1 5 9, four
/// Q parity symbols, then 2 6 10 3 7 11, and four P symbols close the frame. The first
/// six words wait two frames, symbol `s` then waits `4 s`, and every even symbol waits
/// one more.
pub fn placements() -> [Placement; 24] {
    const ORDER: [usize; 12] = [0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11];
    let mut out = [Placement::default(); 24];
    for (position, &word) in ORDER.iter().enumerate() {
        for half in 0..2 {
            let slot = 2 * position + half + if position >= 6 { 4 } else { 0 };
            let delay = 4 * slot + if slot < 12 { 2 } else { 0 } + if slot % 2 == 0 { 1 } else { 0 };
            out[2 * word + half] = Placement { slot: slot as u8, delay: delay as u8 };
        }
    }
    out
}

/// Centre of a symbol on the track, in channel bits: 24 sync, 3 merging, 14 + 3 of
/// subcode, then 32 symbols of 14 + 3.
#[inline]
pub fn symbol_bit(frame: f64, slot: u8) -> f64 {
    frame * FRAME_BITS as f64 + 44.0 + 17.0 * slot as f64 + 8.5
}

/// (frame, slot) of user byte `byte` of sector `lba`, with sector 0 starting frame 0.
pub fn locate_user_byte(p: &[Placement; 24], user_offset: usize, lba: u32, byte: usize) -> (u64, u8) {
    let swapped = (user_offset + byte) ^ 1;
    let q = p[swapped % 24];
    (lba as u64 * FRAMES_PER_SECTOR as u64 + (swapped / 24) as u64 + q.delay as u64, q.slot)
}

/// User-data offset of a raw sector as READ CD returns it, from its mode byte and, for
/// Mode 2, the form bit of its subheader. None for anything that holds no 2048 user bytes.
pub fn user_offset_of_raw(raw: &[u8]) -> Option<usize> {
    const SYNC: [u8; 12] = [0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0];
    if raw.len() < 24 || raw[..12] != SYNC {
        return None;
    }
    match raw[15] {
        1 => Some(MODE1_USER_OFFSET),
        2 if raw[18] & 0x20 == 0 => Some(MODE2_FORM1_USER_OFFSET),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// The opening of the Annex B table, as printed in the standard.
    #[test]
    fn scrambler_matches_published_table() {
        let t = scramble_table();
        assert!(t[..12].iter().all(|&b| b == 0));
        assert_eq!(
            t[12..44],
            [
                0x01, 0x80, 0x00, 0x60, 0x00, 0x28, 0x00, 0x1E, 0x80, 0x08, 0x60, 0x06, 0xA8, 0x02, 0xFE, 0x81, 0x80,
                0x60, 0x60, 0x28, 0x28, 0x1E, 0x9E, 0x88, 0x68, 0x66, 0xAE, 0xAA, 0xFC, 0x7F, 0x01, 0xE0
            ]
        );
        assert_eq!(user_mask(16)[0], t[16]);
        assert_eq!(user_mask(24)[2047], t[24 + 2047]);
    }

    /// The table in disc-probe's CD_PHYSICAL_MODEL.md: data slots in order, with their delays.
    #[test]
    fn delays_match_the_documented_table() {
        let p = placements();
        let mut by_slot: Vec<(u8, u8, usize)> = p.iter().enumerate().map(|(i, q)| (q.slot, q.delay, i / 2)).collect();
        by_slot.sort();
        let delays: Vec<u8> = by_slot.iter().map(|q| q.1).collect();
        assert_eq!(delays, [3, 6, 11, 14, 19, 22, 27, 30, 35, 38, 43, 46, 65, 68, 73, 76, 81, 84, 89, 92, 97, 100, 105, 108]);
        let words: Vec<usize> = by_slot.iter().step_by(2).map(|q| q.2).collect();
        assert_eq!(words, [0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11]);
        let slots: Vec<u8> = by_slot.iter().map(|q| q.0).collect();
        assert_eq!(slots, (0..12).chain(16..28).collect::<Vec<u8>>());
    }

    /// The encoder built stage by stage out of delay lines, fed with numbered bytes: each
    /// one must come out in the frame and slot the closed form names.
    #[test]
    fn closed_form_agrees_with_a_delay_line_encoder() {
        const ORDER: [usize; 12] = [0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11];
        let line = |n: usize| VecDeque::from(vec![None::<usize>; n]);
        let mut two: Vec<VecDeque<Option<usize>>> = (0..24).map(|_| line(2)).collect();
        let mut interleave: Vec<VecDeque<Option<usize>>> = (0..28).map(|s| line(4 * s)).collect();
        let mut one: Vec<VecDeque<Option<usize>>> = (0..32).map(|_| line(1)).collect();
        let p = placements();
        let mut seen = 0;
        for frame in 0..400usize {
            // Stage 1: words 0 4 8 1 5 9 wait two frames; then the C2 layout.
            let mut c2 = [None; 28];
            for (position, &word) in ORDER.iter().enumerate() {
                for half in 0..2 {
                    let i = 2 * word + half;
                    let mut v = Some(frame * 24 + i);
                    if position < 6 {
                        two[i].push_back(v);
                        v = two[i].pop_front().unwrap();
                    }
                    c2[2 * position + half + if position >= 6 { 4 } else { 0 }] = v;
                }
            }
            // Stage 2: symbol s waits 4 s frames. Stage 3: every even symbol one more.
            for s in 0..28 {
                interleave[s].push_back(c2[s]);
                let mut v = interleave[s].pop_front().unwrap();
                if s % 2 == 0 {
                    one[s].push_back(v);
                    v = one[s].pop_front().unwrap();
                }
                if let Some(tag) = v {
                    let q = p[tag % 24];
                    assert_eq!((tag / 24 + q.delay as usize, q.slot as usize), (frame, s), "byte {tag}");
                    seen += 1;
                }
            }
        }
        assert!(seen > 24 * 250);
    }

    #[test]
    fn user_bytes_swap_in_pairs_and_stay_in_their_frame() {
        let p = placements();
        // Mode 1 user byte 0 is raw byte 16, swapped to 17: word 8, second half.
        assert_eq!(locate_user_byte(&p, 16, 0, 0), (22, 5));
        assert_eq!(locate_user_byte(&p, 16, 0, 1), (19, 4));
        // The last user byte of sector 3, raw 2063 -> 2062, frame 85 of the sector.
        let (frame, slot) = locate_user_byte(&p, 16, 3, 2047);
        assert_eq!(frame, 3 * 98 + 85 + p[2062 % 24].delay as u64);
        assert_eq!(slot, p[2062 % 24].slot);
        assert_eq!(symbol_bit(1.0, 0), 588.0 + 52.5);
    }

    #[test]
    fn raw_sector_modes() {
        let mut raw = [0u8; 2352];
        raw[1..11].fill(255);
        raw[15] = 1;
        assert_eq!(user_offset_of_raw(&raw), Some(16));
        raw[15] = 2;
        assert_eq!(user_offset_of_raw(&raw), Some(24));
        raw[18] = 0x20;
        assert_eq!(user_offset_of_raw(&raw), None, "Form 2 holds 2324 bytes");
        raw[0] = 7;
        assert_eq!(user_offset_of_raw(&raw), None, "audio has no sync");
    }
}
