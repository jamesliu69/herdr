use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::warn;

use super::snapshot::{
    parse_history_snapshot, parse_snapshot, snapshot_file_version, SessionHistorySnapshot,
    SessionSnapshot, SNAPSHOT_VERSION,
};

pub(super) fn session_path() -> PathBuf {
    crate::session::data_dir().join("session.json")
}

fn session_history_path() -> PathBuf {
    crate::session::data_dir().join("session-history.json")
}

/// Sibling of `target` that holds the previous complete session file.
fn backup_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
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
    save_json_to_path(path, snapshot, true)
}

fn save_json_to_path<T: serde::Serialize>(
    path: &Path,
    snapshot: &T,
    rotate_backup: bool,
) -> std::io::Result<()> {
    let target = resolve_write_target(path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(snapshot)?;
    write_json_durably(&target, json.as_bytes(), rotate_backup)
}

/// Publishes `contents` to `target` through a synced temp file and an atomic
/// replacement. With `rotate_backup`, the previous complete file is copied to
/// `<target>.bak` first so a failed replacement or later damage to the primary
/// file still leaves the last complete session recoverable.
fn write_json_durably(target: &Path, contents: &[u8], rotate_backup: bool) -> std::io::Result<()> {
    let tmp_path = target.with_extension("json.tmp");
    let result = write_and_sync(&tmp_path, contents).and_then(|()| {
        if rotate_backup {
            if let Err(err) = rotate_backup_file(target) {
                tracing::warn!(
                    event = "persist.backup", subsystem = "persist", outcome = "error",
                    path = %target.display(), err = %err,
                    "failed to keep the previous session copy; saving without it"
                );
            }
        }
        crate::platform::replace_file(&tmp_path, target)
    });
    if let Err(err) = result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }
    crate::platform::sync_parent_directory(target.parent().unwrap_or_else(|| Path::new(".")))
}

fn write_and_sync(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut temporary = std::fs::File::create(path)?;
    // A rename can survive a hard crash before unflushed file contents reach disk.
    temporary.write_all(contents)?;
    temporary.sync_all()
}

/// Keeps the current session file as `<target>.bak`. Only a file that still
/// parses is kept, so a damaged primary cannot replace a good backup.
fn rotate_backup_file(target: &Path) -> std::io::Result<()> {
    let content = match std::fs::read(target) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let Ok(text) = std::str::from_utf8(&content) else {
        return Ok(());
    };
    if parse_snapshot(text).is_err() {
        return Ok(());
    }
    let backup = backup_path(target);
    let mut pending_name = backup.clone().into_os_string();
    pending_name.push(".pending");
    let pending = PathBuf::from(pending_name);
    let result = write_and_sync(&pending, &content)
        .and_then(|()| crate::platform::replace_file(&pending, &backup));
    if let Err(err) = result {
        let _ = std::fs::remove_file(&pending);
        return Err(err);
    }
    Ok(())
}

pub(super) fn save_history_to_path(
    path: &Path,
    history: Option<&SessionHistorySnapshot>,
) -> std::io::Result<()> {
    match history {
        // Pane history is optional and often large; the session layout alone is
        // what needs a recovery copy.
        Some(history) => save_json_to_path(path, history, false),
        None => clear_path(path),
    }
}

pub(super) fn clear_path(path: &Path) -> std::io::Result<()> {
    let target = resolve_write_target(path).unwrap_or_else(|_| path.to_path_buf());
    let removed = remove_file_if_exists(path)? | remove_file_if_exists(&backup_path(&target))?;
    if removed {
        crate::platform::sync_parent_directory(target.parent().unwrap_or_else(|| Path::new(".")))?;
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn clear_history() {
    let path = session_history_path();
    if let Err(err) = clear_path(&path) {
        crate::logging::session_clear_failed(&path, &err.to_string());
    }
}

pub fn load() -> Option<SessionSnapshot> {
    load_from_path(&session_path())
}

fn load_from_path(path: &Path) -> Option<SessionSnapshot> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(
                event = "persist.restore", subsystem = "persist", outcome = "missing",
                path = %path.display(), "session file is missing"
            );
            return recover_from_backup(path, "missing");
        }
        Err(err) => {
            warn!(
                event = "persist.restore", subsystem = "persist", outcome = "read_error",
                path = %path.display(), err = %err, "failed to read session file"
            );
            return None;
        }
    };
    match parse_snapshot(&content) {
        Ok(snapshot) => Some(snapshot),
        Err(err) => {
            if let Some(version) = snapshot_file_version(&content) {
                if version > SNAPSHOT_VERSION {
                    warn!(
                        event = "persist.restore", subsystem = "persist", outcome = "unsupported_version",
                        path = %path.display(), file_version = version, supported = SNAPSHOT_VERSION,
                        "session file is from a newer herdr version, ignoring"
                    );
                    return recover_from_backup(path, "unsupported_version");
                }
            }
            warn!(
                event = "persist.restore", subsystem = "persist", outcome = "parse_error",
                path = %path.display(), err = %err, "failed to parse session file, ignoring"
            );
            recover_from_backup(path, "parse_error")
        }
    }
}

