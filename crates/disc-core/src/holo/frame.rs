//! Where each main-data byte of a DVD sector lands on the track (ECMA-267 clauses 16-21).
//!
//! A data sector is 12 header bytes, 2048 main-data bytes and 4 EDC bytes, laid out as
//! 12 rows of 172. ECC adds 10 PI bytes to every row and one PO row per sector, giving a
//! 13 x 182 recording frame. Each row is recorded as two sync frames, each a 32-bit SYNC
//! code followed by 91 bytes of 16 channel bits: 1488 bits, 26 frames per sector.
//!
//! So main-data byte `i` is NOT at `i / 2048` of the sector's length: the header, the PI
//! columns and the PO row push it about. This module gives the exact position.

pub const ROWS: usize = 13;
pub const DATA_ROWS: usize = 12;
pub const ROW_DATA_BYTES: usize = 172;
pub const FRAME_BYTES: usize = 91;
pub const SYNC_BITS: u32 = 32;
pub const FRAME_BITS: u32 = SYNC_BITS + FRAME_BYTES as u32 * 16;
pub const FRAMES_PER_SECTOR: usize = ROWS * 2;
pub const HEADER_BYTES: usize = 12;
pub const MAIN_BYTES: usize = 2048;

/// Sync frame index in the sector and byte position inside that frame of main-data byte `i`.
#[inline]
pub fn main_byte_slot(i: usize) -> (usize, usize) {
    debug_assert!(i < MAIN_BYTES);
    let d = i + HEADER_BYTES;
    let (row, col) = (d / ROW_DATA_BYTES, d % ROW_DATA_BYTES);
    (row * 2 + col / FRAME_BYTES, col % FRAME_BYTES)
}

/// Channel bit, counted from the sector's first SYNC bit, where byte `i`'s code word starts.
#[inline]
pub fn main_byte_bit(i: usize) -> u32 {
    let (frame, pos) = main_byte_slot(i);
    frame as u32 * FRAME_BITS + SYNC_BITS + pos as u32 * 16
}

/// One sync frame's run of main-data bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSlot {
    pub frame: usize,
    /// First main-data index in this frame.
    pub first: usize,
    pub len: usize,
    /// The run begins with the frame's first byte, right after SYNC, so its first code
    /// word is in State 1 whatever came before.
    pub after_sync: bool,
}

/// Main-data runs of all sync frames that carry any, in track order. The PO row carries none.
pub fn frame_slots() -> Vec<FrameSlot> {
    let mut out: Vec<FrameSlot> = Vec::new();
    for i in 0..MAIN_BYTES {
        let (frame, pos) = main_byte_slot(i);
        match out.last_mut() {
            Some(s) if s.frame == frame => s.len += 1,
            _ => out.push(FrameSlot { frame, first: i, len: 1, after_sync: pos == 0 }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_adds_up() {
        let s = frame_slots();
        assert_eq!(s.len(), 24);
        let lens: Vec<usize> = s.iter().map(|f| f.len).collect();
        assert_eq!(lens[0], 79, "header takes the first 12");
        assert_eq!(lens[1], 81, "second frame ends in 10 PI bytes");
        assert_eq!(lens[2], 91);
        assert_eq!(*lens.last().unwrap(), 77, "EDC follows");
        assert_eq!(lens.iter().sum::<usize>(), MAIN_BYTES);
        assert_eq!(s.iter().filter(|f| f.len >= 81 && f.after_sync).count(), 22);
        assert!(!s[0].after_sync && s[1].after_sync);
        assert_eq!(FRAME_BITS * FRAMES_PER_SECTOR as u32, crate::ecma267::SECTOR_CHANNEL_BITS);
    }

    #[test]
    fn byte_positions() {
        assert_eq!(main_byte_bit(0), SYNC_BITS + 12 * 16);
        assert_eq!(main_byte_bit(79), FRAME_BITS + SYNC_BITS);
        // Last main byte: row 11, column 167, second frame, position 76.
        assert_eq!(main_byte_slot(2047), (23, 76));
        // The linear model the carver uses puts byte 1024 at the middle of the sector;
        // it really sits well before it, because the PO row and PI sit at the end.
        let mid = main_byte_bit(1024) as f64 / crate::ecma267::SECTOR_CHANNEL_BITS as f64;
        assert!(mid < 0.47, "{mid}");
    }
}
