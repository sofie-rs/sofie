use std::io::{self, BufWriter, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime};

use tracing::Level;

use crate::context::LogExport;
use crate::fields::{FieldValue, Fields};
use crate::line::{LineFormat, PendingLine};
use crate::outage::{self, Output};
use crate::scrub;

/// At 512 bytes a line, a few megabytes at most for info, and four times that for warnings.
const QUEUE_CAPACITY: usize = 8192;
const SEVERE_CAPACITY: usize = 4 * QUEUE_CAPACITY;
const BUFFER_BYTES: usize = 64 * 1024;
const FLUSH_WAIT: Duration = Duration::from_secs(2);
/// Under load lines keep coming, and a writer that is not waiting costs a sender no wake-up.
const GATHER: Duration = Duration::from_millis(1);
const TARGET: &str = "sofie_telemetry";

enum Entry {
    Line { line: PendingLine, exported: bool },
    Flush(SyncSender<()>),
}

/// The queue never blocks: past capacity, info and quieter lines are dropped and counted, and
/// warnings and errors past a larger bound, dropped errors reported as an error.
pub(crate) struct LogWriter {
    sender: Sender<Entry>,
    queue: Arc<Queue>,
}

#[derive(Default)]
struct Queue {
    waiting: AtomicUsize,
    dropped: AtomicU64,
    dropped_errors: AtomicU64,
}

impl LogWriter {
    pub(crate) fn start(
        format: LineFormat,
        out: impl Write + Send + 'static,
        export: Option<LogExport>,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let queue = Arc::new(Queue::default());
        let writing = Writing {
            format,
            out: BufWriter::with_capacity(BUFFER_BYTES, out),
            queue: Arc::clone(&queue),
            export,
        };
        thread::Builder::new()
            .name("sofie-log".to_owned())
            .spawn(move || writing.run(&receiver))?;
        Ok(Self { sender, queue })
    }

    pub(crate) fn send(&self, line: PendingLine) {
        self.queue(line, true);
    }

    #[cfg(feature = "development")]
    pub(crate) fn send_unexported(&self, line: PendingLine) {
        self.queue(line, false);
    }

