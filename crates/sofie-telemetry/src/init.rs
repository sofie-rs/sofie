use std::env::{self, VarError};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::context::{Exporter, LogExport};
use crate::filter::LogFilter;
use crate::line::LineFormat;
use crate::sample::{LOG_SAMPLE_VARIABLE, Sample};
use crate::subscriber::{Output, Telemetry};
use crate::writer::LogWriter;

#[derive(Debug)]
pub struct InitError(String);

impl InitError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for InitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InitError {}

pub(crate) fn variable_or(name: &str, unset: &str) -> Result<String, InitError> {
    match env::var(name) {
        Ok(value) => Ok(value),
        Err(VarError::NotPresent) => Ok(unset.to_owned()),
        Err(error) => Err(InitError::new(format!("reading {name}: {error}"))),
    }
}

/// Later calls, and a process that installed its own subscriber, keep the one in place.
pub fn init() -> Result<(), InitError> {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) || tracing::dispatcher::has_been_set() {
        return Ok(());
    }
    #[cfg(not(feature = "otlp"))]
    let (exporter, log_export): (_, Option<LogExport>) = (Exporter, None);
    #[cfg(feature = "otlp")]
    let (exporter, log_export) = (Exporter::exporting()?, LogExport::exporting()?);
    let output = Output::Lines {
        filter: LogFilter::from_environment()?,
        sample: Sample::from_environment(LOG_SAMPLE_VARIABLE)?,
        writer: LogWriter::start(LineFormat::of_build(), std::io::stdout(), log_export)
            .map_err(|error| InitError::new(format!("starting the log writer: {error}")))?,
    };
    tracing::subscriber::set_global_default(Telemetry::new(output, exporter))
        .map_err(|error| InitError::new(format!("installing the subscriber: {error}")))?;
    #[cfg(feature = "metrics")]
    crate::metrics::start()?;
    Ok(())
}
