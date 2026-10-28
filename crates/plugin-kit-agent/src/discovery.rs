//! Where the instruments that allow agents can be found: each writes a record of itself (its
//! process, kind, version and socket) into a folder of the user's own while it listens, and
//! removes it as it stops; the bridge reads the folder, and drops a record whose instrument has
//! gone without removing it (its socket answers no more).
//!
//! The folder is `$INSTRUMENT_AGENT_DIR` if set (the tests'), else `instrument` in the user's runtime
//! folder (`$XDG_RUNTIME_DIR`, which only the user may enter), else `instrument-<user>` in the
//! temporary folder (on macOS the user's own). It is made readable by the user alone, and the
//! sockets in it too: only the user's own programs reach a instrument (decisions.md R-MCP).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A instrument listening for agents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Its id for agents: `<kind>-<pid>` (a plug-in in a host will add which one).
    pub id: String,
    pub pid: u32,
    /// `standalone` (later, a plug-in's format: `clap`, `vst3`, `au`).
    pub kind: String,
    pub version: String,
    pub protocol: u64,
    /// The socket's path.
    pub endpoint: String,
    /// When it began listening (seconds since 1970).
    pub started: u64,
}

/// Safe basename for a record/socket; never a path.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

/// The folder records and sockets are in.
pub fn dir(socket_name: &str, override_env: &str) -> PathBuf {
    if let Some(d) = std::env::var_os(override_env) {
        return PathBuf::from(d);
    }
    if let Some(r) = std::env::var_os("XDG_RUNTIME_DIR").filter(|r| !r.is_empty()) {
        return PathBuf::from(r).join(socket_name);
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into());
    let user: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    std::env::temp_dir().join(format!("{socket_name}-{user}"))
}

/// The folder made, for the user alone: refused if someone else may enter it and it cannot
/// be made the user's alone.
pub fn ensure(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if !meta.is_dir() {
            return Err(format!("{} is not a folder", dir.display()));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
                format!(
                    "{} may be entered by other users and cannot be made private: {e}",
                    dir.display()
                )
            })?;
        }
        Ok(())
    }
    // (On Windows the folder is in the user's own profile, which others may not enter.)
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
    }
}

pub fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

pub fn socket_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.sock"))
}

/// The record written (whole, by a rename, so that a reader never sees half of it).
pub fn publish(dir: &Path, r: &Record) -> Result<(), String> {
    if !valid_id(&r.id) {
        return Err("invalid instance id".into());
    }
    let path = record_path(dir, &r.id);
    let tmp = dir.join(format!(".{}.json.tmp", r.id));
    let text = serde_json::to_string_pretty(r).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every record in the folder, oldest first (none if there is no folder).
pub fn records(dir: &Path) -> Vec<Record> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut list: Vec<Record> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "json")
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .filter_map(|p| {
            use std::io::Read;
            let mut text = String::new();
            std::fs::File::open(p)
                .ok()?
                .take(65537)
                .read_to_string(&mut text)
                .ok()?;
            (text.len() <= 65536).then_some(text)
        })
        .filter_map(|t| serde_json::from_str::<Record>(&t).ok())
        .filter(|r| valid_id(&r.id) && Path::new(&r.endpoint) == socket_path(dir, &r.id))
        .collect();
    list.sort_by(|a: &Record, b: &Record| (a.started, &a.id).cmp(&(b.started, &b.id)));
    list
}

/// The records of the instruments that answer: one whose socket refuses (it has gone) is removed
/// with its socket.
pub fn live(dir: &Path) -> Vec<Record> {
    records(dir)
        .into_iter()
        .filter(|r| {
            let alive = reachable(Path::new(&r.endpoint));
            if !alive {
                let _ = std::fs::remove_file(record_path(dir, &r.id));
                // Never unlink a path supplied by an untrusted discovery record.
            }
            alive
        })
        .collect()
}

#[cfg(any(unix, windows))]
fn reachable(path: &Path) -> bool {
    super::Stream::connect(path).is_ok()
}

#[cfg(not(any(unix, windows)))]
fn reachable(_path: &Path) -> bool {
    false
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    fn record(id: &str, started: u64, endpoint: &Path) -> Record {
        Record {
            id: id.into(),
            pid: 1,
            kind: "standalone".into(),
            version: "0".into(),
            protocol: super::super::PROTOCOL,
            endpoint: endpoint.to_string_lossy().into_owned(),
            started,
        }
    }

    /// Records are read oldest first; one whose socket does not answer is removed.
    #[test]
    fn records_are_read_and_the_dead_removed() {
        let t = tempfile::tempdir().expect("tempdir");
        let dir = t.path().join("agents");
        ensure(&dir).expect("ensure");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        let live_sock = socket_path(&dir, "standalone-2");
        let _listener = crate::Listener::bind(&live_sock).expect("bind");
        publish(&dir, &record("standalone-2", 20, &live_sock)).expect("publish");
        publish(
            &dir,
            &record("standalone-1", 10, &socket_path(&dir, "standalone-1")),
        )
        .expect("publish");
        std::fs::write(dir.join("junk.json"), "{").expect("junk");
        let all: Vec<String> = records(&dir).into_iter().map(|r| r.id).collect();
        assert_eq!(all, ["standalone-1", "standalone-2"]);
        let alive: Vec<String> = live(&dir).into_iter().map(|r| r.id).collect();
        assert_eq!(alive, ["standalone-2"]);
        assert!(!record_path(&dir, "standalone-1").exists());
    }

    /// A folder others may enter is made the user's alone.
    #[cfg(unix)]
    #[test]
    fn an_open_folder_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().expect("tempdir");
        let dir = t.path().join("open");
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).expect("chmod");
        ensure(&dir).expect("ensure");
        let mode = std::fs::metadata(&dir).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
