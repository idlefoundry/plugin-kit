//! Computer settings beside the shared preset library, from the CA-72's R48, R51
//! and R54 and its later per-key transactions, also used by the CA-74.
//!
//! A caller interprets its own keys and supplies its defaults. [`Settings::change`]
//! reads again under a file lock, changes only the chosen keys and replaces a whole
//! file. Unknown keys survive, including those in newer formats. Unreadable files are left alone.
//! These functions do file I/O and belong off the audio thread.

mod toml;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub use toml::{encode, encode_preserving, parse, raw_section};
pub use toml_edit::{Array, Item, Table, TableLike, Value, value};

pub const FORMAT: i64 = 1;

/// The shared root. `IDLE_FOUNDRY_SHARED` replaces the complete Idle Foundry root.
pub fn shared_dir() -> PathBuf {
    root_for(std::env::consts::OS, |key| std::env::var_os(key))
}

fn root_for(os: &str, get: impl Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
    if let Some(root) = get("IDLE_FOUNDRY_SHARED") {
        return PathBuf::from(root);
    }
    let home = get("HOME").map(PathBuf::from);
    match os {
        "windows" => {
            if let Some(appdata) = get("APPDATA") {
                return PathBuf::from(appdata).join("Idle Foundry");
            }
        }
        "macos" => {
            if let Some(home) = home {
                return home.join("Library/Application Support/Idle Foundry");
            }
        }
        _ => {
            if let Some(data) = get("XDG_DATA_HOME") {
                return PathBuf::from(data).join("Idle Foundry");
            }
            if let Some(home) = home {
                return home.join(".local/share/Idle Foundry");
            }
        }
    }
    PathBuf::from("idle-foundry-shared")
}

/// A plug-in's folder, with a single portable directory name supplied by the caller.
pub fn plugin_dir(name: &str) -> Result<PathBuf, String> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_uppercase();
    let device = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|n| {
                let mut characters = n.chars();
                characters
                    .next()
                    .is_some_and(|c| c.is_ascii_digit() || "¹²³".contains(c))
                    && characters.next().is_none()
            })
        });
    if name.is_empty()
        || device
        || name.starts_with(['.', ' '])
        || name.ends_with(['.', ' '])
        || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(char::is_control)
    {
        return Err("A plug-in's directory needs one portable name".into());
    }
    Ok(shared_dir().join(name))
}

/// A complete replacement in the destination directory, with a unique temporary
/// file and its contents synced before replacement. An error before replacement
/// leaves the original file in place. The parent directory must be writable.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = parent(path);
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    let mut temporary = builder
        .tempfile_in(parent)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    temporary
        .write_all(text.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    // Drop the returned handle before another writer replaces the file on Windows.
    drop(temporary.persist(path).map_err(|e| e.to_string())?);
    Ok(())
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Take an operating-system file lock until the returned handle is dropped. The
/// lock file stays in place so other processes always lock the same file.
pub fn lock(path: &Path) -> Result<File, String> {
    std::fs::create_dir_all(parent(path)).map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.lock()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(file)
}

/// Read a table, distinguishing a missing file from a file which cannot be read.
pub fn read(path: &Path) -> Result<Option<Table>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}; left as it is", path.display())),
    }
}

/// Accept a missing format for old files, and refuse a format this version cannot
/// safely rewrite. A wrongly typed format is also an error.
pub fn check_format(table: &Table) -> Result<(), String> {
    if table
        .get("format")
        .is_some_and(|f| f.as_integer() != Some(FORMAT))
    {
        return Err("An unknown file format; left as it is".into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Settings {
    path: PathBuf,
}

impl Settings {
    pub fn shared(name: &str) -> Result<Self, String> {
        Ok(Self::at(plugin_dir(name)?.join("settings.toml")))
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// No file is `None`. An unreadable file is an error; the caller may display
    /// that error and use its defaults without losing the file on the next edit.
    pub fn read(&self) -> Result<Option<Table>, String> {
        read(&self.path)
    }

    /// Read through the caller's schema parser, including future-format keys.
    pub fn read_with<T>(
        &self,
        parse: impl FnOnce(&str) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => parse(&text)
                .map(Some)
                .map_err(|e| format!("{}: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", self.path.display())),
        }
    }

    /// Re-read under the settings lock, then let the caller parse, change and encode its schema.
    /// `None` abandons a transaction without writing (for a claim already taken, for example).
    pub fn change_with<T>(
        &self,
        change: impl FnOnce(Option<&str>) -> Result<(T, Option<String>), String>,
    ) -> Result<T, String> {
        let _lock = lock(&self.path.with_extension("toml.lock"))?;
        let source = self.read_with(|text| Ok(text.to_owned()))?;
        let (result, text) = change(source.as_deref())?;
        if let Some(text) = text {
            parse(&text)?;
            write_atomic(&self.path, &text)?;
        }
        Ok(result)
    }

    /// Re-read and change chosen keys under the same lock in every instance and
    /// process. Return an error from the closure to abandon the entire change.
    pub fn change(
        &self,
        change: impl FnOnce(&mut Table) -> Result<(), String>,
    ) -> Result<(), String> {
        let _lock = lock(&self.path.with_extension("toml.lock"))?;
        let mut table = self.read()?.unwrap_or_default();
        change(&mut table)?;
        table.insert("format", value(FORMAT));
        let text = encode(&table)?;
        write_atomic(&self.path, &text)
    }
}

#[cfg(test)]
mod tests;
