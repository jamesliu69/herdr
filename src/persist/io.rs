use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::warn;

use super::snapshot::{
    parse_history_snapshot, parse_snapshot, snapshot_file_version, SessionHistorySnapshot,
    SessionSnapshot, SNAPSHOT_VERSION,
};

pub(super) fn session_path() -> PathBuf {
    crate::session::persistence_dir().join("session.json")
}

fn session_history_path() -> PathBuf {
    crate::session::persistence_dir().join("session-history.json")
}

/// Pre-upgrade location of this instance's session file.
///
/// A server started with `HERDR_SOCKET_PATH` used to keep its state in the
/// session data directory. That directory still holds the default session's
/// state, so it is only read as a migration fallback and never written.
fn legacy_session_path() -> Option<PathBuf> {
    if !instance_has_no_state() {
        return None;
    }
    let path = crate::session::data_dir().join("session.json");
    (path != session_path()).then_some(path)
}

fn legacy_session_history_path() -> Option<PathBuf> {
    if !instance_has_no_state() {
        return None;
    }
    let path = crate::session::data_dir().join("session-history.json");
    (path != session_history_path()).then_some(path)
}

/// True while this instance has never written its own session state.
///
/// A missing `session.json` alone is not enough: clearing the last workspace
/// deliberately removes it, and that must not resurrect the shared pre-upgrade
/// file. The instance directory exists as soon as the writer saves or clears.
fn instance_has_no_state() -> bool {
    !crate::session::persistence_dir().exists()
}

// Follow symlinks manually so a write through a (possibly dangling) symlink
// lands on the target. `fs::canonicalize` requires the target to exist, which
// excludes the dangling-symlink case stow users hit on the very first save.
fn resolve_write_target(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..16 {
        let meta = match std::fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            Err(_) => return Ok(current),
        };
        if !meta.file_type().is_symlink() {
            return Ok(current);
        }
        let link = std::fs::read_link(&current)?;
        current = if link.is_absolute() {
            link
        } else {
            current
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(link)
        };
    }
    Ok(current)
}

pub(super) fn save_to_path(path: &Path, snapshot: &SessionSnapshot) -> std::io::Result<()> {
    save_json_to_path(path, snapshot)
}

fn save_json_to_path<T: serde::Serialize>(path: &Path, snapshot: &T) -> std::io::Result<()> {
    let target = resolve_write_target(path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(snapshot)?;
    let tmp_path = target.with_extension("json.tmp");
    let mut temporary = std::fs::File::create(&tmp_path)?;
    // A rename can survive a hard crash before unflushed file contents reach disk.
    let result = temporary
        .write_all(json.as_bytes())
        .and_then(|()| temporary.sync_all());
    drop(temporary);
    if let Err(err) = result.and_then(|()| crate::platform::replace_file(&tmp_path, &target)) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }
    crate::platform::sync_parent_directory(target.parent().unwrap_or_else(|| Path::new(".")))
}

pub(super) fn save_history_to_path(
    path: &Path,
    history: Option<&SessionHistorySnapshot>,
) -> std::io::Result<()> {
    match history {
        Some(history) => save_json_to_path(path, history),
        None => clear_path(path),
    }
}

pub(super) fn clear_path(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn clear_history() {
    let path = session_history_path();
    if let Err(err) = clear_path(&path) {
        crate::logging::session_clear_failed(&path, &err.to_string());
    }
}

enum SnapshotRead {
    Missing,
    Found(SessionSnapshot),
    Failed,
}

fn read_snapshot(path: &Path) -> SnapshotRead {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return SnapshotRead::Missing,
        Err(err) => {
            warn!(
                event = "persist.restore", subsystem = "persist", outcome = "read_error",
                path = %path.display(), err = %err, "failed to read session file"
            );
            return SnapshotRead::Failed;
        }
    };
    match parse_snapshot(&content) {
        Ok(snapshot) => SnapshotRead::Found(snapshot),
        Err(err) => {
            if let Some(version) = snapshot_file_version(&content) {
                if version > SNAPSHOT_VERSION {
                    warn!(
                        event = "persist.restore", subsystem = "persist", outcome = "unsupported_version",
                        path = %path.display(), file_version = version, supported = SNAPSHOT_VERSION,
                        "session file is from a newer herdr version, ignoring"
                    );
                    return SnapshotRead::Failed;
                }
            }
            warn!(
                event = "persist.restore", subsystem = "persist", outcome = "parse_error",
                path = %path.display(), err = %err, "failed to parse session file, ignoring"
            );
            SnapshotRead::Failed
        }
    }
}

