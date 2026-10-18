//! PTY spawning that never lets a frozen working directory stall the server.
//!
//! `portable-pty` resolves the configured working directory synchronously: it
//! stats the path and the forked child `chdir`s into it before `exec`. When
//! that path lives on an unresponsive network mount those calls can block for
//! minutes, so every spawn runs on a helper thread with a bounded wait. The
//! request loop fails the pane with an error instead of freezing every API
//! request until the mount answers again.

#[cfg(unix)]
mod unix;

#[cfg(unix)]
use unix::spawn_pty_uncached;
#[cfg(unix)]
pub(crate) use unix::SpawnedPty;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder};

#[cfg(windows)]
use portable_pty::{native_pty_system, MasterPty, PtySize};

/// How long the server waits for a pane process to start before failing the
/// request. A healthy spawn takes milliseconds; the wait only matters when the
/// child's working directory is on a stalled mount.
pub(crate) const PTY_SPAWN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a working directory that timed out a spawn stays marked
/// unresponsive. Repeated requests for the same directory fail immediately
/// instead of waiting on the mount again.
const UNRESPONSIVE_CWD_TTL: Duration = Duration::from_secs(30);

/// Upper bound on remembered unresponsive directories so a pathological client
/// cannot grow the cache without limit.
const UNRESPONSIVE_CWD_CAPACITY: usize = 64;

/// Cap on concurrently starting pane processes. Stuck spawns keep their helper
/// thread until the mount recovers, so refuse new spawns once the backlog is
/// clearly abnormal.
const MAX_IN_FLIGHT_PTY_SPAWNS: usize = 32;

static PTY_SPAWNS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
pub(crate) struct SpawnedPty {
    pub master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

#[cfg(windows)]
fn spawn_pty_uncached(rows: u16, cols: u16, cmd: CommandBuilder) -> std::io::Result<SpawnedPty> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| std::io::Error::other(err.to_string()))?;

    Ok(SpawnedPty {
        master: pair.master,
        child,
    })
}

/// Spawns the pane process on a helper thread and waits at most
/// [`PTY_SPAWN_TIMEOUT`] for it. See the module docs for why this cannot run
/// inline.
pub(crate) fn spawn_with_portable_pty(
    rows: u16,
    cols: u16,
    cmd: CommandBuilder,
) -> io::Result<SpawnedPty> {
    let cwd = cmd.get_cwd().map(|cwd| PathBuf::from(cwd.as_os_str()));
    let timeout = pty_spawn_timeout(cwd.as_deref());
    run_bounded_pty_spawn(cwd.as_deref(), timeout, move || {
        let mut cmd = cmd;
        if let Some(cwd) = cmd.get_cwd().map(PathBuf::from) {
            // Windows resolves on-disk component casing here; on Unix this is
            // the identity. Resolve it on the worker so a frozen drive cannot
            // block the request loop.
            cmd.cwd(crate::platform::normalize_cwd_for_launch(&cwd));
        }
        spawn_pty_uncached(rows, cols, cmd).map(KillOnDropSpawn::new)
    })
    .map(KillOnDropSpawn::take)
}

/// Kills the pane process if the spawn result is abandoned because the caller
/// already timed out. Without this, a late-finishing spawn would leave an
/// unreaped child behind.
struct KillOnDropSpawn(Option<SpawnedPty>);

impl KillOnDropSpawn {
    fn new(spawned: SpawnedPty) -> Self {
        Self(Some(spawned))
    }

    fn take(mut self) -> SpawnedPty {
        self.0.take().expect("spawned pty present")
    }
}

impl Drop for KillOnDropSpawn {
    fn drop(&mut self) {
        if let Some(mut spawned) = self.0.take() {
            let _ = spawned.child.kill();
            let _ = spawned.child.wait();
        }
    }
}

/// Checks whether `path` is a directory without letting an unresponsive
/// network mount stall the caller past the PTY spawn timeout. A timeout is
/// reported as an error; callers treat both `Ok(false)` and `Err(_)` as
/// unavailable.
pub(crate) fn directory_is_available(path: &Path) -> io::Result<bool> {
    let path = path.to_path_buf();
    let probe = path.clone();
    let timeout = pty_spawn_timeout(Some(&path));
    run_bounded_pty_spawn(Some(&path), timeout, move || Ok(probe.is_dir()))
}

fn pty_spawn_timeout(_cwd: Option<&Path>) -> Duration {
    #[cfg(test)]
    if let Some(timeout) = test_hook::spawn_timeout_for(_cwd) {
        return timeout;
    }
    PTY_SPAWN_TIMEOUT
}

