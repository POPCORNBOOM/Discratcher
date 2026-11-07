//! ECMA-267 main-data scrambler.
//!
//! The drive XORs each 2048-byte sector with this stream before EFMPlus, so writing
//! `Q ^ stream` makes the drive record the wanted scrambled byte `Q`.
//!
//! Facts below were checked against a real, burned image rather than taken on faith:
//! feedback is r14 ^ r10 (x^15 + x^4 + 1), each output byte is the register's low
//! eight bits followed by eight shifts, and the register is preset again at the start
//! of EVERY sector. All 16 sectors of an ECC block use the same preset, so one table of
//! 16 x 2048 bytes covers the whole disc.

pub const SECTOR_BYTES: usize = 2048;
pub const ECC_BLOCK_SECTORS: usize = 16;
pub const ECC_BLOCK_BYTES: usize = SECTOR_BYTES * ECC_BLOCK_SECTORS;
pub const DEFAULT_TOTAL_SECTORS: u32 = 2_295_104;
pub const SECTOR_CHANNEL_BITS: u32 = 38_688;
pub const NOMINAL_BIT_LENGTH_NM: f64 = 133.3;
pub const PSN_OFFSET: u32 = 0x30000;
/// Scrambled byte with 5 NRZI transitions per codeword: dense short marks.
pub const DARK_BYTE: u8 = 0xA5;
/// Scrambled byte with 2 NRZI transitions per codeword: sparse long marks.
pub const LIGHT_BYTE: u8 = 0x92;

pub const LFSR_SEEDS: [u16; 16] = [
    0x0001, 0x5500, 0x0002, 0x2A00, 0x0004, 0x5400, 0x0008, 0x2800, 0x0010, 0x5000, 0x0020, 0x2001,
    0x0040, 0x4002, 0x0080, 0x0005,
];

pub type ScramblerLut = [[u8; SECTOR_BYTES]; 16];

#[inline]
pub fn seed_index(lba: u32) -> usize {
    (((lba + PSN_OFFSET) >> 4) & 0x0F) as usize
}

pub fn build_scrambler_lut() -> Box<ScramblerLut> {
    let mut lut = Box::new([[0u8; SECTOR_BYTES]; 16]);
    for (row, &seed) in lut.iter_mut().zip(LFSR_SEEDS.iter()) {
        let mut r = seed;
        for byte in row.iter_mut() {
            *byte = (r & 0xFF) as u8;
            for _ in 0..8 {
                let feedback = ((r >> 14) ^ (r >> 10)) & 1;
                r = ((r << 1) & 0x7FFF) | feedback;
            }
        }
    }
    lut
}

/// Turn wanted scrambled bytes into the bytes a host writes, in place. Its own inverse.
#[inline]
pub fn xor_sector(lut: &ScramblerLut, lba: u32, sector: &mut [u8]) {
    let table = &lut[seed_index(lba)];
    for (b, s) in sector.iter_mut().zip(table.iter()) {
        *b ^= *s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_published_vector() {
        let lut = build_scrambler_lut();
        let got: Vec<u8> = lut[0][..8].iter().map(|s| s ^ LIGHT_BYTE).collect();
        assert_eq!(got, [0x93, 0x92, 0xb0, 0x96, 0x96, 0x1a, 0x0a, 0x90]);
    }

    #[test]
    fn capacity_constants_agree() {
        assert_eq!(DEFAULT_TOTAL_SECTORS as u64 * SECTOR_BYTES as u64, 4_700_372_992);
        assert_eq!(DEFAULT_TOTAL_SECTORS % ECC_BLOCK_SECTORS as u32, 0);
        assert_eq!(SECTOR_CHANNEL_BITS, 26 * 1488);
    }

    #[test]
    fn seed_changes_every_ecc_block_and_repeats_every_256() {
        assert_eq!(seed_index(0), 0);
        assert_eq!(seed_index(15), 0);
        assert_eq!(seed_index(16), 1);
        assert_eq!(seed_index(255), 15);
        assert_eq!(seed_index(256), 0);
    }

    /// Real image from the sibling discprobe checkout, skipped when it is not on this machine.
    #[test]
    fn real_burned_image_descrambles_to_exactly_two_bytes() {
        let Some(path) = crate::probe_root().map(|r| r.join("carved_artwork.iso")) else { return };
        let Ok(mut f) = std::fs::File::open(&path) else { return };
        use std::io::{Read, Seek, SeekFrom};
        let lut = build_scrambler_lut();
        for k in 0..64u64 {
            let lba = (k * 35_911 + k * k * 3) as u32 % DEFAULT_TOTAL_SECTORS;
            f.seek(SeekFrom::Start(lba as u64 * 2048)).unwrap();
            let mut s = [0u8; 2048];
            f.read_exact(&mut s).unwrap();
            xor_sector(&lut, lba, &mut s);
            assert!(s.iter().all(|&b| b == DARK_BYTE || b == LIGHT_BYTE), "lba {lba}");
        }
    }
}
