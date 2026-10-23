use std::num::NonZeroUsize;
use std::path::Path;

use super::ToolOutput;
use crate::fuzz::{FuzzOptions, fuzz};
use crate::gen_command::GenError;
use crate::seed_command::seed;

pub(super) fn fuzz_app(root: &Path, seed: Option<u64>, requests: Option<u64>) -> ToolOutput {
    let requests = requests.map(|count| usize::try_from(count).unwrap_or(usize::MAX));
    if requests == Some(0) {
        let refused = GenError::Usage("`requests` must be at least 1".to_owned());
        return ToolOutput::from_error(root, &refused);
    }
    let requests = requests.and_then(NonZeroUsize::new);
    match fuzz(root, FuzzOptions::new(seed, requests)) {
        Ok(report) => ToolOutput::judged(report.to_string(), report.findings.is_empty()),
        Err(error) => ToolOutput::from_error(root, &error),
    }
}

pub(super) fn seed_app(root: &Path, volume: Option<u64>) -> ToolOutput {
    let volume = volume.map_or(0, |count| usize::try_from(count).unwrap_or(usize::MAX));
    match seed(root, volume) {
        Ok(report) => ToolOutput::judged(
            report.to_string(),
            report.routes.iter().all(|route| route.refused == 0),
        ),
        Err(error) => ToolOutput::from_error(root, &error),
    }
}
