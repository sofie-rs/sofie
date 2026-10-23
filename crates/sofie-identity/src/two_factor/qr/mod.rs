//! QR codes (ISO/IEC 18004) in byte mode at error correction level M, drawn as an SVG path.

mod reed_solomon;
#[cfg(test)]
mod tests;

use sofie::prelude::*;

/// The light border a reader needs around the code, in modules.
const QUIET_ZONE: usize = 4;
const PIXELS_PER_MODULE: usize = 4;
const BYTE_MODE: usize = 0b0100;
const PAD_BYTES: [u8; 2] = [0xEC, 0x11];
/// Level M's two format bits are zero.
const LEVEL_M: usize = 0b00;
const FORMAT_MASK: usize = 0x5412;
const FORMAT_GENERATOR: usize = 0x537;
const VERSION_GENERATOR: usize = 0x1F25;
/// Level M's error correction codewords per block and blocks, by version.
const EC_PER_BLOCK: [usize; 41] = [
    0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
];
const BLOCKS: [usize; 41] = [
    0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23,
    25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
];
const PENALTY_RUN: usize = 3;
const PENALTY_BLOCK: usize = 3;
const PENALTY_FINDER_LIKE: usize = 40;
const PENALTY_BALANCE: usize = 10;
const FINDER_LIKE: [bool; 11] = [
    true, false, true, true, true, false, true, false, false, false, false,
];

pub(crate) struct QrCode {
    size: usize,
    dark: Vec<bool>,
}

impl QrCode {
    pub(crate) fn encode(text: &[u8]) -> Result<Self> {
        let version = Version::fitting(text.len())?;
        let codewords = version.with_error_correction(&version.data_codewords(text));
        let mut grid = Grid::with_function_patterns(version);
        grid.place(&codewords);
        let mask = (0..8)
            .min_by_key(|mask| grid.penalty_masked(*mask))
            .unwrap_or_default();
        grid.apply_mask(mask);
        grid.draw_format(mask);
        Ok(Self {
            size: grid.size,
            dark: grid.dark,
        })
    }

    #[cfg(test)]
    fn is_dark(&self, x: usize, y: usize) -> bool {
        self.dark[y * self.size + x]
    }

    pub(crate) fn pixels(&self) -> usize {
        (self.size + 2 * QUIET_ZONE) * PIXELS_PER_MODULE
    }

    /// One rectangle per run of dark modules in a row.
    pub(crate) fn svg_path(&self) -> String {
        let mut rectangles = Vec::new();
        for (y, row) in self.dark.chunks(self.size).enumerate() {
            let mut x = 0;
            for run in row.chunk_by(|left, right| left == right) {
                if run[0] {
                    rectangles.push(rectangle(x, y, run.len()));
                }
                x += run.len();
            }
        }
        rectangles.concat()
    }
}

fn rectangle(x: usize, y: usize, length: usize) -> String {
    let [left, top] = [x, y].map(|at| (at + QUIET_ZONE) * PIXELS_PER_MODULE);
    let (width, height) = (length * PIXELS_PER_MODULE, PIXELS_PER_MODULE);
    format!("M{left} {top}h{width}v{height}h-{width}z")
}

#[derive(Clone, Copy)]
struct Version(usize);

impl Version {
    fn fitting(bytes: usize) -> Result<Self> {
        (1..=40)
            .map(Self)
            .find(|version| version.data_bits(bytes) <= version.data_codeword_count() * 8)
            .ok_or_else(|| {
                Error::internal_message(format!("{bytes} bytes do not fit in a QR code"))
            })
    }

    fn size(self) -> usize {
        self.0 * 4 + 17
    }

    fn count_bits(self) -> usize {
        if self.0 < 10 { 8 } else { 16 }
    }

    fn data_bits(self, bytes: usize) -> usize {
        4 + self.count_bits() + bytes * 8
    }

    /// The modules left once every function pattern is drawn.
    fn raw_modules(self) -> usize {
        let version = self.0;
        let mut modules = (16 * version + 128) * version + 64;
        if version >= 2 {
            let alignments = version / 7 + 2;
            modules -= (25 * alignments - 10) * alignments - 55;
            if version >= 7 {
                modules -= 36;
            }
        }
        modules
    }

    fn data_codeword_count(self) -> usize {
        self.raw_modules() / 8 - EC_PER_BLOCK[self.0] * BLOCKS[self.0]
    }

    fn alignment_positions(self) -> Vec<usize> {
        if self.0 == 1 {
            return Vec::new();
        }
        let count = self.0 / 7 + 2;
        let step = (self.0 * 8 + count * 3 + 5) / (count * 4 - 4) * 2;
        let mut positions: Vec<usize> =
            (0..count - 1).map(|i| self.size() - 7 - i * step).collect();
        positions.push(6);
        positions.reverse();
        positions
    }

