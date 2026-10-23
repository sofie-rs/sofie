use std::cell::RefCell;

use ring::rand::{SecureRandom, SystemRandom};

use crate::RandomnessUnavailable;

const BLOCK_BYTES: usize = 256;

thread_local! {
    static BLOCK: RefCell<Block> = const {
        RefCell::new(Block {
            bytes: [0; BLOCK_BYTES],
            next: BLOCK_BYTES,
        })
    };
}

/// Every byte is handed out once and cleared.
struct Block {
    bytes: [u8; BLOCK_BYTES],
    next: usize,
}

pub fn fill(out: &mut [u8]) -> Result<(), RandomnessUnavailable> {
    if out.len() > BLOCK_BYTES {
        return system_fill(out);
    }
    BLOCK
        .try_with(|block| block.borrow_mut().take(out))
        .unwrap_or_else(|_| system_fill(out))
}

impl Block {
    fn take(&mut self, out: &mut [u8]) -> Result<(), RandomnessUnavailable> {
        if BLOCK_BYTES - self.next < out.len() {
            system_fill(&mut self.bytes)?;
            self.next = 0;
        }
        let taken = &mut self.bytes[self.next..self.next + out.len()];
        out.copy_from_slice(taken);
        taken.fill(0);
        self.next += out.len();
        Ok(())
    }
}

fn system_fill(out: &mut [u8]) -> Result<(), RandomnessUnavailable> {
    SystemRandom::new()
        .fill(out)
        .map_err(|_| RandomnessUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_never_repeat_across_blocks_and_large_ones_go_to_the_system() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            let mut id = [0; 16];
            fill(&mut id).unwrap();
            assert!(seen.insert(id), "a draw repeated");
        }
        let mut large = [0; BLOCK_BYTES + 1];
        fill(&mut large).unwrap();
        assert!(large.iter().any(|&byte| byte != 0));
    }
}
