use std::collections::BTreeMap;
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// Process-wide, as the interrupt is: Ctrl-C or SIGTERM stops every child sofie started,
/// which a SIGTERM sent to sofie alone would otherwise leave running.
static LIVE: Registry = Registry(Mutex::new(BTreeMap::new()));

/// Waiting polls, so a waiting thread never holds the lock an interrupt needs.
const WAIT_POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessGroup {
    /// sofie's, so a child run in the terminal gets its Ctrl-C as it would run by hand.
    Inherited,
    /// The child's own, so stopping it stops what it started too, such as cargo's test
    /// binaries. Not on Windows, where stopping it stops the child alone.
    Own,
}

struct Running {
    child: Child,
    group: ProcessGroup,
}

type Live = BTreeMap<u32, Running>;

#[derive(Default)]
struct Registry(Mutex<Live>);

impl Registry {
    /// Spawned under the lock, so an interrupt handled meanwhile cannot miss it.
    fn spawn(
        &'static self,
        group: ProcessGroup,
        command: &mut Command,
        spawn: impl FnOnce(&mut Command) -> io::Result<Child>,
    ) -> io::Result<ChildProcess> {
        #[cfg(unix)]
        if group == ProcessGroup::Own {
            std::os::unix::process::CommandExt::process_group(command, 0);
        }
        let mut live = self.lock();
        let child = spawn(command)?;
        let pid = child.id();
        live.insert(pid, Running { child, group });
        Ok(ChildProcess {
            pid,
            registry: self,
            status: None,
        })
    }

    fn reap(&self, pid: u32) -> io::Result<Option<ExitStatus>> {
        let mut live = self.lock();
        let running = live
            .get_mut(&pid)
            .ok_or_else(|| io::Error::other(format!("child process {pid} is no longer tracked")))?;
        let status = running.child.try_wait()?;
        if status.is_some() {
            live.remove(&pid);
        }
        Ok(status)
    }

    /// Holding the returned lock until the process exits keeps anything from spawning
    /// another child.
    fn stop_all_for_exit(&self) -> MutexGuard<'_, Live> {
        let mut live = self.lock();
        for running in live.values_mut() {
            kill(running);
        }
        live.clear();
        live
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A child process in the registry; dropping it while it runs kills it.
pub(crate) struct ChildProcess {
    pid: u32,
    registry: &'static Registry,
    /// Once reaped, its pid may name another process, which nothing here may signal.
    status: Option<ExitStatus>,
}

impl ChildProcess {
    pub(crate) fn spawn(
        group: ProcessGroup,
        command: &mut Command,
        spawn: impl FnOnce(&mut Command) -> io::Result<Child>,
    ) -> io::Result<Self> {
        LIVE.spawn(group, command, spawn)
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn exited(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self.registry.reap(self.pid)?;
        }
        Ok(self.status)
    }

    pub(crate) fn wait(mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.exited()? {
                return Ok(status);
            }
            std::thread::sleep(WAIT_POLL);
        }
    }

    pub(crate) fn kill(mut self) -> Option<ExitStatus> {
        self.stop()
    }

    /// Under the lock, so an interrupt arriving meanwhile still finds the child.
    fn stop(&mut self) -> Option<ExitStatus> {
        if self.status.is_none() {
            let mut live = self.registry.lock();
            self.status = kill(&mut live.remove(&self.pid)?);
        }
        self.status
    }
}

