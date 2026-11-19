//! ECMA-330, the DVD-RAM medium.
//!
//! Discratcher records DVDs with EFMPlus (ECMA-267) and CDs with EFM (ECMA-130).
//! DVD-RAM records with neither: its bytes are turned into 16-bit code words by the
//! 8-to-16 code of Annex G, and the encoder keeps a state so that the digital sum
//! value stays low. The medium is laid out in zones of its own, with a sector of
//! 2 697 bytes instead of 2 048, a tighter track pitch and a rotation speed that
//! falls from zone to zone. It is a format of its own, not a DVD with a new name.
//!
//! Nothing here records a DVD-RAM: the drives in this house cannot write one, the
//! discs are long out of production, and the burning stack of the system has no
//! DVD-RAM writer to talk to. What the module gives the rest of the program is the
//! numbers a refusal has to be right about, and the record code that a writer would
//! have to speak.
//!
//! Every number below is from ECMA-330, third edition, June 2005.

/// Bytes a sector holds on the disc: the header, the user data and the error
/// detection code.
pub const SECTOR_BYTES: usize = 2_697;
/// User bytes in a sector.
pub const USER_DATA_BYTES: usize = 2_048;
/// A data frame: the header, the main data and the EDC.
pub const DATA_FRAME_BYTES: usize = 2_064;
/// Channel bits one byte is recorded in.
pub const CHANNEL_BITS_PER_BYTE: u32 = 16;
/// Channel bits one sector is recorded in.
pub const SECTOR_CHANNEL_BITS: u32 = 43_152;
/// User data bits per second. The same on every zone of the disc.
pub const USER_BIT_RATE: f64 = 22_160_000.0;
/// Period of one channel bit.
pub const CHANNEL_BIT_PERIOD_NS: f64 = 17.13;
/// Period of one byte.
pub const BYTE_PERIOD_NS: f64 = 274.0;
/// Period of one sector.
pub const SECTOR_PERIOD_US: f64 = 739.0;
/// Track pitch in the rewritable area, where the recording layer is.
pub const REWRITABLE_TRACK_PITCH_UM: f64 = 0.615;
/// Track pitch in the embossed area.
pub const EMBOSSED_TRACK_PITCH_UM: f64 = 0.74;
/// Zones in the rewritable area of a 120 mm disc.
pub const ZONE_COUNT: usize = 35;
/// Sectors in one ECC block.
pub const ECC_BLOCK_SECTORS: usize = 16;
/// Bytes in one ECC block: 208 rows of 182 bytes.
pub const ECC_BLOCK_BYTES: usize = 37_856;
/// Recording frames one ECC block is interleaved into.
pub const RECORDING_FRAMES_PER_BLOCK: usize = 16;
/// Bytes in one recording frame.
pub const RECORDING_FRAME_BYTES: usize = 2_366;
/// Sectors of a 120 mm Type 1S disc, the 4.7 GB one.
pub const TYPE1_SECTORS: u32 = 2_295_104;
/// Sectors of a 120 mm Type 2S disc, the 9.4 GB one.
pub const TYPE2_SECTORS: u32 = 4_589_844;
/// The sixteen scrambler presets, in the order of the preset numbers. The same values
/// ECMA-267 uses for a DVD.
pub const SCRAMBLER_PRESETS: [u16; 16] = [
    0x0001, 0x5500, 0x0002, 0x2A00, 0x0004, 0x5400, 0x0008, 0x2800, 0x0010, 0x5000, 0x0020, 0x2001,
    0x0040, 0x4002, 0x0080, 0x0005,
];

/// The physical sector the first logical block of the user area sits at.
///
/// The Data Zone starts at sector (031000), and the sector number written into
/// the Data ID of a frame is that sector's physical number.
pub const FIRST_PHYSICAL_SECTOR: u32 = 0x3_1000;

/// The physical sector of a logical block of the user area.
pub const fn physical_sector(lba: u32) -> u32 {
    lba + FIRST_PHYSICAL_SECTOR
}

/// The scrambler preset a logical block is recorded with.
///
/// The pre-set number is bits b7 to b4 of the Data ID, and the Data ID carries
/// the physical sector number, so the phase of the sequence follows the
/// physical number. The offset is a whole number of 256 sectors, so the phase
/// is the same one a DVD uses and the same table of pre-set values applies.
#[inline]
pub fn preset_index(lba: u32) -> usize {
    ((physical_sector(lba) >> 4) & 0x0F) as usize
}

