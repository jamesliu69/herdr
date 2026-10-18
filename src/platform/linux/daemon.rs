//! Detached server startup for Linux.
//!
//! SSH logins run inside a systemd `session-N.scope`. `setsid` gives the
//! daemon its own terminal session, but not its own cgroup: with
//! `KillUserProcesses=yes` logind kills every process in the session scope
//! when the SSH session ends, so the server and every pane die with it
//! (herdr#5170). Starting the daemon through the user systemd manager places
//! it under `user@.service` instead, which user linger keeps alive after the
//! last session closes.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// One-shot launch marker consumed by `prepare_daemon_process`.
pub(super) const SERVER_DAEMON_ENV: &str = "HERDR_SERVER_DAEMON";

/// A failed `systemd-run` normally exits immediately. Waiting briefly lets the
/// launch fall back to the inherited session instead of leaving attach to time
/// out on a server that never started.
const SCOPE_PROBE: Duration = Duration::from_millis(200);

/// Marks a daemon command before it is spawned. The marker is applied by the
/// server process itself so both launch paths end with the same session.
pub(crate) fn configure_server_daemon_command(command: &mut Command) {
    // A handoff replacement must keep the source server's context.
    if command.get_args().any(|arg| arg == "--handoff-import") {
        command.env_remove(SERVER_DAEMON_ENV);
    } else {
        command.env(SERVER_DAEMON_ENV, "1");
    }
}

/// Called after exec, before logging, worker threads, or pane creation.
/// Returns whether the server now runs in its own session.
pub(crate) fn prepare_daemon_process(handoff_import: bool) -> io::Result<bool> {
    let requested = std::env::var_os(SERVER_DAEMON_ENV).is_some();
    // One-shot launch request, never environment for panes or plugins.
    std::env::remove_var(SERVER_DAEMON_ENV);
    if !daemon_needs_new_session(requested, handoff_import, process_is_session_leader()) {
        return Ok(requested && !handoff_import);
    }
    // SAFETY: called before the server starts any threads.
    if unsafe { libc::setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(true)
}

fn daemon_needs_new_session(
    requested: bool,
    handoff_import: bool,
    already_session_leader: bool,
) -> bool {
    requested && !handoff_import && !already_session_leader
}

fn process_is_session_leader() -> bool {
    // SAFETY: getsid and getpid have no preconditions.
    unsafe { libc::getsid(0) == libc::getpid() }
}

/// Starts the server outside the launcher's systemd session scope when the
/// user manager is reachable, and directly otherwise.
pub(crate) fn launch_server_daemon(command: &mut Command) -> io::Result<u32> {
    if let Some(runtime_dir) = systemd_user_runtime_dir() {
        if let Ok(mut scoped) = scoped_command(command, &runtime_dir).spawn() {
            // `systemd-run --scope` stays alive until the daemon exits, so a
            // prompt exit means the scope unit was never created.
            std::thread::sleep(SCOPE_PROBE);
            if matches!(scoped.try_wait(), Ok(None)) {
                return Ok(scoped.id());
            }
        }
    }
    command.spawn().map(|child| child.id())
}

fn scoped_command(command: &Command, runtime_dir: &PathBuf) -> Command {
    let mut scoped = Command::new("systemd-run");
    scoped
        .arg("--user")
        .arg("--scope")
        .arg("--quiet")
        .arg("--")
        .arg(command.get_program());
    scoped.args(command.get_args());
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                scoped.env(key, value);
            }
            None => {
                scoped.env_remove(key);
            }
        }
    }
    if let Some(dir) = command.get_current_dir() {
        scoped.current_dir(dir);
    }
    // The daemon is placed in a new cgroup, but its runtime directory must not
    // fall back to a value that only the launcher's session had.
    scoped.env("XDG_RUNTIME_DIR", runtime_dir);
    scoped
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    scoped
}

fn systemd_user_runtime_dir() -> Option<PathBuf> {
    systemd_user_runtime_dir_from(
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        std::env::var_os("PATH"),
    )
}