    fn queue(&self, line: PendingLine, exported: bool) {
        let waiting = self.queue.waiting.fetch_add(1, Ordering::Relaxed);
        let capacity = if line.level > Level::WARN {
            QUEUE_CAPACITY
        } else {
            SEVERE_CAPACITY
        };
        if waiting >= capacity {
            self.queue.waiting.fetch_sub(1, Ordering::Relaxed);
            let counter = if line.level == Level::ERROR {
                &self.queue.dropped_errors
            } else {
                &self.queue.dropped
            };
            counter.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.sender.send(Entry::Line { line, exported }).is_err() {
            self.queue.waiting.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Bounded, so a stuck stdout cannot hold a stopping process. The log export is left to
    /// its own final export, so a slow collector never stalls the writer.
    pub(crate) fn flush(&self) {
        let (done, written) = mpsc::sync_channel(1);
        if self.sender.send(Entry::Flush(done)).is_ok() {
            let _ = written.recv_timeout(FLUSH_WAIT);
        }
    }
}

fn next_after_a_pause(receiver: &Receiver<Entry>) -> Option<Entry> {
    thread::sleep(GATHER);
    match receiver.try_recv() {
        Ok(entry) => Some(entry),
        Err(TryRecvError::Empty) => receiver.recv().ok(),
        Err(TryRecvError::Disconnected) => None,
    }
}

struct Writing<W: Write> {
    format: LineFormat,
    out: BufWriter<W>,
    queue: Arc<Queue>,
    export: Option<LogExport>,
}

impl<W: Write> Writing<W> {
    fn run(mut self, receiver: &Receiver<Entry>) {
        let mut text = String::new();
        loop {
            let entry = match receiver.try_recv() {
                Ok(entry) => entry,
                Err(TryRecvError::Empty) => {
                    self.report_dropped(&mut text);
                    self.flush();
                    match next_after_a_pause(receiver) {
                        Some(entry) => entry,
                        None => return,
                    }
                }
                Err(TryRecvError::Disconnected) => return self.flush(),
            };
            match entry {
                Entry::Line { mut line, exported } => {
                    self.queue.waiting.fetch_sub(1, Ordering::Relaxed);
                    scrub::scrub(&mut line);
                    self.write(&line, &mut text);
                    if exported {
                        self.export(line);
                    }
                }
                Entry::Flush(done) => {
                    self.report_dropped(&mut text);
                    self.flush();
                    let _ = done.send(());
                }
            }
        }
    }

    fn write(&mut self, line: &PendingLine, text: &mut String) {
        text.clear();
        self.format.render_into(text, line);
        // Logging must not take the process down: readiness reports the failure instead.
        let written = self.out.write_all(text.as_bytes()).is_ok();
        outage::report(Output::Logs, written);
    }

    fn export(&mut self, line: PendingLine) {
        if let Some(export) = &mut self.export {
            export.send(line);
        }
    }

    fn report_dropped(&mut self, text: &mut String) {
        let errors = self.queue.dropped_errors.swap(0, Ordering::Relaxed);
        if errors > 0 {
            let message = "log errors dropped: the writer fell behind";
            self.report(Level::ERROR, message, errors, text);
        }
        let dropped = self.queue.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            let message = "log lines dropped: the writer fell behind";
            self.report(Level::WARN, message, dropped, text);
        }
        let unexported = self.export.as_mut().map_or(0, LogExport::take_dropped);
        if unexported > 0 {
            let message = "log records dropped: the OTLP export fell behind";
            self.report(Level::WARN, message, unexported, text);
        }
    }

    fn report(&mut self, level: Level, message: &str, dropped: u64, text: &mut String) {
        let mut fields = Fields::default();
        fields.set("message", FieldValue::Formatted(message.to_owned()));
        fields.set("dropped", FieldValue::Unsigned(dropped));
        let line = PendingLine {
            level,
            target: TARGET,
            fields,
            scope: Vec::new(),
            trace: None,
            time: SystemTime::now(),
        };
        self.write(&line, text);
        self.export(line);
    }

    fn flush(&mut self) {
        let flushed = self.out.flush().is_ok();
        outage::report(Output::Logs, flushed);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    pub(crate) struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub(crate) fn line(level: Level, message: &str) -> PendingLine {
        let mut fields = Fields::default();
        fields.set("message", FieldValue::Formatted(message.to_owned()));
        PendingLine {
            level,
            target: "test",
            fields,
            scope: Vec::new(),
            trace: None,
            time: SystemTime::UNIX_EPOCH,
        }
    }

    pub(crate) fn written(out: &Shared) -> String {
        String::from_utf8(out.0.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn lines_are_written_in_order_by_the_time_flush_returns() {
        let out = Shared::default();
        let writer = LogWriter::start(LineFormat::Json, out.clone(), None).unwrap();

        writer.send(line(Level::INFO, "first"));
        writer.send(line(Level::ERROR, "second"));
        writer.flush();

        let text = written(&out);
        let first = text.find("first").unwrap();
        assert!(text[first..].contains("second"), "{text}");
        assert_eq!(text.lines().count(), 2, "{text}");
    }

    #[test]
    fn the_writer_scrubs_what_it_writes() {
        let out = Shared::default();
        let writer = LogWriter::start(LineFormat::Json, out.clone(), None).unwrap();

        writer.send(line(Level::ERROR, "signing in with password=hunter2"));
        writer.flush();

        let text = written(&out);
        assert!(text.contains("password=***"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    #[test]
    fn the_writer_masks_emails_and_addresses_in_messages_and_values() {
        let out = Shared::default();
        let writer = LogWriter::start(LineFormat::Json, out.clone(), None).unwrap();
        let mut sent = line(Level::WARN, "no account for jane@example.com from 10.0.0.7");
        sent.fields
            .set("peer", FieldValue::Text("2001:db8::1".to_owned()));

        writer.send(sent);
        writer.flush();

        let text = written(&out);
        assert!(text.contains("no account for *** from ***"), "{text}");
        assert!(text.contains("\"peer\":\"***\""), "{text}");
    }

    #[test]
    fn past_capacity_info_is_dropped_and_counted_but_warnings_are_kept() {
        let out = Shared::default();
        let writer = LogWriter::start(LineFormat::Json, out.clone(), None).unwrap();
        writer
            .queue
            .waiting
            .store(QUEUE_CAPACITY, Ordering::Relaxed);

        writer.send(line(Level::INFO, "dropped"));
        writer.send(line(Level::WARN, "kept"));
        writer
            .queue
            .waiting
            .fetch_sub(QUEUE_CAPACITY, Ordering::Relaxed);
        writer.flush();

        let text = written(&out);
        assert!(!text.contains("\"message\":\"dropped\""), "{text}");
        assert!(text.contains("\"message\":\"kept\""), "{text}");
        assert!(text.contains("\"message\":\"log lines dropped"), "{text}");
        assert!(text.contains("\"dropped\":1"), "{text}");
    }

    #[test]
    fn past_the_hard_bound_errors_are_dropped_and_reported_as_errors() {
        let out = Shared::default();
        let writer = LogWriter::start(LineFormat::Json, out.clone(), None).unwrap();
        writer
            .queue
            .waiting
            .store(SEVERE_CAPACITY, Ordering::Relaxed);

        writer.send(line(Level::ERROR, "lost"));
        writer
            .queue
            .waiting
            .fetch_sub(SEVERE_CAPACITY, Ordering::Relaxed);
        writer.flush();

        let text = written(&out);
        assert!(!text.contains("\"message\":\"lost\""), "{text}");
        assert!(text.contains("\"level\":\"ERROR\""), "{text}");
        assert!(text.contains("\"message\":\"log errors dropped"), "{text}");
    }
}