/// Falls back to the previous complete session kept by `rotate_backup_file`
/// when the primary file is missing or unusable, then repairs the primary path
/// so later saves rotate the recovered layout instead of the damaged file.
fn recover_from_backup(path: &Path, reason: &str) -> Option<SessionSnapshot> {
    let target = resolve_write_target(path).ok()?;
    let backup = backup_path(&target);
    let content = match std::fs::read_to_string(&backup) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(
                event = "persist.restore", subsystem = "persist", outcome = "no_backup",
                path = %path.display(), backup = %backup.display(), reason,
                "no session backup available"
            );
            return None;
        }
        Err(err) => {
            warn!(
                event = "persist.restore", subsystem = "persist", outcome = "backup_read_error",
                path = %backup.display(), err = %err, reason, "failed to read session backup"
            );
            return None;
        }
    };
    match parse_snapshot(&content) {
        Ok(snapshot) => {
            warn!(
                event = "persist.restore", subsystem = "persist", outcome = "recovered",
                path = %path.display(), backup = %backup.display(), reason,
                "session file was unusable; recovered the previous saved session"
            );
            match serde_json::to_string_pretty(&snapshot) {
                Ok(json) => {
                    if let Err(err) = write_json_durably(&target, json.as_bytes(), false) {
                        warn!(
                            event = "persist.restore", subsystem = "persist", outcome = "repair_error",
                            path = %target.display(), err = %err,
                            "failed to repair session file from backup"
                        );
                    }
                }
                Err(err) => {
                    warn!(
                        event = "persist.restore", subsystem = "persist", outcome = "repair_error",
                        path = %target.display(), err = %err, reason,
                        "failed to serialize recovered session"
                    );
                }
            }
            Some(snapshot)
        }
        Err(err) => {
            warn!(
                event = "persist.restore", subsystem = "persist", outcome = "backup_parse_error",
                path = %backup.display(), err = %err, reason,
                "failed to parse session backup"
            );
            None
        }
    }
}

pub fn load_history() -> Option<SessionHistorySnapshot> {
    let path = session_history_path();
    if !path.exists() {
        return None;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
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

    fn snapshot_with_selected(selected: usize) -> SessionSnapshot {
        let mut snapshot = empty_snapshot();
        snapshot.selected = selected;
        snapshot
    }

    fn read_selected(path: &Path) -> usize {
        parse_snapshot(&std::fs::read_to_string(path).unwrap())
            .unwrap()
            .selected
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
    fn save_keeps_the_previous_complete_session_in_the_backup() {
        let path = temp_session_path("rotate-backup");
        let backup = backup_path(&path);

        save_to_path(&path, &snapshot_with_selected(1)).unwrap();
        assert!(!backup.exists(), "first save has no previous session");
        save_to_path(&path, &snapshot_with_selected(2)).unwrap();

        assert_eq!(read_selected(&path), 2);
        assert_eq!(read_selected(&backup), 1);
        assert!(!path.with_extension("json.tmp").exists());
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            2
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_session_recovers_from_backup() {
        let path = temp_session_path("missing-backup");
        save_to_path(&path, &snapshot_with_selected(1)).unwrap();
        save_to_path(&path, &snapshot_with_selected(2)).unwrap();
        std::fs::remove_file(&path).unwrap();

        let recovered = load_from_path(&path).expect("backup should be restored");
        assert_eq!(recovered.selected, 1);
        assert_eq!(read_selected(&path), 1, "the primary file is repaired");
        assert_eq!(read_selected(&backup_path(&path)), 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_session_recovers_from_backup() {
        let path = temp_session_path("corrupt-backup");
        save_to_path(&path, &snapshot_with_selected(1)).unwrap();
        save_to_path(&path, &snapshot_with_selected(2)).unwrap();
        std::fs::write(&path, b"\0\0\0\0").unwrap();

        let recovered = load_from_path(&path).expect("backup should be restored");
        assert_eq!(recovered.selected, 1);
        assert_eq!(read_selected(&path), 1, "the primary file is repaired");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_session_without_backup_starts_empty() {
        let path = temp_session_path("corrupt-no-backup");
        save_to_path(&path, &snapshot_with_selected(1)).unwrap();
        std::fs::write(&path, b"not json").unwrap();

        assert!(load_from_path(&path).is_none());
        assert!(!backup_path(&path).exists());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_damaged_primary_does_not_replace_a_good_backup() {
        let path = temp_session_path("damaged-primary");
        save_to_path(&path, &snapshot_with_selected(1)).unwrap();
        save_to_path(&path, &snapshot_with_selected(2)).unwrap();
        std::fs::write(&path, b"not json").unwrap();

        save_to_path(&path, &snapshot_with_selected(3)).unwrap();

        assert_eq!(read_selected(&path), 3);
        assert_eq!(
            read_selected(&backup_path(&path)),
            1,
            "the last complete session stays recoverable"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_temp_write_keeps_the_previous_session_and_backup() {
        let path = temp_session_path("failed-temp");
        save_to_path(&path, &snapshot_with_selected(1)).unwrap();
        save_to_path(&path, &snapshot_with_selected(2)).unwrap();
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();

        assert!(save_to_path(&path, &snapshot_with_selected(3)).is_err());

        assert_eq!(read_selected(&path), 2);
        assert_eq!(read_selected(&backup_path(&path)), 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn clear_removes_session_and_its_backup() {
        let path = temp_session_path("clear-backup");
        save_to_path(&path, &snapshot_with_selected(1)).unwrap();
        save_to_path(&path, &snapshot_with_selected(2)).unwrap();

        clear_path(&path).unwrap();

        assert!(!path.exists());
        assert!(!backup_path(&path).exists());
        assert!(load_from_path(&path).is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn history_saves_do_not_rotate_a_backup() {
        let (session_path, history_path) = temp_session_paths("history-no-backup");
        save_history_to_path(&history_path, Some(&history_snapshot("first"))).unwrap();
        save_history_to_path(&history_path, Some(&history_snapshot("second"))).unwrap();

        assert!(!backup_path(&history_path).exists());
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