pub fn load() -> Option<SessionSnapshot> {
    let path = session_path();
    match read_snapshot(&path) {
        SnapshotRead::Found(snapshot) => Some(snapshot),
        // An instance file that exists but cannot be read is authoritative:
        // never resurrect another session's state over it.
        SnapshotRead::Failed => None,
        SnapshotRead::Missing => {
            tracing::info!(
                event = "persist.restore", subsystem = "persist", outcome = "missing",
                path = %path.display(), "session file is missing"
            );
            let legacy = legacy_session_path()?;
            match read_snapshot(&legacy) {
                SnapshotRead::Found(snapshot) => {
                    warn!(
                        event = "persist.restore", subsystem = "persist", outcome = "legacy_fallback",
                        path = %legacy.display(), session_path = %path.display(),
                        "no per-instance session file yet; restored the shared pre-upgrade session file. Future saves use the per-instance path."
                    );
                    Some(snapshot)
                }
                SnapshotRead::Missing | SnapshotRead::Failed => None,
            }
        }
    }
}

fn read_history(path: &Path) -> Option<SessionHistorySnapshot> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            warn!(err = %err, "failed to read session history file");
            return None;
        }
    };
    match parse_history_snapshot(&content) {
        Ok(snapshot) => Some(snapshot),
        Err(err) => {
            if let Some(version) = snapshot_file_version(&content) {
                if version > SNAPSHOT_VERSION {
                    warn!(
                        file_version = version,
                        supported = SNAPSHOT_VERSION,
                        "session history file is from a newer herdr version, ignoring"
                    );
                    return None;
                }
            }
            warn!(err = %err, "failed to parse session history file, ignoring");
            None
        }
    }
}

