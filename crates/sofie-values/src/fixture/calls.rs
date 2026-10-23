use std::sync::{Mutex, MutexGuard};

use sofie_core::{Error, Result, unpoisoned};

use super::{Fixture, Seed};

#[derive(Default)]
pub struct Calls {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    made: Vec<&'static str>,
    failing: Vec<&'static str>,
    seed: Seed,
}

impl Calls {
    pub fn answer<T: Fixture>(&self, method: &'static str) -> Result<T> {
        let mut state = self.state();
        state.made.push(method);
        if state.failing.contains(&method) {
            return Err(Error::Unavailable(format!("the fake fails `{method}`")));
        }
        Ok(T::fixture(&mut state.seed))
    }

    pub fn value<T: Fixture>(&self, method: &'static str) -> T {
        let mut state = self.state();
        state.made.push(method);
        T::fixture(&mut state.seed)
    }

    pub fn fail(&self, method: &'static str) {
        self.state().failing.push(method);
    }

    #[must_use]
    pub fn made(&self) -> Vec<&'static str> {
        self.state().made.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        unpoisoned::lock(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_each_call_and_fails_the_methods_it_is_told_to() {
        let calls = Calls::default();

        let answered: Result<i64> = calls.answer("find");
        calls.fail("find");
        let failed: Result<i64> = calls.answer("find");

        assert!(answered.is_ok());
        assert!(matches!(failed, Err(Error::Unavailable(_))));
        assert_eq!(calls.made(), ["find", "find"]);
    }
}
