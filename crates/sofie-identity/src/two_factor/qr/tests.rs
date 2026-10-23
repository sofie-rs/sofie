use super::reed_solomon::{generator, multiply, remainder};
use super::{BLOCKS, EC_PER_BLOCK, Grid, QrCode, Version, format_bits, is_masked, placement_order};

/// "HELLO WORLD" at version 1-M, from Thonky's QR code tutorial (thonky.com/qr-code-tutorial).
const HELLO_WORLD_DATA: [u8; 16] = [
    32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17,
];
const HELLO_WORLD_CORRECTION: [u8; 10] = [196, 35, 39, 119, 235, 215, 231, 226, 93, 23];

#[test]
fn error_correction_matches_the_version_1_m_example() {
    assert_eq!(
        remainder(&HELLO_WORLD_DATA, &generator(10)),
        HELLO_WORLD_CORRECTION
    );
}

#[test]
fn format_bits_match_the_standard_s_table_for_level_m() {
    let table = [
        0b101_0100_0001_0010,
        0b101_0001_0010_0101,
        0b101_1110_0111_1100,
        0b101_1011_0100_1011,
        0b100_0101_1111_1001,
        0b100_0000_1100_1110,
        0b100_1111_1001_0111,
        0b100_1010_1010_0000,
    ];
    assert_eq!((0..8).map(format_bits).collect::<Vec<_>>(), table);
}

#[test]
fn version_bits_match_the_standard_s_table() {
    assert_eq!(
        (7..=10).map(super::version_bits).collect::<Vec<_>>(),
        [0x07C94, 0x085BC, 0x09A99, 0x0A4D3]
    );
}

#[test]
fn capacities_and_alignments_match_the_standard_s_tables() {
    let data_codewords = [1, 2, 5, 10, 40].map(|version| Version(version).data_codeword_count());
    assert_eq!(data_codewords, [16, 28, 86, 216, 2334]);
    let fitting = [14, 15, 213, 214, 2331].map(|bytes| Version::fitting(bytes).unwrap().0);
    assert_eq!(fitting, [1, 2, 10, 11, 40]);
    assert!(Version::fitting(2332).is_err());
    assert_eq!(Version(2).alignment_positions(), [6, 18]);
    assert_eq!(Version(14).alignment_positions(), [6, 26, 46, 66]);
    assert_eq!(Version(32).alignment_positions(), [6, 34, 60, 86, 112, 138]);
    assert_eq!(
        Version(40).alignment_positions(),
        [6, 30, 58, 86, 114, 142, 170]
    );
}

#[test]
fn codes_read_back_as_the_text_they_encode() {
    let uri = "otpauth://totp/Shop:someone%40example.com?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=Shop";
    for text in ["", "a", uri, &"x".repeat(400)] {
        let code = QrCode::encode(text.as_bytes()).unwrap();
        assert_eq!(decoded(&code), text.as_bytes());
    }
}

#[test]
fn finders_timing_and_the_dark_module_are_in_place() {
    let code = QrCode::encode(b"otpauth://totp/x").unwrap();
    let size = code.size;
    for (left, top) in [(0, 0), (size - 7, 0), (0, size - 7)] {
        for offset in 0..7 {
            assert!(code.is_dark(left + offset, top) && code.is_dark(left, top + offset));
        }
        assert!(!code.is_dark(left + 1, top + 1) && code.is_dark(left + 3, top + 3));
    }
    assert!((8..size - 8).all(|i| code.is_dark(i, 6) == (i % 2 == 0)));
    assert!(code.is_dark(8, size - 8));
    assert!(code.svg_path().starts_with("M16 16h28v4h-28z"));
}

/// Reads a code the way a scanner would once it has found the grid.
fn decoded(code: &QrCode) -> Vec<u8> {
    let version = Version((code.size - 17) / 4);
    let mask = read_mask(code);
    let reserved = Grid::with_function_patterns(version).reserved;
    let bits: Vec<bool> = placement_order(code.size)
        .filter(|&(x, y)| !reserved[y * code.size + x])
        .map(|(x, y)| code.is_dark(x, y) ^ is_masked(mask, x, y))
        .collect();
    let codewords: Vec<u8> = bits
        .as_chunks::<8>()
        .0
        .iter()
        .map(|byte| byte.iter().fold(0, |sum, bit| sum << 1 | u8::from(*bit)))
        .collect();
    let data = checked_data(version, &codewords);
    read_bytes(version, &data)
}

fn read_mask(code: &QrCode) -> usize {
    let positions = (0..6)
        .map(|i| (8, i))
        .chain([(8, 7), (8, 8), (7, 8)])
        .chain((9..15).map(|i| (14 - i, 8)));
    let read = positions.enumerate().fold(0, |bits, (i, (x, y))| {
        bits | usize::from(code.is_dark(x, y)) << i
    });
    (0..8)
        .find(|mask| format_bits(*mask) == read)
        .expect("format bits name a level M mask")
}

/// Undoes the interleaving and checks every block's syndromes are zero.
fn checked_data(version: Version, codewords: &[u8]) -> Vec<u8> {
    let (blocks, ec_length) = (BLOCKS[version.0], EC_PER_BLOCK[version.0]);
    let raw = version.raw_modules() / 8;
    let short_blocks = blocks - raw % blocks;
    let short_data = raw / blocks - ec_length;
    let mut split: Vec<Vec<u8>> = vec![Vec::new(); blocks];
    let mut next = codewords.iter().copied();
    for index in 0..=short_data {
        for (block, held) in split.iter_mut().enumerate() {
            if index < short_data || block >= short_blocks {
                held.push(next.next().unwrap());
            }
        }
    }
    for _ in 0..ec_length {
        for held in &mut split {
            held.push(next.next().unwrap());
        }
    }
    let mut data = Vec::new();
    for block in split {
        let mut root = 1;
        for _ in 0..ec_length {
            let syndrome = block.iter().fold(0, |sum, term| multiply(sum, root) ^ term);
            assert_eq!(syndrome, 0, "a block's syndrome is not zero");
            root = multiply(root, 2);
        }
        data.extend_from_slice(&block[..block.len() - ec_length]);
    }
    data
}

fn read_bytes(version: Version, data: &[u8]) -> Vec<u8> {
    let bits: Vec<bool> = data
        .iter()
        .flat_map(|byte| (0..8).rev().map(move |bit| (byte >> bit) & 1 == 1))
        .collect();
    let number = |bits: &[bool]| bits.iter().fold(0, |sum, bit| sum << 1 | usize::from(*bit));
    assert_eq!(number(&bits[..4]), 0b0100, "byte mode");
    let count_end = 4 + version.count_bits();
    let length = number(&bits[4..count_end]);
    bits[count_end..count_end + length * 8]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|byte| u8::try_from(number(byte)).unwrap())
        .collect()
}
