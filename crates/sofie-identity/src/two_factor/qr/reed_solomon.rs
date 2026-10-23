//! Reed–Solomon error correction over GF(256) with QR's field polynomial x⁸+x⁴+x³+x²+1.

const FIELD_POLYNOMIAL: u16 = 0x11D;

pub(super) fn multiply(left: u8, right: u8) -> u8 {
    let mut product: u16 = 0;
    for bit in (0..8).rev() {
        product = (product << 1) ^ ((product >> 7) * FIELD_POLYNOMIAL);
        product ^= u16::from((right >> bit) & 1) * u16::from(left);
    }
    product.to_le_bytes()[0]
}

/// The product of (x − αⁱ) for i below `degree`, highest coefficient (always 1) left out.
pub(super) fn generator(degree: usize) -> Vec<u8> {
    let mut coefficients = vec![0; degree];
    coefficients[degree - 1] = 1;
    let mut root = 1;
    for _ in 0..degree {
        for index in 0..degree {
            coefficients[index] = multiply(coefficients[index], root);
            if let Some(next) = coefficients.get(index + 1).copied() {
                coefficients[index] ^= next;
            }
        }
        root = multiply(root, 2);
    }
    coefficients
}

pub(super) fn remainder(data: &[u8], generator: &[u8]) -> Vec<u8> {
    let mut remainder = vec![0; generator.len()];
    for byte in data {
        let factor = byte ^ remainder.remove(0);
        remainder.push(0);
        for (term, coefficient) in remainder.iter_mut().zip(generator) {
            *term ^= multiply(*coefficient, factor);
        }
    }
    remainder
}
