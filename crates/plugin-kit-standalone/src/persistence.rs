use crate::Config;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

const FORMAT: u32 = 1;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFile {
    pub format: u32,
    pub app_id: String,
    pub config: Config,
    pub plugin_state: Vec<u8>,
}

impl SessionFile {
    pub fn new(app_id: String, config: Config, plugin_state: Vec<u8>) -> Self {
        Self {
            format: FORMAT,
            app_id,
            config,
            plugin_state,
        }
    }
}

pub fn load_session(path: &Path, app_id: &str) -> Result<Option<SessionFile>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Session file is too large; original preserved.".into());
    }
    let session: SessionFile = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Session could not be read; original preserved: {e}"))?;
    if session.format != FORMAT || session.app_id != app_id {
        return Err("Session version or application does not match; original preserved.".into());
    }
    Ok(Some(session))
}

/// Same-directory temporary file, sync, then atomic replacement. Existing files
/// survive serialization/write errors. Call only from a control thread.
pub fn save_session(path: &Path, session: &SessionFile) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Session file needs a parent directory")?;
    let bytes = serde_json::to_vec(session).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Session exceeds the size limit.".into());
    }
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_interrupted_replacement_preserve_last_good_session() {
        let dir = tempfile::tempdir().expect("temporary dir");
        let path = dir.path().join("session.json");
        let session = SessionFile::new("test".into(), Config::initial(), vec![1, 2, 3]);
        assert_eq!(load_session(&path, "test").expect("missing"), None);
        save_session(&path, &session).expect("save");
        assert_eq!(
            load_session(&path, "test").expect("read"),
            Some(session.clone())
        );
        let mut partial = tempfile::NamedTempFile::new_in(dir.path()).expect("temp");
        partial.write_all(b"truncated").expect("partial");
        drop(partial);
        assert_eq!(
            load_session(&path, "test").expect("original"),
            Some(session)
        );
    }
    #[test]
    fn malformed_future_and_wrong_app_files_are_preserved() {
        let dir = tempfile::tempdir().expect("temporary dir");
        let path = dir.path().join("session.json");
        for bytes in [b"{truncated".as_slice(), b"{}".as_slice()] {
            std::fs::write(&path, bytes).expect("write");
            assert!(load_session(&path, "test").is_err());
            assert_eq!(std::fs::read(&path).expect("original"), bytes);
        }
        let mut session = SessionFile::new("test".into(), Config::initial(), vec![]);
        session.format = 99;
        save_session(&path, &session).expect("future save");
        assert!(load_session(&path, "test").is_err());
        session.format = 1;
        save_session(&path, &session).expect("save");
        assert!(load_session(&path, "other").is_err());
    }
    #[test]
    fn competing_atomic_writes_always_leave_one_complete_session() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("session.json");
        std::thread::scope(|scope| {
            for writer in 0..4 {
                let path = path.clone();
                scope.spawn(move || {
                    save_session(
                        &path,
                        &SessionFile::new("test".into(), Config::initial(), vec![writer; 4096]),
                    )
                    .expect("atomic write");
                });
            }
        });
        let session = load_session(&path, "test").expect("read").expect("session");
        assert_eq!(session.plugin_state.len(), 4096);
        assert!(
            session
                .plugin_state
                .iter()
                .all(|v| *v == session.plugin_state[0])
        );
    }
}