fn run_bounded_pty_spawn<T, F>(cwd: Option<&Path>, timeout: Duration, work: F) -> io::Result<T>
where
    F: FnOnce() -> io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    if let Some(cwd) = cwd {
        if cwd_is_marked_unresponsive(cwd) {
            return Err(unresponsive_cwd_error(cwd));
        }
    }

    let in_flight = InFlightSpawn::acquire()?;
    #[cfg(test)]
    let hook_cwd = cwd.map(Path::to_path_buf);
    let result =
        crate::thread_spawn::spawn_blocking_with_timeout("herdr-pty-spawn", timeout, move || {
            let _in_flight = in_flight;
            #[cfg(test)]
            test_hook::maybe_wait_for_spawn(hook_cwd.as_deref());
            work()
        });

    match &result {
        Ok(Ok(_)) => {
            if let Some(cwd) = cwd {
                clear_unresponsive_cwd(cwd);
            }
        }
        Err(err) if err.kind() == io::ErrorKind::TimedOut => {
            if let Some(cwd) = cwd {
                mark_unresponsive_cwd(cwd);
            }
        }
        _ => {}
    }

    result.and_then(|inner| inner)
}

fn unresponsive_cwd_error(cwd: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "pane working directory {} is not responding; refusing to start the pane",
            cwd.display()
        ),
    )
}

fn unresponsive_cwds() -> &'static Mutex<HashMap<PathBuf, Instant>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Instant>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_unresponsive_cwds() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Instant>> {
    unresponsive_cwds()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn cwd_is_marked_unresponsive(cwd: &Path) -> bool {
    let now = Instant::now();
    let mut cache = lock_unresponsive_cwds();
    cache.retain(|_, marked_at| now.saturating_duration_since(*marked_at) < UNRESPONSIVE_CWD_TTL);
    cache.contains_key(cwd)
}

fn mark_unresponsive_cwd(cwd: &Path) {
    mark_unresponsive_cwd_at(cwd, Instant::now());
}

fn mark_unresponsive_cwd_at(cwd: &Path, marked_at: Instant) {
    let mut cache = lock_unresponsive_cwds();
    if cache.len() >= UNRESPONSIVE_CWD_CAPACITY && !cache.contains_key(cwd) {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, marked_at)| **marked_at)
            .map(|(path, _)| path.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(cwd.to_path_buf(), marked_at);
}

fn clear_unresponsive_cwd(cwd: &Path) {
    lock_unresponsive_cwds().remove(cwd);
}

#[derive(Debug)]
struct InFlightSpawn;

impl InFlightSpawn {
    fn acquire() -> io::Result<Self> {
        let in_flight = PTY_SPAWNS_IN_FLIGHT.fetch_add(1, Ordering::AcqRel) + 1;
        if in_flight > MAX_IN_FLIGHT_PTY_SPAWNS {
            PTY_SPAWNS_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "too many pane processes are still starting; try again",
            ));
        }
        Ok(Self)
    }
}