#[cfg(all(test, unix))]
impl ChildProcess {
    pub(crate) fn untracked() -> Self {
        let registry: &'static Registry = Box::leak(Box::default());
        let child = registry
            .spawn(
                ProcessGroup::Inherited,
                &mut Command::new("true"),
                Command::spawn,
            )
            .expect("`true` starts");
        drop(registry.stop_all_for_exit());
        child
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `Command::output`, with the child in the registry.
pub(crate) fn output(group: ProcessGroup, command: &mut Command) -> io::Result<Output> {
    LIVE.output(group, command)
}

impl Registry {
    fn output(&'static self, group: ProcessGroup, command: &mut Command) -> io::Result<Output> {
        let mut stdout = None;
        let mut stderr = None;
        let process = self.spawn(group, command, |command| {
            let mut child = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            stdout = child.stdout.take();
            stderr = child.stderr.take();
            Ok(child)
        })?;
        // Read beside stdout, so neither pipe fills while the other is read.
        let stderr = std::thread::spawn(move || read_all(stderr));
        let stdout = read_all(stdout)?;
        let stderr = stderr
            .join()
            .map_err(|_| io::Error::other("reading a child process's stderr panicked"))??;
        Ok(Output {
            status: process.wait()?,
            stdout,
            stderr,
        })
    }
}

fn read_all(pipe: Option<impl Read>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

/// Killed rather than stopped: a development app's held reload requests would keep a
/// graceful shutdown waiting, and nothing is lost. Killing fails only for a process that
/// already exited, which `wait` then reaps.
fn kill(running: &mut Running) -> Option<ExitStatus> {
    if running.group == ProcessGroup::Own {
        kill_group(running.child.id());
    }
    let _ = running.child.kill();
    running.child.wait().ok()
}

/// std signals a single process, and libc's `killpg` would need `unsafe`. The leader is
/// not reaped yet, so its pid still names the group and no other.
#[cfg(unix)]
fn kill_group(leader: u32) {
    let killed = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{leader}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(error) = killed {
        eprintln!("sofie: stopping process group {leader} failed: {error}");
    }
}

#[cfg(not(unix))]
fn kill_group(_leader: u32) {}

/// Ctrl-C, the SIGTERM an agent stops `sofie mcp` with, or a closed terminal's SIGHUP stops
/// and reaps every child and exits with 128 plus the signal's number, as a shell reports a
/// process the signal ended. A SIGHUP ignored at start, as under `nohup`, stays ignored for
/// sofie and its children. `process::exit` flushes stdout. Listening again does nothing.
pub(crate) fn stop_on_interrupt() -> io::Result<()> {
    static LISTENING: AtomicBool = AtomicBool::new(false);
    if LISTENING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut signals = {
        let _context = runtime.enter();
        Signals::listen()?
    };
    std::thread::Builder::new()
        .name("sofie-interrupt".to_owned())
        .spawn(move || {
            let code = runtime.block_on(signals.next());
            let _held = LIVE.stop_all_for_exit();
            std::process::exit(code);
        })?;
    Ok(())
}

#[cfg(unix)]
struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: Option<tokio::signal::unix::Signal>,
}

#[cfg(unix)]
impl Signals {
    /// tokio's handler replaces an inherited ignore, so a SIGHUP ignored at start is left alone.
    fn listen() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};

        let hangup = if hangup_is_ignored()? {
            None
        } else {
            Some(signal(SignalKind::hangup())?)
        };
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup,
        })
    }

    async fn next(&mut self) -> i32 {
        std::future::poll_fn(|context| {
            if self.interrupt.poll_recv(context).is_ready() {
                return std::task::Poll::Ready(130);
            }
            if self.terminate.poll_recv(context).is_ready() {
                return std::task::Poll::Ready(143);
            }
            match &mut self.hangup {
                Some(hangup) => hangup.poll_recv(context).map(|_| 129),
                None => std::task::Poll::Pending,
            }
        })
        .await
    }
}