    fn data_codewords(self, text: &[u8]) -> Vec<u8> {
        let capacity = self.data_codeword_count() * 8;
        let mut bits = Bits::default();
        bits.push(BYTE_MODE, 4);
        bits.push(text.len(), self.count_bits());
        for byte in text {
            bits.push(usize::from(*byte), 8);
        }
        let terminator = (capacity - bits.0.len()).min(4);
        bits.push(0, terminator);
        bits.push(0, (8 - bits.0.len() % 8) % 8);
        let mut codewords = bits.bytes();
        let padding = capacity / 8 - codewords.len();
        codewords.extend(PAD_BYTES.iter().cycle().take(padding));
        codewords
    }

    /// Split into blocks, each followed by its error correction, then interleaved.
    fn with_error_correction(self, data: &[u8]) -> Vec<u8> {
        let (blocks, ec_length) = (BLOCKS[self.0], EC_PER_BLOCK[self.0]);
        let raw_codewords = self.raw_modules() / 8;
        let short_blocks = blocks - raw_codewords % blocks;
        let short_data = raw_codewords / blocks - ec_length;
        let divisor = reed_solomon::generator(ec_length);
        let mut rest = data;
        let split: Vec<(&[u8], Vec<u8>)> = (0..blocks)
            .map(|block| {
                let (taken, left) = rest.split_at(short_data + usize::from(block >= short_blocks));
                rest = left;
                (taken, reed_solomon::remainder(taken, &divisor))
            })
            .collect();
        let mut interleaved = Vec::with_capacity(raw_codewords);
        for index in 0..=short_data {
            interleaved.extend(split.iter().filter_map(|(block, _)| block.get(index)));
        }
        for index in 0..ec_length {
            interleaved.extend(split.iter().map(|(_, correction)| correction[index]));
        }
        interleaved
    }
}

#[derive(Default)]
struct Bits(Vec<bool>);

impl Bits {
    fn push(&mut self, value: usize, length: usize) {
        (0..length)
            .rev()
            .for_each(|bit| self.0.push((value >> bit) & 1 == 1));
    }

    fn bytes(&self) -> Vec<u8> {
        self.0
            .chunks(8)
            .map(|byte| byte.iter().fold(0, |sum, bit| sum << 1 | u8::from(*bit)))
            .collect()
    }
}

struct Grid {
    version: Version,
    size: usize,
    dark: Vec<bool>,
    reserved: Vec<bool>,
}

