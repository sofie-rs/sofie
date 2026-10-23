use std::path::{Path, PathBuf};

use crate::Code;
use crate::effects::Effects;

/// A call of an aggregate's crate-private `across`, which reads past the owner filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossOwnerCheck {
    pub aggregate: String,
    /// `actions::book_slot`, or `Booking::reschedule` for a method.
    pub caller: String,
    pub file: PathBuf,
    pub line: usize,
}

impl Code {
    #[must_use]
    pub fn cross_owner_checks(&self) -> Vec<CrossOwnerCheck> {
        let functions = self.functions.iter().map(|function| {
            let caller = function.path.join("::");
            self.checks_in(&caller, &function.effects, &function.file)
        });
        let methods = self.methods.iter().map(|method| {
            let caller = format!("{}::{}", method.owner, method.name);
            self.checks_in(&caller, &method.effects, &method.file)
        });
        functions.chain(methods).flatten().collect()
    }

    fn checks_in(&self, caller: &str, effects: &Effects, file: &Path) -> Vec<CrossOwnerCheck> {
        effects
            .calls
            .iter()
            .filter_map(|call| {
                let [.., aggregate, method] = call.path.as_slice() else {
                    return None;
                };
                let known = self.aggregates.iter().any(|known| known.name == *aggregate);
                (method == "across" && known).then(|| CrossOwnerCheck {
                    aggregate: aggregate.clone(),
                    caller: caller.to_owned(),
                    file: file.to_path_buf(),
                    line: call.line,
                })
            })
            .collect()
    }
}