/// The scrambler of a DVD-RAM: the same feedback register ECMA-267 uses, with
/// the same sixteen pre-set values and the same phase.
pub type ScramblerLut = [[u8; USER_DATA_BYTES]; 16];

/// The pre-set value of each of the sixteen pre-set numbers.
pub fn build_scrambler_lut() -> Box<ScramblerLut> {
    let mut lut = Box::new([[0u8; USER_DATA_BYTES]; 16]);
    for (row, &seed) in lut.iter_mut().zip(SCRAMBLER_PRESETS.iter()) {
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
    let table = &lut[preset_index(lba)];
    for (b, s) in sector.iter_mut().zip(table.iter()) {
        *b ^= s;
    }
}

/// Sectors the user area of a 120 mm DVD-RAM holds.
///
/// The Data Zone holds more, and the defect management takes the spare areas
/// out of it, but what the drive shows the host is this many logical blocks.
/// The disc in the Pioneer reports exactly this many.
pub const USER_AREA_SECTORS: u32 = 2_295_072;

/// Nominal length of a channel bit on the rewritable area.
///
/// The period of a channel bit is the same on every zone, and the linear
/// density is the same to within a percent, so the bit keeps about the same
/// length from the inner to the outer radius. The value here is the one that
/// makes the whole disc add up: the length of the spiral between the two radii
/// divided by the channel bits it carries.
pub const NOMINAL_BIT_LENGTH_NM: f64 = 142.9;

/// Inner radius of the Data Zone of a 120 mm disc, in millimetres.
pub const DATA_ZONE_INNER_RADIUS_MM: f64 = 24.1;
/// Outer radius of the Data Zone of a 120 mm disc, in millimetres.
pub const DATA_ZONE_OUTER_RADIUS_MM: f64 = 57.9;

/// Sectors per track in a zone of the rewritable area.
///
/// The standard raises the count by one per zone, from 25 in Zone 0 to 59 in Zone 34,
/// so that the linear density stays about the same from the inner to the outer radius.
pub const fn sectors_per_track(zone: u8) -> u32 {
    25 + zone as u32
}

/// Nominal rotation speed of a zone, in turns per second.
///
/// The standard gives the formula: the user data bit rate is the same on every zone,
/// so a zone with more sectors per track turns more slowly.
pub fn rotation_hz(sectors_per_track: u32) -> f64 {
    USER_BIT_RATE / (sectors_per_track as f64 * USER_DATA_BYTES as f64 * 8.0)
}

/// The same, for a zone number.
pub fn zone_rotation_hz(zone: u8) -> f64 {
    rotation_hz(sectors_per_track(zone))
}

/// The nominal rotation speed of every zone, as Table 3 of ECMA-330 gives it.
pub const PUBLISHED_ROTATION_HZ: [f64; ZONE_COUNT] = [
    54.10, 52.02, 50.09, 48.30, 46.64, 45.08, 43.63, 42.27, 40.99, 39.78, 38.64, 37.57, 36.56,
    35.59, 34.68, 33.81, 32.99, 32.20, 31.45, 30.74, 30.06, 29.40, 28.78, 28.18, 27.60, 27.05,
    26.52, 26.01, 25.52, 25.05, 24.59, 24.15, 23.73, 23.32, 22.92,
];

/// The scrambler preset a frame is recorded with.
///
/// The number is carried in bits b7 to b4 of the Data ID field, which the recorder
/// writes into the frame. The same preset is used for sixteen consecutive frames, and
/// the sequence of presets repeats after sixteen groups of sixteen frames.
pub fn preset_for_data_id(data_id: u8) -> u16 {
    SCRAMBLER_PRESETS[((data_id >> 4) & 0x0F) as usize]
}

/// Whether a medium name is a DVD-RAM.
pub fn is_dvd_ram(media_type: &str) -> bool {
    media_type.trim() == "DVD-RAM"
}

/// The byte that records the densest marks: five ONEs in every state, so a run of it
/// is a dark stretch of track. A DVD uses the same byte, and for the same reason.
pub const DARK_BYTE: u8 = 0xA5;
/// The byte that records the sparsest marks: two ONEs in every state. A DVD uses the
/// same byte, and for the same reason.
pub const LIGHT_BYTE: u8 = 0x92;

/// The 8-to-16 recording code of Annex G.
///
/// A byte becomes a 16-bit code word, and the encoder carries a state that says which
/// of the four columns of the conversion table to read. The table is Table G.1 of the
/// standard, taken from the PDF of the third edition and checked against the rules it
/// has to obey: every code word keeps at least two and at most ten ZEROs between two
/// ONEs.
pub mod record {
    /// The code word for every byte, one table per state.
    pub const CODE_WORDS: [[u16; 256]; 4] = [
[
    0x2009, 0x2012, 0x2120, 0x2048, 0x2090, 0x2024, 0x2024, 0x2048,
    0x2090, 0x2120, 0x2480, 0x2240, 0x2481, 0x2241, 0x2049, 0x2121,
    0x2091, 0x2022, 0x1009, 0x2011, 0x1012, 0x0802, 0x0401, 0x2220,
    0x2110, 0x2088, 0x2044, 0x1120, 0x2008, 0x1090, 0x1048, 0x1024,
    0x1004, 0x1004, 0x1024, 0x1048, 0x1090, 0x1120, 0x2008, 0x2044,
    0x2088, 0x2110, 0x2220, 0x2440, 0x1240, 0x0201, 0x2482, 0x2089,
    0x2441, 0x2242, 0x2221, 0x1049, 0x2122, 0x2111, 0x2092, 0x2042,
    0x2021, 0x0809, 0x1241, 0x1121, 0x1091, 0x1022, 0x1011, 0x0812,
    0x0402, 0x2420, 0x2210, 0x2108, 0x2084, 0x2010, 0x1088, 0x1220,
    0x1008, 0x1110, 0x1044, 0x0920, 0x0890, 0x0848, 0x0824, 0x0804,
    0x0804, 0x0824, 0x0848, 0x0890, 0x0920, 0x1008, 0x1044, 0x1088,
    0x1110, 0x1220, 0x2010, 0x2084, 0x2108, 0x2210, 0x2420, 0x0202,
    0x0101, 0x2489, 0x2492, 0x2442, 0x2421, 0x2249, 0x2222, 0x2211,
    0x2112, 0x2082, 0x2109, 0x2041, 0x1242, 0x1221, 0x1122, 0x1111,
    0x1092, 0x1042, 0x1089, 0x1021, 0x0921, 0x0891, 0x0849, 0x0822,
    0x0811, 0x0409, 0x0412, 0x2484, 0x2410, 0x2208, 0x2244, 0x1108,
    0x2124, 0x0888, 0x2104, 0x2020, 0x1210, 0x0808, 0x1084, 0x1010,
    0x0910, 0x0844, 0x0448, 0x0490, 0x0424, 0x0404, 0x0404, 0x0424,
    0x0448, 0x0490, 0x0808, 0x0844, 0x0888, 0x0910, 0x1010, 0x1084,
    0x1108, 0x1210, 0x2020, 0x2104, 0x2124, 0x2208, 0x2244, 0x2410,
    0x2484, 0x0212, 0x0209, 0x0102, 0x0081, 0x2491, 0x2422, 0x2449,
    0x2411, 0x2212, 0x2102, 0x2209, 0x2081, 0x1222, 0x1211, 0x1112,
    0x1082, 0x1249, 0x1041, 0x0922, 0x0911, 0x1109, 0x0892, 0x0842,
    0x0889, 0x0821, 0x0491, 0x0422, 0x0449, 0x0411, 0x0248, 0x0224,
    0x0204, 0x2488, 0x2444, 0x2408, 0x2224, 0x2204, 0x2248, 0x1244,
    0x1124, 0x1104, 0x1208, 0x1020, 0x0884, 0x0810, 0x0908, 0x0488,
    0x0444, 0x0408, 0x0204, 0x0224, 0x0248, 0x0408, 0x0444, 0x0488,
    0x0810, 0x0884, 0x0908, 0x1020, 0x1104, 0x1124, 0x1208, 0x1244,
    0x2204, 0x2224, 0x2248, 0x2408, 0x2444, 0x2488, 0x2040, 0x0249,
    0x0222, 0x0211, 0x0112, 0x0109, 0x0082, 0x0041, 0x2412, 0x2202,
    0x2409, 0x2101, 0x1212, 0x1102, 0x1209, 0x1081, 0x0912, 0x0882,
    0x0909, 0x0841, 0x0492, 0x0442, 0x0489, 0x0421, 0x0244, 0x0208,
],
[
    0x4120, 0x2012, 0x2120, 0x4480, 0x2090, 0x2024, 0x2024, 0x4012,
    0x2090, 0x2120, 0x2480, 0x2240, 0x2481, 0x2241, 0x4024, 0x2121,
    0x2091, 0x2022, 0x4090, 0x2011, 0x1012, 0x0802, 0x0401, 0x2220,
    0x2110, 0x4024, 0x2044, 0x1120, 0x4090, 0x1090, 0x4120, 0x1024,
    0x1004, 0x1004, 0x1024, 0x4240, 0x1090, 0x1120, 0x4901, 0x2044,
    0x4481, 0x2110, 0x2220, 0x2440, 0x1240, 0x4440, 0x2482, 0x4241,
    0x2441, 0x2242, 0x2221, 0x4121, 0x2122, 0x2111, 0x2092, 0x2042,
    0x2021, 0x4091, 0x1241, 0x1121, 0x1091, 0x1022, 0x1011, 0x0812,
    0x0402, 0x2420, 0x2210, 0x4022, 0x2084, 0x2010, 0x4220, 0x1220,
    0x4110, 0x1110, 0x1044, 0x0920, 0x0890, 0x4044, 0x0824, 0x0804,
    0x0804, 0x0824, 0x4044, 0x0890, 0x0920, 0x4110, 0x1044, 0x4220,
    0x1110, 0x1220, 0x2010, 0x2084, 0x4011, 0x2210, 0x2420, 0x4902,
    0x4881, 0x4420, 0x2492, 0x2442, 0x2421, 0x4482, 0x2222, 0x2211,
    0x2112, 0x2082, 0x4210, 0x2041, 0x1242, 0x1221, 0x1122, 0x1111,
    0x1092, 0x1042, 0x4420, 0x1021, 0x0921, 0x0891, 0x4441, 0x0822,
    0x0811, 0x4242, 0x0412, 0x2484, 0x2410, 0x4221, 0x2244, 0x4122,
    0x2124, 0x4111, 0x2104, 0x2020, 0x1210, 0x4092, 0x1084, 0x1010,
    0x0910, 0x0844, 0x4042, 0x0490, 0x0424, 0x0404, 0x0404, 0x0424,
    0x4084, 0x0490, 0x4010, 0x0844, 0x4084, 0x0910, 0x1010, 0x1084,
    0x4210, 0x1210, 0x2020, 0x2104, 0x2124, 0x4021, 0x2244, 0x2410,
    0x2484, 0x4010, 0x4924, 0x4924, 0x4912, 0x2491, 0x2422, 0x4904,
    0x2411, 0x2212, 0x2102, 0x4820, 0x2081, 0x1222, 0x1211, 0x1112,
    0x1082, 0x4882, 0x1041, 0x0922, 0x0911, 0x4820, 0x0892, 0x0842,
    0x4484, 0x0821, 0x0491, 0x0422, 0x4841, 0x0411, 0x4484, 0x4410,
    0x4244, 0x4410, 0x2444, 0x4492, 0x2224, 0x2204, 0x4442, 0x1244,
    0x1124, 0x1104, 0x4421, 0x1020, 0x0884, 0x0810, 0x4222, 0x4211,
    0x0444, 0x4112, 0x4082, 0x4124, 0x4104, 0x4041, 0x0444, 0x4020,
    0x0810, 0x0884, 0x4104, 0x1020, 0x1104, 0x1124, 0x4124, 0x1244,
    0x2204, 0x2224, 0x4244, 0x4904, 0x2444, 0x4020, 0x2040, 0x4922,
    0x4884, 0x4810, 0x4040, 0x4911, 0x4892, 0x4842, 0x2412, 0x2202,
    0x4884, 0x2101, 0x1212, 0x1102, 0x4821, 0x1081, 0x0912, 0x0882,
    0x4491, 0x0841, 0x0492, 0x0442, 0x4422, 0x0421, 0x4411, 0x4212,
],
[
    0x2009, 0x8120, 0x8012, 0x2048, 0x8120, 0x9200, 0x8900, 0x2048,
    0x8480, 0x9201, 0x8901, 0x8090, 0x8090, 0x8481, 0x2049, 0x8241,
    0x8121, 0x8240, 0x1009, 0x9100, 0x8880, 0x8091, 0x8049, 0x8048,
    0x8048, 0x2088, 0x8022, 0x8011, 0x2008, 0x9202, 0x1048, 0x9101,
    0x8902, 0x8881, 0x8024, 0x1048, 0x8024, 0x8440, 0x2008, 0x9080,
    0x2088, 0x8482, 0x8220, 0x8441, 0x8220, 0x8242, 0x8221, 0x2089,
    0x8110, 0x8088, 0x8110, 0x1049, 0x8122, 0x8111, 0x8092, 0x8089,
    0x8042, 0x0809, 0x8021, 0x4049, 0x9212, 0x9209, 0x9102, 0x8044,
    0x4048, 0x8420, 0x8210, 0x2108, 0x8108, 0x8084, 0x1088, 0x4088,
    0x1008, 0x8044, 0x4048, 0x8420, 0x8210, 0x0848, 0x8108, 0x8084,
    0x4088, 0x8840, 0x0848, 0x8088, 0x9248, 0x1008, 0x9224, 0x1088,
    0x9248, 0x9081, 0x8912, 0x8909, 0x2108, 0x8882, 0x8841, 0x8492,
    0x8489, 0x2489, 0x9204, 0x9224, 0x8442, 0x2249, 0x8421, 0x8249,
    0x8222, 0x8211, 0x2109, 0x8112, 0x8109, 0x8082, 0x8041, 0x4089,
    0x9249, 0x9222, 0x1089, 0x9211, 0x9112, 0x9109, 0x0849, 0x8924,
    0x8904, 0x0409, 0x8820, 0x8484, 0x8410, 0x2208, 0x8244, 0x1108,
    0x8208, 0x0888, 0x8124, 0x9204, 0x8924, 0x0808, 0x8820, 0x8484,
    0x8410, 0x8244, 0x0448, 0x8208, 0x9082, 0x8104, 0x8124, 0x8104,
    0x0448, 0x9040, 0x0808, 0x8020, 0x0888, 0x8020, 0x4108, 0x8040,
    0x1108, 0x9041, 0x4108, 0x9124, 0x8922, 0x2208, 0x8904, 0x9244,
    0x9208, 0x8911, 0x8892, 0x8889, 0x8842, 0x9124, 0x9104, 0x2449,
    0x9244, 0x8821, 0x8491, 0x2209, 0x8449, 0x8422, 0x8411, 0x8212,
    0x8209, 0x1249, 0x8102, 0x8081, 0x4909, 0x1109, 0x4489, 0x4249,
    0x0889, 0x9020, 0x8908, 0x8884, 0x0449, 0x8810, 0x8488, 0x8444,
    0x8408, 0x2488, 0x8248, 0x2408, 0x8224, 0x8204, 0x2248, 0x4208,
    0x9020, 0x8908, 0x1208, 0x8884, 0x8488, 0x8444, 0x0908, 0x0488,
    0x8224, 0x0408, 0x8408, 0x8248, 0x8204, 0x0408, 0x4208, 0x0488,
    0x9210, 0x9104, 0x0908, 0x4109, 0x9210, 0x9108, 0x1208, 0x9208,
    0x8810, 0x9242, 0x2248, 0x2408, 0x9108, 0x2488, 0x9221, 0x9122,
    0x9111, 0x9092, 0x9089, 0x9042, 0x9021, 0x8921, 0x8891, 0x9084,
    0x2409, 0x9084, 0x8080, 0x8849, 0x1209, 0x8822, 0x8811, 0x8412,
    0x0909, 0x8409, 0x8202, 0x8101, 0x0489, 0x4889, 0x9010, 0x8910,
],
[
    0x4120, 0x8120, 0x8012, 0x4480, 0x8120, 0x9200, 0x8900, 0x4012,
    0x8480, 0x9201, 0x8901, 0x8090, 0x8090, 0x8481, 0x4024, 0x8241,
    0x8121, 0x8240, 0x4090, 0x9100, 0x8880, 0x8091, 0x8049, 0x8048,
    0x8048, 0x4024, 0x8022, 0x8011, 0x4090, 0x9202, 0x4120, 0x9101,
    0x8902, 0x8881, 0x8024, 0x4240, 0x8024, 0x8440, 0x4901, 0x9080,
    0x4481, 0x8482, 0x8220, 0x8441, 0x8220, 0x4440, 0x8221, 0x4241,
    0x8110, 0x8088, 0x8110, 0x4121, 0x8122, 0x8111, 0x8092, 0x8089,
    0x8042, 0x4091, 0x8021, 0x4049, 0x9212, 0x9209, 0x9102, 0x8044,
    0x4048, 0x8420, 0x8210, 0x4022, 0x8108, 0x8084, 0x4220, 0x4088,
    0x4110, 0x8044, 0x4048, 0x8420, 0x8210, 0x4044, 0x8108, 0x8084,
    0x4088, 0x8840, 0x4044, 0x8088, 0x9248, 0x4110, 0x9224, 0x4220,
    0x9248, 0x9081, 0x8912, 0x8909, 0x4011, 0x8882, 0x8841, 0x4902,
    0x4881, 0x4420, 0x9204, 0x9224, 0x8442, 0x4482, 0x8421, 0x8249,
    0x8222, 0x8211, 0x4210, 0x8112, 0x8109, 0x8082, 0x8041, 0x4089,
    0x9249, 0x9222, 0x4420, 0x9211, 0x9112, 0x9109, 0x4441, 0x8924,
    0x8904, 0x4242, 0x8820, 0x8484, 0x8410, 0x4221, 0x8244, 0x4122,
    0x8208, 0x4111, 0x8124, 0x9204, 0x8924, 0x4092, 0x8820, 0x8484,
    0x8410, 0x8244, 0x4042, 0x8208, 0x9082, 0x8104, 0x8124, 0x8104,
    0x4084, 0x9040, 0x4010, 0x8020, 0x4084, 0x8020, 0x4108, 0x8040,
    0x4210, 0x9041, 0x4108, 0x9124, 0x8922, 0x4021, 0x4900, 0x9244,
    0x9208, 0x4010, 0x4924, 0x4924, 0x4912, 0x9124, 0x9104, 0x4904,
    0x9244, 0x8821, 0x8491, 0x4820, 0x8449, 0x8422, 0x8411, 0x8212,
    0x8209, 0x4882, 0x8102, 0x8081, 0x4909, 0x4820, 0x4489, 0x4249,
    0x4484, 0x9020, 0x8908, 0x8884, 0x4841, 0x8810, 0x4484, 0x4410,
    0x4244, 0x4410, 0x8248, 0x4492, 0x8224, 0x8204, 0x4442, 0x4208,
    0x9020, 0x8908, 0x4421, 0x8884, 0x8488, 0x8444, 0x4222, 0x4211,
    0x8224, 0x4112, 0x4082, 0x4124, 0x4104, 0x4041, 0x4208, 0x4020,
    0x9210, 0x9104, 0x4104, 0x4109, 0x9210, 0x9108, 0x4124, 0x9208,
    0x8810, 0x9242, 0x4244, 0x4904, 0x9108, 0x4020, 0x9221, 0x4922,
    0x4884, 0x4810, 0x4040, 0x4911, 0x4892, 0x4842, 0x8891, 0x9084,
    0x4884, 0x9084, 0x8080, 0x8849, 0x4821, 0x8822, 0x8811, 0x8412,
    0x4491, 0x8409, 0x8202, 0x8101, 0x4422, 0x4889, 0x4411, 0x4212,
]
    ];

    /// The state the encoder is in after that byte.
    pub const NEXT_STATES: [[u8; 256]; 4] = [
[
    1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2,
],
[
    2, 1, 2, 4, 2, 2, 3, 1, 3, 3, 4, 4, 1, 1, 3, 1,
    1, 1, 2, 1, 1, 1, 1, 2, 2, 2, 2, 2, 3, 2, 3, 2,
    2, 3, 3, 4, 3, 3, 1, 3, 1, 3, 3, 4, 4, 4, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 2, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 1, 3, 3, 1,
    1, 2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 1, 1, 1, 1, 1,
    1, 1, 3, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 1, 2, 1,
    2, 1, 2, 2, 2, 1, 2, 2, 2, 2, 1, 2, 2, 2, 3, 3,
    2, 3, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 1, 3, 3,
    3, 3, 2, 3, 1, 1, 1, 2, 1, 1, 1, 3, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 2, 1, 1, 3, 1, 1, 1, 1, 1, 2, 2,
    2, 3, 2, 1, 2, 2, 1, 2, 2, 2, 1, 2, 2, 2, 1, 1,
    2, 1, 1, 2, 2, 1, 3, 2, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 4, 1, 2, 2, 4, 1, 1, 1, 1, 1,
    3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
],
[
    1, 3, 1, 2, 2, 4, 4, 3, 4, 1, 1, 3, 2, 1, 1, 1,
    1, 4, 1, 4, 4, 1, 1, 2, 3, 2, 1, 1, 2, 1, 2, 1,
    1, 1, 2, 3, 3, 4, 3, 4, 3, 1, 2, 1, 3, 1, 1, 1,
    2, 2, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 2, 3, 3,
    3, 4, 3, 3, 2, 3, 2, 3, 3, 1, 1, 1, 3, 1, 1, 1,
    1, 1, 2, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 2, 2, 1, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 3, 3, 2, 3, 3, 3, 3, 2, 3, 1, 2, 3, 3,
    3, 4, 3, 2, 3, 3, 3, 4, 3, 1, 2, 3, 1, 3, 3, 2,
    2, 1, 1, 1, 1, 2, 2, 1, 3, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 2, 3, 3, 3, 2, 2,
    3, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 1, 2, 2, 3, 3,
    3, 1, 3, 3, 3, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3,
    1, 2, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2,
],
[
    2, 3, 1, 4, 2, 4, 4, 1, 4, 1, 1, 3, 2, 1, 3, 1,
    1, 4, 2, 4, 4, 1, 1, 2, 3, 2, 1, 1, 3, 1, 3, 1,
    1, 1, 2, 4, 3, 4, 1, 4, 1, 1, 2, 1, 3, 4, 1, 1,
    2, 2, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2,
    2, 2, 2, 1, 2, 2, 2, 2, 2, 3, 3, 3, 3, 2, 3, 3,
    3, 4, 3, 3, 2, 3, 2, 3, 3, 1, 1, 1, 1, 1, 1, 1,
    1, 2, 2, 3, 1, 1, 1, 1, 1, 1, 2, 1, 1, 1, 1, 1,
    1, 1, 3, 1, 1, 1, 1, 2, 2, 1, 2, 2, 2, 1, 2, 1,
    2, 1, 2, 3, 3, 1, 3, 3, 3, 3, 1, 3, 1, 2, 3, 3,
    2, 4, 2, 2, 3, 3, 3, 4, 3, 1, 2, 3, 1, 1, 4, 2,
    2, 3, 2, 3, 1, 2, 2, 2, 3, 1, 1, 3, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 2, 1, 1, 3, 2, 2, 2, 1, 2, 2, 2,
    2, 3, 2, 1, 2, 2, 1, 2, 3, 3, 1, 3, 3, 3, 1, 1,
    3, 1, 1, 2, 2, 1, 3, 2, 3, 3, 3, 1, 2, 2, 3, 3,
    3, 1, 3, 3, 3, 3, 1, 1, 2, 2, 4, 1, 1, 1, 1, 3,
    3, 2, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
]
    ];

    /// The code word a byte is recorded in, and the state that follows it.
    pub fn encode(byte: u8, state: u8) -> (u16, u8) {
        let s = (state.clamp(1, 4) - 1) as usize;
        (CODE_WORDS[s][byte as usize], NEXT_STATES[s][byte as usize])
    }

    /// The channel bits of a code word. Under NRZI every ONE is a transition, so the
    /// ONEs of the word are the marks the disc carries.
    pub fn transitions(code_word: u16) -> u32 {
        code_word.count_ones()
    }

    /// The runs of ZEROs between the ONEs of a code word.
    pub fn zero_runs(code_word: u16) -> Vec<u32> {
        let ones: Vec<u32> = (0..16).filter(|b| code_word & (1 << b) != 0).collect();
        ones.windows(2).map(|w| w[1] - w[0] - 1).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sector_is_what_the_standard_says() {
        assert_eq!(SECTOR_BYTES, 2_697);
        assert_eq!(SECTOR_CHANNEL_BITS, SECTOR_BYTES as u32 * CHANNEL_BITS_PER_BYTE);
        assert_eq!(DATA_FRAME_BYTES, USER_DATA_BYTES + 16);
        assert_eq!(ECC_BLOCK_BYTES, 208 * 182);
        assert_eq!(RECORDING_FRAME_BYTES * RECORDING_FRAMES_PER_BLOCK, ECC_BLOCK_BYTES);
    }

    #[test]
    fn the_timing_follows_from_the_bit_rate() {
        let sector_us = USER_DATA_BYTES as f64 * 8.0 / USER_BIT_RATE * 1e6;
        assert!((sector_us - SECTOR_PERIOD_US).abs() < 1.0, "{sector_us}");
        let byte_ns = sector_us * 1e3 / SECTOR_BYTES as f64;
        assert!((byte_ns - BYTE_PERIOD_NS).abs() < 1.0, "{byte_ns}");
        let bit_ns = byte_ns / CHANNEL_BITS_PER_BYTE as f64;
        assert!((bit_ns - CHANNEL_BIT_PERIOD_NS).abs() < 0.05, "{bit_ns}");
    }

    #[test]
    fn every_zone_turns_at_the_published_speed() {
        for (zone, published) in PUBLISHED_ROTATION_HZ.iter().enumerate() {
            let z = zone as u8;
            assert_eq!(sectors_per_track(z), 25 + zone as u32);
            let got = (zone_rotation_hz(z) * 100.0).round() / 100.0;
            assert!((got - published).abs() < 0.001, "zone {zone}: {got} against {published}");
        }
    }

    /// The pre-set of a logical block comes from the physical sector, and
    /// the offset to it is a whole number of 256 sectors, so a DVD-RAM picks
    /// the same pre-set as a DVD does for the same logical block.
    #[test]
    fn the_scrambler_runs_in_the_phase_a_dvd_uses() {
        for lba in [0u32, 1, 15, 16, 255, 256, 100_000, 2_295_071] {
            assert_eq!(preset_index(lba), crate::ecma267::seed_index(lba), "lba {lba}");
        }
        // And the table of pre-set values is the same one.
        let ram = build_scrambler_lut();
        let dvd = crate::ecma267::build_scrambler_lut();
        for i in 0..16 {
            assert_eq!(ram[i], dvd[i], "pre-set {i}");
        }
    }

    /// The bit length that makes the zones of the disc add up.
    #[test]
    fn the_nominal_bit_length_fits_the_zones_of_the_disc() {
        let length_mm = std::f64::consts::PI
            * (DATA_ZONE_OUTER_RADIUS_MM.powi(2) - DATA_ZONE_INNER_RADIUS_MM.powi(2))
            / (REWRITABLE_TRACK_PITCH_UM * 1e-3);
        let bits = USER_AREA_SECTORS as f64 * SECTOR_CHANNEL_BITS as f64;
        let bit_nm = length_mm / bits * 1e6;
        assert!((bit_nm - NOMINAL_BIT_LENGTH_NM).abs() < 0.5, "{bit_nm}");
    }

    #[test]
    fn the_presets_are_the_ones_a_dvd_uses() {
        assert_eq!(SCRAMBLER_PRESETS, crate::ecma267::LFSR_SEEDS);
        assert_eq!(preset_for_data_id(0x00), SCRAMBLER_PRESETS[0]);
        assert_eq!(preset_for_data_id(0xF0), SCRAMBLER_PRESETS[15]);
    }

    #[test]
    fn the_code_words_keep_the_run_length_the_standard_asks_for() {
        for state in 0..4 {
            for byte in 0..256usize {
                let word = record::CODE_WORDS[state][byte];
                for run in record::zero_runs(word) {
                    assert!((2..=10).contains(&run), "byte {byte} state {state}: {run} in {word:016b}");
                }
            }
        }
    }

    #[test]
    fn the_dark_and_the_light_byte_are_the_ones_a_dvd_uses() {
        assert_eq!(DARK_BYTE, crate::ecma267::DARK_BYTE);
        assert_eq!(LIGHT_BYTE, crate::ecma267::LIGHT_BYTE);
        for state in 1..=4u8 {
            let (dark, _) = record::encode(DARK_BYTE, state);
            let (light, _) = record::encode(LIGHT_BYTE, state);
            assert_eq!(record::transitions(dark), 5, "state {state}");
            assert_eq!(record::transitions(light), 2, "state {state}");
        }
    }

    #[test]
    fn the_ambiguity_the_standard_points_out_is_here() {
        // The standard names these two bytes as the case where one code word leaves
        // two states, and the state that follows tells the two apart.
        let (five, after_five) = record::encode(5, 1);
        let (six, after_six) = record::encode(6, 1);
        assert_eq!(five, six);
        assert_ne!(after_five, after_six);
    }
}