impl Drop for InFlightSpawn {
    fn drop(&mut self) {
        PTY_SPAWNS_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
pub(crate) mod test_hook {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::Mutex;
    use std::time::Duration;

    static BLOCKED_SPAWN: Mutex<Option<(PathBuf, Receiver<()>)>> = Mutex::new(None);
    static SPAWN_TIMEOUT: Mutex<Option<(PathBuf, Duration)>> = Mutex::new(None);

    /// Makes the next PTY spawn for `cwd` wait until the returned sender is
    /// used or dropped.
    pub(crate) fn block_next_spawn_for_cwd(cwd: &Path) -> Sender<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        *BLOCKED_SPAWN.lock().expect("pty spawn block hook") = Some((cwd.to_path_buf(), rx));
        tx
    }

    pub(super) fn maybe_wait_for_spawn(cwd: Option<&Path>) {
        let receiver = {
            let mut guard = BLOCKED_SPAWN.lock().expect("pty spawn block hook");
            let matches = guard
                .as_ref()
                .is_some_and(|(blocked, _)| Some(blocked.as_path()) == cwd);
            if matches {
                guard.take().map(|(_, rx)| rx)
            } else {
                None
            }
        };
        if let Some(rx) = receiver {
            let _ = rx.recv();
        }
    }

    pub(crate) fn set_spawn_timeout_for_cwd(cwd: &Path, timeout: Duration) {
        *SPAWN_TIMEOUT.lock().expect("pty spawn timeout hook") = Some((cwd.to_path_buf(), timeout));
    }

    pub(crate) fn clear_spawn_timeout() {
        *SPAWN_TIMEOUT.lock().expect("pty spawn timeout hook") = None;
    }

    pub(super) fn spawn_timeout_for(cwd: Option<&Path>) -> Option<Duration> {
        let guard = SPAWN_TIMEOUT.lock().expect("pty spawn timeout hook");
        guard
            .as_ref()
            .and_then(|(blocked, timeout)| (Some(blocked.as_path()) == cwd).then_some(*timeout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn unique_cwd(label: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "herdr-pty-cwd-{label}-{}-{stamp}",
            std::process::id()
        ))
    }

    /// Reads the cache without pruning expired entries so tests can observe
    /// that a successful spawn removed the marker.
    fn cwd_entry_present(cwd: &Path) -> bool {
        lock_unresponsive_cwds().contains_key(cwd)
    }

    #[test]
    fn blocking_spawn_times_out_and_marks_cwd_unresponsive() {
        let _guard = TEST_LOCK.lock().expect("pty backend test lock");
        let cwd = unique_cwd("timeout");
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let caller = std::thread::current().id();

        let err =
            run_bounded_pty_spawn::<u32, _>(Some(&cwd), Duration::from_millis(20), move || {
                assert_ne!(std::thread::current().id(), caller);
                let _ = release_rx.recv();
                Ok(7)
            })
            .expect_err("blocking pty spawn should time out");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(cwd_is_marked_unresponsive(&cwd));

        // The next request for the same directory fails without starting work.
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_started = std::sync::Arc::clone(&started);
        let err = run_bounded_pty_spawn::<u32, _>(Some(&cwd), Duration::from_secs(5), move || {
            worker_started.store(true, Ordering::Release);
            Ok(7)
        })
        .expect_err("unresponsive cwd should fail before spawning");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(!started.load(Ordering::Acquire));

        release_tx.send(()).expect("release blocked spawn worker");
        clear_unresponsive_cwd(&cwd);
    }

    #[test]
    fn successful_spawn_clears_unresponsive_marker() {
        let _guard = TEST_LOCK.lock().expect("pty backend test lock");
        let cwd = unique_cwd("recover");
        let expired = Instant::now()
            .checked_sub(UNRESPONSIVE_CWD_TTL + Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        mark_unresponsive_cwd_at(&cwd, expired);
        assert!(cwd_entry_present(&cwd));

        let result = run_bounded_pty_spawn::<u32, _>(Some(&cwd), Duration::from_secs(5), || Ok(3))
            .expect("healthy spawn should succeed");
        assert_eq!(result, 3);
        assert!(!cwd_entry_present(&cwd));
    }

    #[test]
    fn unresponsive_marker_expires_after_ttl() {
        let _guard = TEST_LOCK.lock().expect("pty backend test lock");
        let cwd = unique_cwd("expiry");
        let expired = Instant::now()
            .checked_sub(UNRESPONSIVE_CWD_TTL + Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        mark_unresponsive_cwd_at(&cwd, expired);
        assert!(!cwd_is_marked_unresponsive(&cwd));
    }

    #[test]
    fn spawn_failure_propagates_to_the_caller() {
        let _guard = TEST_LOCK.lock().expect("pty backend test lock");
        let cwd = unique_cwd("failure");
        let err = run_bounded_pty_spawn::<u32, _>(Some(&cwd), Duration::from_secs(5), || {
            Err(io::Error::new(io::ErrorKind::NotFound, "no shell"))
        })
        .expect_err("inner spawn error should surface");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!cwd_is_marked_unresponsive(&cwd));
    }

    #[test]
    fn directory_check_reports_present_and_missing_paths() {
        let _guard = TEST_LOCK.lock().expect("pty backend test lock");
        let present = std::env::temp_dir();
        assert!(directory_is_available(&present).expect("temp dir probe"));
        let missing = unique_cwd("missing");
        assert!(!directory_is_available(&missing).expect("missing dir probe"));
    }

    #[test]
    fn in_flight_spawn_cap_is_bounded() {
        let _guard = TEST_LOCK.lock().expect("pty backend test lock");
        let slots: Vec<InFlightSpawn> = (0..MAX_IN_FLIGHT_PTY_SPAWNS)
            .map(|_| InFlightSpawn::acquire().expect("spawn slot"))
            .collect();
        let err = InFlightSpawn::acquire().expect_err("cap should reject the extra spawn");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        drop(slots);
        InFlightSpawn::acquire().expect("slot after release");
    }
}