/// Reading the disposition takes `sigaction`, which needs `unsafe`, and macOS's `ps` does not
/// show it. A child inherits an ignored signal across exec, so a shell that hangs itself up
/// survives exactly when sofie ignores SIGHUP. Asked before any handler is installed, since
/// a handled signal reaches a child as the default.
#[cfg(unix)]
fn hangup_is_ignored() -> io::Result<bool> {
    use std::os::unix::process::ExitStatusExt;

    let probe = Command::new("sh")
        .args(["-c", "kill -s HUP $$"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    match (probe.code(), probe.signal()) {
        (Some(0), _) => Ok(true),
        (_, Some(1)) => Ok(false),
        _ => Err(io::Error::other(format!(
            "telling whether SIGHUP is ignored failed: the probing shell ended with {probe}"
        ))),
    }
}

#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    fn listen() -> io::Result<Self> {
        Ok(Self)
    }

    async fn next(&mut self) -> i32 {
        match tokio::signal::ctrl_c().await {
            Ok(()) => 130,
            Err(error) => {
                eprintln!("sofie: listening for Ctrl-C failed: {error}");
                std::future::pending().await
            }
        }
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::{ChildProcess, ProcessGroup, Registry, Running, Signals};

    fn sleeping(registry: &'static Registry) -> ChildProcess {
        registry
            .spawn(
                ProcessGroup::Inherited,
                Command::new("sleep").arg("60"),
                Command::spawn,
            )
            .unwrap()
    }

    fn is_running(pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn stopping_for_exit_reaps_every_child_and_keeps_another_from_starting() {
        let registry: &'static Registry = Box::leak(Box::default());
        let children = [sleeping(registry), sleeping(registry)];

        let held = registry.stop_all_for_exit();

        assert!(held.is_empty());
        drop(held);
        assert!(children.iter().all(|child| !is_running(child.pid())));
    }

    #[test]
    fn a_dropped_child_is_killed_and_a_waited_one_leaves_the_registry() {
        let registry: &'static Registry = Box::leak(Box::default());
        let dropped = sleeping(registry).pid();

        let finished = registry
            .output(ProcessGroup::Own, Command::new("echo").arg("done"))
            .unwrap();

        assert!(!is_running(dropped));
        assert!(registry.lock().is_empty());
        assert_eq!(finished.stdout, b"done\n");
        assert!(finished.status.success());
    }

    #[test]
    fn a_reaped_child_dropped_later_leaves_a_child_reusing_its_pid_running() {
        let registry: &'static Registry = Box::leak(Box::default());
        let mut finished = registry
            .spawn(
                ProcessGroup::Inherited,
                &mut Command::new("true"),
                Command::spawn,
            )
            .unwrap();
        while finished.exited().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let reusing = Command::new("sleep").arg("60").spawn().unwrap();
        let reusing_pid = reusing.id();
        registry.lock().insert(
            finished.pid(),
            Running {
                child: reusing,
                group: ProcessGroup::Inherited,
            },
        );

        drop(finished);

        assert!(is_running(reusing_pid));
        drop(registry.stop_all_for_exit());
    }

    #[test]
    fn stopping_a_child_in_its_own_group_stops_what_it_started() {
        let registry: &'static Registry = Box::leak(Box::default());
        let mut stdout = None;
        let child = registry
            .spawn(
                ProcessGroup::Own,
                Command::new("sh").args(["-c", "sleep 60 & echo started; wait"]),
                |command| {
                    let mut child = command.stdout(Stdio::piped()).spawn()?;
                    stdout = child.stdout.take();
                    Ok(child)
                },
            )
            .unwrap();
        let mut stdout = BufReader::new(stdout.unwrap());
        stdout.read_line(&mut String::new()).unwrap();
        let killed = Instant::now();

        child.kill();

        // The backgrounded sleep holds the pipe open until it stops too.
        stdout.read_to_end(&mut Vec::new()).unwrap();
        assert!(killed.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn each_signal_is_told_apart_by_its_exit_code() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for (signal, code) in [("-TERM", 143), ("-INT", 130), ("-HUP", 129)] {
            let mut signals = {
                let _context = runtime.enter();
                Signals::listen().unwrap()
            };
            let sent = Command::new("kill")
                .args([signal, &std::process::id().to_string()])
                .status()
                .unwrap();
            assert!(sent.success());

            let received = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), signals.next()).await
            });

            assert_eq!(received, Ok(code), "{signal}");
        }
    }
}
