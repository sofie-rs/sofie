//! A panic while a lock was held must not fail every later caller of the lock.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn read<T: ?Sized>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub fn write<T: ?Sized>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_a_panic_poisoned_still_hands_out_its_value() {
        let mutex = Mutex::new(1);
        let rw = RwLock::new(2);
        let _ = std::panic::catch_unwind(|| {
            let _held = (lock(&mutex), write(&rw));
            panic!("poisoning both");
        });

        assert!(mutex.is_poisoned() && rw.is_poisoned());
        assert_eq!(*lock(&mutex), 1);
        assert_eq!(*read(&rw), 2);
        assert_eq!(*write(&rw), 2);
    }
}