pub fn load_history() -> Option<SessionHistorySnapshot> {
    if let Some(history) = read_history(&session_history_path()) {
        return Some(history);
    }
    // Adopt the pre-upgrade history only together with the pre-upgrade session
    // snapshot, never for an instance that has its own state.
    legacy_session_history_path().and_then(|path| read_history(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::snapshot::{
        PaneHistorySnapshot, TabHistorySnapshot, WorkspaceHistorySnapshot,
    };

    fn temp_session_path(name: &str) -> PathBuf {
        let unique = format!(
            "herdr-session-tests-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique).join("session.json")
    }

    fn temp_session_paths(name: &str) -> (PathBuf, PathBuf) {
        let session = temp_session_path(name);
        let history = session.with_file_name("session-history.json");
        (session, history)
    }

    fn empty_snapshot() -> SessionSnapshot {
        SessionSnapshot {
            version: SNAPSHOT_VERSION,
            workspaces: vec![],
            active: None,
            selected: 0,
            sidebar_width: Some(26),
            sidebar_section_split: Some(0.5),
            collapsed_space_keys: std::collections::HashSet::new(),
        }
    }

    fn history_snapshot(secret: &str) -> SessionHistorySnapshot {
        SessionHistorySnapshot {
            version: SNAPSHOT_VERSION,
            layout_fingerprint: None,
            workspaces: vec![WorkspaceHistorySnapshot {
                tabs: vec![TabHistorySnapshot {
                    panes: std::collections::HashMap::from([(
                        0,
                        PaneHistorySnapshot {
                            ansi: secret.to_string(),
                            lines: 1,
                        },
                    )]),
                }],
            }],
        }
    }

    fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    fn with_isolated_config<R>(name: &str, run: impl FnOnce() -> R) -> R {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let config_home =
            std::env::temp_dir().join(format!("herdr-session-io-{name}-{}", std::process::id()));
        let previous_config_home = std::env::var_os("XDG_CONFIG_HOME");
        let previous_socket = std::env::var_os(crate::api::SOCKET_PATH_ENV_VAR);
        let previous_session = std::env::var_os(crate::session::SESSION_ENV_VAR);
        std::env::set_var("XDG_CONFIG_HOME", &config_home);
        std::env::remove_var(crate::api::SOCKET_PATH_ENV_VAR);
        std::env::remove_var(crate::session::SESSION_ENV_VAR);
        crate::session::clear_explicit_session_for_test();

        let result = run();

        restore_env("XDG_CONFIG_HOME", previous_config_home);
        restore_env(crate::api::SOCKET_PATH_ENV_VAR, previous_socket);
        restore_env(crate::session::SESSION_ENV_VAR, previous_session);
        crate::session::clear_explicit_session_for_test();
        let _ = std::fs::remove_dir_all(&config_home);
        result
    }

    #[test]
    fn socket_overrides_do_not_share_one_session_file() {
        with_isolated_config("socket-instances", || {
            std::env::set_var(
                crate::api::SOCKET_PATH_ENV_VAR,
                "/tmp/herdr-instance-a.sock",
            );
            let path_a = session_path();
            let mut snapshot_a = empty_snapshot();
            snapshot_a.selected = 1;
            save_to_path(&path_a, &snapshot_a).unwrap();

            std::env::set_var(
                crate::api::SOCKET_PATH_ENV_VAR,
                "/tmp/herdr-instance-b.sock",
            );
            let path_b = session_path();
            let mut snapshot_b = empty_snapshot();
            snapshot_b.selected = 2;
            save_to_path(&path_b, &snapshot_b).unwrap();

            assert_ne!(path_a, path_b);
            assert_eq!(load().unwrap().selected, 2);
            assert!(session_path().parent().unwrap().exists());
            assert!(!crate::session::data_dir().join("session.json").exists());

            std::env::set_var(
                crate::api::SOCKET_PATH_ENV_VAR,
                "/tmp/herdr-instance-a.sock",
            );
            assert_eq!(load().unwrap().selected, 1);
        });
    }

    #[test]
    fn load_falls_back_to_pre_upgrade_session_and_never_rewrites_it() {
        with_isolated_config("legacy-fallback", || {
            std::env::set_var(crate::api::SOCKET_PATH_ENV_VAR, "/tmp/herdr-legacy.sock");
            let legacy_path = crate::session::data_dir().join("session.json");
            let instance_path = session_path();
            assert_ne!(legacy_path, instance_path);

            let mut legacy = empty_snapshot();
            legacy.selected = 7;
            save_to_path(&legacy_path, &legacy).unwrap();

            let restored = load().expect("pre-upgrade session should be restored");
            assert_eq!(restored.selected, 7);
            assert!(
                !instance_path.exists(),
                "reading the fallback must not write the instance file"
            );

            let legacy_bytes = std::fs::read(&legacy_path).unwrap();
            let mut instance = empty_snapshot();
            instance.selected = 3;
            save_to_path(&instance_path, &instance).unwrap();
            assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy_bytes);

            let mut newer_legacy = empty_snapshot();
            newer_legacy.selected = 9;
            save_to_path(&legacy_path, &newer_legacy).unwrap();
            assert_eq!(
                load().unwrap().selected,
                3,
                "the instance file must win once it exists"
            );

            // Clearing the last workspace removes the file but keeps the
            // instance directory; that must not resurrect the legacy session.
            clear_path(&instance_path).unwrap();
            assert!(
                load().is_none(),
                "an intentionally cleared instance must stay cleared"
            );
        });
    }

    #[test]
    fn broken_instance_file_does_not_fall_back_to_another_session() {
        with_isolated_config("broken-instance", || {
            std::env::set_var(crate::api::SOCKET_PATH_ENV_VAR, "/tmp/herdr-broken.sock");
            let mut legacy = empty_snapshot();
            legacy.selected = 7;
            save_to_path(&crate::session::data_dir().join("session.json"), &legacy).unwrap();

            let instance_path = session_path();
            std::fs::create_dir_all(instance_path.parent().unwrap()).unwrap();
            std::fs::write(&instance_path, "{ not json").unwrap();

            assert!(load().is_none());
        });
    }

    #[test]
    fn load_history_falls_back_only_with_the_pre_upgrade_snapshot() {
        with_isolated_config("history-fallback", || {
            std::env::set_var(crate::api::SOCKET_PATH_ENV_VAR, "/tmp/herdr-history.sock");
            let legacy_session = crate::session::data_dir().join("session.json");
            let legacy_history = crate::session::data_dir().join("session-history.json");
            save_to_path(&legacy_session, &empty_snapshot()).unwrap();
            save_history_to_path(&legacy_history, Some(&history_snapshot("legacy-secret")))
                .unwrap();

            let history = load_history().expect("pre-upgrade history should be restored");
            assert_eq!(
                history.workspaces[0].tabs[0].panes[&0].ansi,
                "legacy-secret"
            );

            save_to_path(&session_path(), &empty_snapshot()).unwrap();
            assert!(
                load_history().is_none(),
                "another session's history must not leak in"
            );
        });
    }

    #[test]
    fn save_to_paths_writes_pane_history_only_to_history_file() {
        let (session_path, history_path) = temp_session_paths("split-history");

        save_to_path(&session_path, &empty_snapshot()).unwrap();
        save_history_to_path(&history_path, Some(&history_snapshot("split-secret"))).unwrap();

        let session = std::fs::read_to_string(&session_path).unwrap();
        let history = std::fs::read_to_string(&history_path).unwrap();
        assert!(!session.contains("split-secret"));
        assert!(!session.contains("history"));
        assert!(history.contains("split-secret"));
    }

    #[cfg(windows)]
    #[test]
    fn failed_replacement_preserves_saved_files_and_retries() {
        use std::os::windows::fs::OpenOptionsExt;

        let (session_path, history_path) = temp_session_paths("failed-replacement");
        save_to_path(&session_path, &empty_snapshot()).unwrap();
        save_history_to_path(&history_path, Some(&history_snapshot("old history"))).unwrap();

        for path in [&session_path, &history_path] {
            let original = std::fs::read(path).unwrap();
            let locked = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
                .open(path)
                .unwrap();
            let mut changed = empty_snapshot();
            changed.selected = 7;
            let save = || {
                if path == &session_path {
                    save_to_path(path, &changed)
                } else {
                    save_history_to_path(path, Some(&history_snapshot("new history")))
                }
            };
            assert!(save().is_err());
            assert_eq!(std::fs::read(path).unwrap(), original);
            assert!(!path.with_extension("json.tmp").exists());

            drop(locked);
            save().unwrap();
            assert!(!path.with_extension("json.tmp").exists());
        }

        assert_eq!(
            parse_snapshot(&std::fs::read_to_string(&session_path).unwrap())
                .unwrap()
                .selected,
            7
        );
        let history = std::fs::read_to_string(&history_path).unwrap();
        assert!(history.contains("new history"));
        assert!(!history.contains("old history"));
        std::fs::remove_dir_all(session_path.parent().unwrap()).unwrap();
    }

    #[test]
    fn save_to_paths_removes_stale_history_when_history_is_disabled() {
        let (session_path, history_path) = temp_session_paths("clear-history");
        save_to_path(&session_path, &empty_snapshot()).unwrap();
        save_history_to_path(&history_path, Some(&history_snapshot("stale-secret"))).unwrap();

        save_history_to_path(&history_path, None).unwrap();

        assert!(session_path.exists());
        assert!(!history_path.exists());
    }

    #[test]
    fn clear_path_removes_existing_session_file() {
        let path = temp_session_path("clear-existing");
        save_to_path(&path, &empty_snapshot()).unwrap();

        clear_path(&path).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn clear_path_ignores_missing_session_file() {
        let path = temp_session_path("clear-missing");

        clear_path(&path).unwrap();

        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_to_path_preserves_existing_symlink() {
        let target = temp_session_path("symlink-target");
        let link = target.with_file_name("link.json");
        save_to_path(&target, &empty_snapshot()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut snap = empty_snapshot();
        snap.selected = 7;
        save_to_path(&link, &snap).unwrap();

        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let parsed = parse_snapshot(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(parsed.selected, 7);
    }

    #[cfg(unix)]
    #[test]
    fn save_to_path_writes_through_dangling_symlink() {
        let target = temp_session_path("dangling-target");
        let link = target.with_file_name("link.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        save_to_path(&link, &empty_snapshot()).unwrap();

        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_to_path_resolves_relative_symlink() {
        let session = temp_session_path("relative-symlink");
        let dir = session.parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();
        let target = dir.join("real.json");
        let link = dir.join("link.json");
        std::os::unix::fs::symlink("real.json", &link).unwrap();

        save_to_path(&link, &empty_snapshot()).unwrap();

        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(target.exists());
    }
}