fn systemd_user_runtime_dir_from(
    runtime_dir: Option<PathBuf>,
    path: Option<OsString>,
) -> Option<PathBuf> {
    let runtime_dir = runtime_dir
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            // SAFETY: getuid has no preconditions.
            let uid = unsafe { libc::getuid() };
            PathBuf::from(format!("/run/user/{uid}"))
        });
    if runtime_dir.join("systemd/private").exists()
        && program_in_path(path.as_deref(), "systemd-run")
    {
        Some(runtime_dir)
    } else {
        None
    }
}

fn program_in_path(path: Option<&std::ffi::OsStr>, program: &str) -> bool {
    path.map(|path| std::env::split_paths(path).any(|directory| directory.join(program).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "herdr-daemon-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn daemon_commands_are_marked_and_handoff_commands_are_not() {
        let mut daemon = Command::new("herdr");
        daemon.args(["server"]);
        configure_server_daemon_command(&mut daemon);
        assert!(daemon.get_envs().any(|(key, value)| {
            key == OsStr::new(SERVER_DAEMON_ENV) && value == Some(OsStr::new("1"))
        }));

        let mut handoff = Command::new("herdr");
        handoff.args(["server", "--handoff-import", "socket", "token"]);
        handoff.env(SERVER_DAEMON_ENV, "1");
        configure_server_daemon_command(&mut handoff);
        assert!(handoff
            .get_envs()
            .any(|(key, value)| key == OsStr::new(SERVER_DAEMON_ENV) && value.is_none()));
    }

    #[test]
    fn scoped_command_preserves_program_arguments_environment_and_directory() {
        let mut command = Command::new("/usr/local/bin/herdr");
        command.args(["server", "--flag"]);
        command.env("HERDR_SESSION", "work");
        command.env_remove("HERDR_SOCKET_PATH");
        command.current_dir("/tmp/work");

        let scoped = scoped_command(&command, &PathBuf::from("/run/user/1000"));

        assert_eq!(scoped.get_program(), OsStr::new("systemd-run"));
        assert_eq!(
            scoped.get_args().collect::<Vec<_>>(),
            [
                "--user",
                "--scope",
                "--quiet",
                "--",
                "/usr/local/bin/herdr",
                "server",
                "--flag",
            ]
        );
        let envs: Vec<_> = scoped.get_envs().collect();
        assert!(envs.contains(&(OsStr::new("HERDR_SESSION"), Some(OsStr::new("work")))));
        assert!(envs.contains(&(
            OsStr::new("XDG_RUNTIME_DIR"),
            Some(OsStr::new("/run/user/1000"))
        )));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == OsStr::new("HERDR_SOCKET_PATH") && value.is_none()));
        assert_eq!(scoped.get_current_dir(), Some(Path::new("/tmp/work")));
    }

    #[test]
    fn new_session_is_only_requested_for_marked_non_handoff_processes() {
        assert!(daemon_needs_new_session(true, false, false));
        assert!(!daemon_needs_new_session(true, false, true));
        assert!(!daemon_needs_new_session(true, true, false));
        assert!(!daemon_needs_new_session(false, false, false));
    }

    #[test]
    fn user_manager_detection_requires_runtime_socket_and_program() {
        let dir = temp_dir("detection");
        let program_dir = temp_dir("program");
        let path = OsString::from(program_dir.as_os_str());

        assert_eq!(
            systemd_user_runtime_dir_from(Some(dir.clone()), Some(path.clone())),
            None,
            "missing runtime socket"
        );

        std::fs::create_dir_all(dir.join("systemd")).unwrap();
        std::fs::write(dir.join("systemd/private"), b"").unwrap();
        assert_eq!(
            systemd_user_runtime_dir_from(Some(dir.clone()), Some(path.clone())),
            None,
            "missing systemd-run"
        );

        std::fs::write(program_dir.join("systemd-run"), b"").unwrap();
        assert_eq!(
            systemd_user_runtime_dir_from(Some(dir.clone()), Some(path)),
            Some(dir.clone())
        );
        assert_eq!(
            systemd_user_runtime_dir_from(Some(PathBuf::from("relative")), None),
            None,
            "relative runtime directories cannot be trusted"
        );

        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(program_dir).unwrap();
    }
}