impl Grid {
    fn with_function_patterns(version: Version) -> Self {
        let size = version.size();
        let mut grid = Self {
            version,
            size,
            dark: vec![false; size * size],
            reserved: vec![false; size * size],
        };
        for i in 0..size {
            grid.set(6, i, i.is_multiple_of(2));
            grid.set(i, 6, i.is_multiple_of(2));
        }
        for (x, y) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            grid.draw_square(x, y, 4, |distance| distance != 2 && distance != 4);
        }
        grid.draw_alignments();
        grid.draw_format(0);
        grid.draw_version();
        grid
    }

    fn set(&mut self, x: usize, y: usize, dark: bool) {
        self.dark[y * self.size + x] = dark;
        self.reserved[y * self.size + x] = true;
    }

    /// Draws the modules up to `radius` around a centre, dark by their ring's distance.
    fn draw_square(&mut self, x: usize, y: usize, radius: usize, dark: impl Fn(usize) -> bool) {
        for dy in 0..=2 * radius {
            for dx in 0..=2 * radius {
                let (column, row) = ((x + dx).checked_sub(radius), (y + dy).checked_sub(radius));
                if let (Some(column), Some(row)) = (column, row)
                    && column < self.size
                    && row < self.size
                {
                    self.set(
                        column,
                        row,
                        dark(dx.abs_diff(radius).max(dy.abs_diff(radius))),
                    );
                }
            }
        }
    }

    fn draw_alignments(&mut self) {
        let positions = self.version.alignment_positions();
        let last = positions.len().saturating_sub(1);
        for (i, x) in positions.iter().enumerate() {
            for (j, y) in positions.iter().enumerate() {
                let on_finder = (i == 0 && (j == 0 || j == last)) || (i == last && j == 0);
                if !on_finder {
                    self.draw_square(*x, *y, 2, |distance| distance != 1);
                }
            }
        }
    }

    fn draw_format(&mut self, mask: usize) {
        let bits = format_bits(mask);
        let bit = |i: usize| (bits >> i) & 1 == 1;
        let size = self.size;
        for i in 0..6 {
            self.set(8, i, bit(i));
        }
        self.set(8, 7, bit(6));
        self.set(8, 8, bit(7));
        self.set(7, 8, bit(8));
        for i in 9..15 {
            self.set(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set(size - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set(8, size - 15 + i, bit(i));
        }
        self.set(8, size - 8, true);
    }

    fn draw_version(&mut self) {
        if self.version.0 < 7 {
            return;
        }
        let bits = version_bits(self.version.0);
        for i in 0..18 {
            let dark = (bits >> i) & 1 == 1;
            let (along, across) = (self.size - 11 + i % 3, i / 3);
            self.set(along, across, dark);
            self.set(across, along, dark);
        }
    }

    /// In two-column strips from the right, up and down in turn, skipping the timing column.
    fn place(&mut self, codewords: &[u8]) {
        let mut bits = codewords
            .iter()
            .flat_map(|codeword| (0..8).rev().map(move |bit| (codeword >> bit) & 1 == 1));
        for (x, y) in placement_order(self.size) {
            if !self.reserved[y * self.size + x] {
                self.dark[y * self.size + x] = bits.next().unwrap_or(false);
            }
        }
    }

    fn apply_mask(&mut self, mask: usize) {
        for y in 0..self.size {
            for x in 0..self.size {
                if !self.reserved[y * self.size + x] && is_masked(mask, x, y) {
                    self.dark[y * self.size + x] ^= true;
                }
            }
        }
    }

    fn penalty_masked(&mut self, mask: usize) -> usize {
        self.apply_mask(mask);
        self.draw_format(mask);
        let penalty = self.penalty();
        self.apply_mask(mask);
        penalty
    }

    fn penalty(&self) -> usize {
        let at = |x: usize, y: usize| self.dark[y * self.size + x];
        let lines = (0..self.size).flat_map(|i| {
            [
                (0..self.size).map(|x| at(x, i)).collect::<Vec<_>>(),
                (0..self.size).map(|y| at(i, y)).collect(),
            ]
        });
        let in_lines: usize = lines.map(|line| line_penalty(&line)).sum();
        let blocks = (0..self.size - 1)
            .flat_map(|y| (0..self.size - 1).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let colour = at(x, y);
                at(x + 1, y) == colour && at(x, y + 1) == colour && at(x + 1, y + 1) == colour
            })
            .count();
        let total = self.dark.len();
        let dark = self.dark.iter().filter(|dark| **dark).count();
        let balance = (dark * 20)
            .abs_diff(total * 10)
            .div_ceil(total)
            .saturating_sub(1);
        in_lines + blocks * PENALTY_BLOCK + balance * PENALTY_BALANCE
    }
}

fn line_penalty(line: &[bool]) -> usize {
    let runs = line
        .chunk_by(|left, right| left == right)
        .filter(|run| run.len() >= 5)
        .map(|run| PENALTY_RUN + run.len() - 5)
        .sum::<usize>();
    let finder_like = line
        .windows(FINDER_LIKE.len())
        .filter(|window| {
            window.iter().eq(&FINDER_LIKE) || window.iter().eq(FINDER_LIKE.iter().rev())
        })
        .count();
    runs + finder_like * PENALTY_FINDER_LIKE
}

fn placement_order(size: usize) -> impl Iterator<Item = (usize, usize)> {
    (0..size / 2)
        .map(move |strip| {
            let right = size - 1 - strip * 2;
            if right <= 6 { right - 1 } else { right }
        })
        .flat_map(move |right| {
            let upward = (right + 1) & 2 == 0;
            (0..size).flat_map(move |step| {
                let y = if upward { size - 1 - step } else { step };
                [(right, y), (right - 1, y)]
            })
        })
}

fn is_masked(mask: usize, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y).is_multiple_of(2),
        1 => y.is_multiple_of(2),
        2 => x.is_multiple_of(3),
        3 => (x + y).is_multiple_of(3),
        4 => (x / 3 + y / 2).is_multiple_of(2),
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3).is_multiple_of(2),
        _ => ((x + y) % 2 + x * y % 3).is_multiple_of(2),
    }
}

/// `data` followed by the remainder of its division by `generator`, a BCH code.
fn bch(data: usize, generator: usize) -> usize {
    let degree = generator.ilog2();
    let mut remainder = data << degree;
    while remainder != 0 && remainder.ilog2() >= degree {
        remainder ^= generator << (remainder.ilog2() - degree);
    }
    data << degree | remainder
}

fn format_bits(mask: usize) -> usize {
    bch(LEVEL_M << 3 | mask, FORMAT_GENERATOR) ^ FORMAT_MASK
}

fn version_bits(version: usize) -> usize {
    bch(version, VERSION_GENERATOR)
}
