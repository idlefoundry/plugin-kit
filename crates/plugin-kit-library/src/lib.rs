//! The shared TOML preset library, from the CA-72's R10 and R18 and the CA-74's
//! diverged library. Factory sounds are supplied by the plug-in and never written.
//! User sounds live in `Idle Foundry/<name>/Presets`, beside `settings.toml` in
//! the plug-in's directory. `IDLE_FOUNDRY_SHARED` overrides the shared root.
//!
//! [`PresetCodec`] keeps parameter types, migrations, tuning laws and extra sound
//! data with the plug-in. [`PlainValues`] provides the common plain numbers by id,
//! with the caller's player controls omitted on save. Hand-written values still
//! read as written. Parameter application and host gestures belong to the caller.
//!
//! Files are replaced whole; library mutations take a directory lock across
//! instances and processes. Bad files are reported and left alone. All file I/O
//! belongs off the audio thread.

mod names;
mod overlay;

use std::ffi::OsString;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub use names::{check_name, parse_tags};
pub use plugin_kit_settings::{Array, Item, Table, TableLike, Value, value};
use plugin_kit_settings::{
    check_format, encode, encode_preserving, lock, parse, plugin_dir, write_atomic,
};

pub const FORMAT: i64 = plugin_kit_settings::FORMAT;

/// A plug-in's sound data. The factory root is supplied as defaults when reading
/// factory sounds, and is empty for user sounds. Metadata belongs to this crate;
/// a codec reads and writes its own keys, including `values` and a writer version.
pub trait PresetCodec: Clone + Debug {
    type Parameters: Clone + Debug;

    fn read(&self, preset: &Table, factory_defaults: &Table) -> Result<Self::Parameters, String>;
    fn write(&self, parameters: &Self::Parameters, preset: &mut Table) -> Result<(), String>;
}

/// Plain parameter values, as in the CA-72's R10. The caller lists the player's
/// controls that its own saves omit; this list does not affect reading a file.
#[derive(Clone, Debug, Default)]
pub struct PlainValues {
    pub kept_on_save: Vec<String>,
}

impl PresetCodec for PlainValues {
    type Parameters = Vec<(String, f64)>;

    fn read(&self, preset: &Table, _: &Table) -> Result<Self::Parameters, String> {
        let Some(values) = preset.get("values") else {
            return Ok(Vec::new());
        };
        let values = values.as_table_like().ok_or("`values` is a table")?;
        values
            .iter()
            .map(|(id, v)| {
                let number = v
                    .as_float()
                    .or_else(|| v.as_integer().map(|n| n as f64))
                    .or_else(|| v.as_bool().map(|b| f64::from(u8::from(b))))
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| format!("`{id}` is not a finite number"))?;
                Ok((id.to_owned(), number))
            })
            .collect()
    }

    fn write(&self, parameters: &Self::Parameters, preset: &mut Table) -> Result<(), String> {
        let mut values = Table::new();
        for (id, number) in parameters {
            if self.kept_on_save.contains(id) {
                continue;
            }
            if !number.is_finite() {
                return Err(format!("`{id}` is not a finite number"));
            }
            let v = if number.fract() == 0.0 && number.abs() < 1e6 {
                value(*number as i64)
            } else {
                value(*number)
            };
            values.insert(id, v);
        }
        preset.insert("values", Item::Table(values));
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Sound<P> {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub parameters: P,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Factory,
    Edited,
    User,
}

#[derive(Clone, Debug)]
pub struct Entry<P> {
    pub sound: Sound<P>,
    pub favorite: bool,
    pub origin: Origin,
    pub replaces: Option<String>,
    file: Option<PathBuf>,
    table: Table,
    source: Option<String>,
}

impl<P> Entry<P> {
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }
}

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn strings(tags: &[String]) -> Item {
    value(tags.iter().map(String::as_str).collect::<Array>())
}

fn sound_of<C: PresetCodec>(
    table: &Table,
    defaults: &Table,
    codec: &C,
) -> Result<Sound<C::Parameters>, String> {
    let name = table
        .get("name")
        .and_then(Item::as_str)
        .ok_or("A preset has no `name`")?;
    check_name(name)?;
    let tags = match table.get("tags") {
        None => Vec::new(),
        Some(tags) => tags
            .as_array()
            .ok_or("`tags` is a list")?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "`tags` holds text only".to_owned())
            })
            .collect::<Result<_, _>>()?,
    };
    Ok(Sound {
        name: name.to_owned(),
        description: table
            .get("description")
            .and_then(Item::as_str)
            .unwrap_or_default()
            .to_owned(),
        tags,
        parameters: codec.read(table, defaults)?,
    })
}

fn factory_entries<C: PresetCodec>(
    text: &str,
    codec: &C,
) -> Result<Vec<Entry<C::Parameters>>, String> {
    let root = parse(text)?;
    if root.get("format").and_then(Item::as_integer) != Some(FORMAT) {
        return Err(format!("Factory presets need format {FORMAT}"));
    }
    let mut entries: Vec<Entry<C::Parameters>> = Vec::new();
    let tables: Vec<Table> = match root.get("preset") {
        None => Vec::new(),
        Some(Item::ArrayOfTables(tables)) => tables.iter().cloned().collect(),
        Some(Item::Value(Value::Array(array))) => array
            .iter()
            .map(|v| {
                v.as_inline_table()
                    .cloned()
                    .map(|t| t.into_table())
                    .ok_or_else(|| "A preset is a table".to_owned())
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("`preset` is a list of tables".into()),
    };
    for table in tables {
        let sound = sound_of(&table, &root, codec)?;
        if entries.iter().any(|e| same(&e.sound.name, &sound.name)) {
            return Err(format!("A second factory preset {}", sound.name));
        }
        entries.push(Entry {
            sound,
            favorite: false,
            origin: Origin::Factory,
            replaces: None,
            file: None,
            table,
            source: None,
        });
    }
    Ok(entries)
}

pub fn parse_factory<C: PresetCodec>(
    text: &str,
    codec: &C,
) -> Result<Vec<Sound<C::Parameters>>, String> {
    Ok(factory_entries(text, codec)?
        .into_iter()
        .map(|e| e.sound)
        .collect())
}

/// A user file read on its own, without searching a directory or applying overlays.
pub fn parse_user<C: PresetCodec>(text: &str, codec: &C) -> Result<Sound<C::Parameters>, String> {
    let table = parse(text)?;
    check_format(&table)?;
    sound_of(&table, &Table::new(), codec)
}

type Seen = Vec<(OsString, u64, Option<SystemTime>)>;
type Read<P> = (Vec<Entry<P>>, Vec<String>);
type Cache<P> = Arc<Mutex<Option<(Seen, Read<P>)>>>;

#[derive(Clone, Debug)]
pub struct Library<C: PresetCodec> {
    dir: PathBuf,
    factory: Arc<Vec<Entry<C::Parameters>>>,
    codec: C,
    last: Cache<C::Parameters>,
}

impl<C: PresetCodec> Library<C> {
    pub fn at(dir: impl Into<PathBuf>, factory: &str, codec: C) -> Result<Self, String> {
        let factory = Arc::new(factory_entries(factory, &codec)?);
        Ok(Self {
            dir: dir.into(),
            factory,
            codec,
            last: Arc::default(),
        })
    }

    pub fn shared(name: &str, factory: &str, codec: C) -> Result<Self, String> {
        Self::at(plugin_dir(name)?.join("Presets"), factory, codec)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn forget(&self) {
        if let Ok(mut last) = self.last.lock() {
            *last = None;
        }
    }

    fn transaction(&self) -> Result<std::fs::File, String> {
        let guard = lock(&self.dir.join(".library.lock"))?;
        self.forget();
        Ok(guard)
    }

    fn files(&self) -> Result<Vec<PathBuf>, String> {
        let directory = match std::fs::read_dir(&self.dir) {
            Ok(directory) => directory,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut files = Vec::new();
        for entry in directory {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "toml") {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }

    fn seen(&self) -> Result<Seen, String> {
        self.files()?
            .into_iter()
            .map(|path| {
                let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                Ok((
                    path.into_os_string(),
                    metadata.len(),
                    metadata.modified().ok(),
                ))
            })
            .collect()
    }

    /// Presets in case-insensitive name order, with a report for each unreadable
    /// file or duplicate. The first file by path wins a duplicate name.
    pub fn entries(&self) -> Read<C::Parameters> {
        let seen = self.seen();
        if let Ok(seen) = &seen
            && let Ok(last) = self.last.lock()
            && let Some((then, read)) = last.as_ref()
            && then == seen
        {
            return read.clone();
        }
        let read = self.read_entries();
        if let Ok(seen) = seen
            && let Ok(mut last) = self.last.lock()
        {
            *last = Some((seen, read.clone()));
        }
        read
    }

    fn read_entries(&self) -> Read<C::Parameters> {
        let mut bad = Vec::new();
        let overlay = match self.overlay() {
            Ok(overlay) => overlay,
            Err(e) => {
                bad.push(e);
                Table::new()
            }
        };
        let mut users: Vec<Entry<C::Parameters>> = Vec::new();
        let files = match self.files() {
            Ok(files) => files,
            Err(e) => {
                bad.push(e);
                Vec::new()
            }
        };
        for file in files {
            if file
                .file_stem()
                .is_some_and(|s| same(&s.to_string_lossy(), "factory"))
            {
                continue;
            }
            let read = || -> Result<Entry<C::Parameters>, String> {
                let source = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
                let table = parse(&source)?;
                check_format(&table)?;
                let sound = sound_of(&table, &Table::new(), &self.codec)?;
                if users.iter().any(|e| same(&e.sound.name, &sound.name)) {
                    return Err(format!("A second preset {}", sound.name));
                }
                let replaces = [
                    table.get("replaces").and_then(Item::as_str),
                    Some(sound.name.as_str()),
                ]
                .into_iter()
                .flatten()
                .find_map(|name| self.factory.iter().find(|e| same(&e.sound.name, name)))
                .map(|e| e.sound.name.clone());
                Ok(Entry {
                    favorite: table
                        .get("favorite")
                        .and_then(Item::as_bool)
                        .unwrap_or(false),
                    origin: if replaces.is_some() {
                        Origin::Edited
                    } else {
                        Origin::User
                    },
                    replaces,
                    sound,
                    table,
                    file: Some(file.clone()),
                    source: Some(source),
                })
            };
            match read() {
                Ok(entry) => users.push(entry),
                Err(e) => bad.push(format!("{}: {e}", file.display())),
            }
        }
        let mut list: Vec<_> = self
            .factory
            .iter()
            .filter_map(|factory| {
                let marks = overlay::marks(&overlay, &factory.sound.name);
                if marks.and_then(|m| m.get("hidden")).and_then(Item::as_bool) == Some(true)
                    || users
                        .iter()
                        .any(|e| e.replaces.as_deref() == Some(factory.sound.name.as_str()))
                {
                    return None;
                }
                let mut entry = factory.clone();
                if let Some(marks) = marks {
                    entry.favorite = marks
                        .get("favorite")
                        .and_then(Item::as_bool)
                        .unwrap_or(false);
                    if let Some(tags) = marks.get("tags").and_then(Item::as_array) {
                        entry.sound.tags = tags
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect();
                    }
                }
                Some(entry)
            })
            .collect();
        list.extend(users);
        list.sort_by(|a, b| {
            a.sound
                .name
                .to_lowercase()
                .cmp(&b.sound.name.to_lowercase())
                .then(a.sound.name.cmp(&b.sound.name))
        });
        (list, bad)
    }

    pub fn find(&self, name: &str) -> Option<Entry<C::Parameters>> {
        self.entries()
            .0
            .into_iter()
            .find(|e| same(&e.sound.name, name))
    }

    pub fn search(
        &self,
        query: &str,
        tags: &[String],
        favorite: bool,
        mine: bool,
    ) -> Vec<Entry<C::Parameters>> {
        matching(&self.entries().0, query, tags, favorite, mine)
    }

    pub fn tags(&self) -> Vec<String> {
        tags_of(&self.entries().0)
    }

    /// Choose the preferred filename only if no other file owns it, ignoring
    /// case on every system. An existing sound otherwise stays in its own file.
    fn place(&self, name: &str, own: Option<&Path>) -> Result<PathBuf, String> {
        let preferred = self.dir.join(format!("{name}.toml"));
        let occupied = std::fs::read_dir(&self.dir)
            .map_err(|e| e.to_string())?
            .map(|entry| entry.map(|e| e.path()).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|path| {
                Some(path.as_path()) != own
                    && path
                        .file_name()
                        .zip(preferred.file_name())
                        .is_some_and(|(a, b)| same(&a.to_string_lossy(), &b.to_string_lossy()))
            });
        if occupied {
            return own
                .map(Path::to_path_buf)
                .ok_or_else(|| format!("There is already a file for {name}"));
        }
        Ok(preferred)
    }

    fn put(
        &self,
        entry: &Entry<C::Parameters>,
        file: &Path,
        old_file: Option<&Path>,
    ) -> Result<(), String> {
        let mut table = entry.table.clone();
        table.insert("format", value(FORMAT));
        table.insert("name", value(entry.sound.name.as_str()));
        table.insert("description", value(entry.sound.description.as_str()));
        table.insert("tags", strings(&entry.sound.tags));
        table.insert("favorite", value(entry.favorite));
        if let Some(replaces) = &entry.replaces {
            table.insert("replaces", value(replaces.as_str()));
        } else {
            table.remove("replaces");
        }
        let text = match &entry.source {
            Some(source) => encode_preserving(&table, source)?,
            None => encode(&table)?,
        };
        // Validate the codec's result before replacing an existing file.
        parse_user(&text, &self.codec)?;
        write_atomic(file, &text)?;
        self.forget();
        if let Some(old) = old_file.filter(|old| *old != file) {
            // A case-only change may already have replaced the same filesystem entry.
            if old.exists() && !same_file_name(old, file)? {
                std::fs::remove_file(old).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Save the current sound, keeping the existing description, tags (unless
    /// supplied) and favourite. A factory sound gains a user version.
    pub fn save(
        &self,
        name: &str,
        parameters: C::Parameters,
        tags: Option<Vec<String>>,
    ) -> Result<Entry<C::Parameters>, String> {
        let name = name.trim();
        check_name(name)?;
        let _guard = self.transaction()?;
        let existing = self.find(name);
        let factory = self.factory.iter().find(|e| same(&e.sound.name, name));
        let mut entry = existing
            .or_else(|| factory.cloned())
            .unwrap_or_else(|| Entry {
                sound: Sound {
                    name: name.to_owned(),
                    description: String::new(),
                    tags: Vec::new(),
                    parameters: parameters.clone(),
                },
                favorite: false,
                origin: Origin::User,
                replaces: None,
                file: None,
                table: Table::new(),
                source: None,
            });
        entry.replaces = entry
            .replaces
            .or_else(|| factory.map(|e| e.sound.name.clone()));
        entry.sound.name = name.to_owned();
        entry.sound.parameters = parameters;
        if let Some(tags) = tags {
            entry.sound.tags = tags;
        }
        self.codec
            .write(&entry.sound.parameters, &mut entry.table)?;
        let file = self.place(name, entry.file())?;
        self.put(&entry, &file, entry.file())?;
        self.find(name)
            .ok_or_else(|| format!("{name} was not saved"))
    }

    pub fn rename(&self, name: &str, to: &str) -> Result<Entry<C::Parameters>, String> {
        let to = to.trim();
        check_name(to)?;
        let _guard = self.transaction()?;
        let mut entry = self.find(name).ok_or_else(|| format!("No preset {name}"))?;
        if self
            .entries()
            .0
            .iter()
            .any(|e| same(&e.sound.name, to) && !same(&e.sound.name, name))
        {
            return Err(format!("There is already a preset {to}"));
        }
        if entry.origin == Origin::Factory {
            entry.replaces = Some(entry.sound.name.clone());
            // Factory defaults can be inherited from the factory file's root.
            // A standalone user file must spell them out through its codec.
            self.codec
                .write(&entry.sound.parameters, &mut entry.table)?;
        }
        entry.sound.name = to.to_owned();
        let file = self.place(to, entry.file())?;
        self.put(&entry, &file, entry.file())?;
        self.find(to).ok_or_else(|| format!("{to} was not saved"))
    }

    fn mark(
        &self,
        name: &str,
        tags: Option<Vec<String>>,
        favorite: Option<bool>,
    ) -> Result<(), String> {
        let _guard = self.transaction()?;
        let mut entry = self.find(name).ok_or_else(|| format!("No preset {name}"))?;
        if let Some(file) = entry.file.clone() {
            if let Some(tags) = tags {
                entry.sound.tags = tags;
            }
            if let Some(favorite) = favorite {
                entry.favorite = favorite;
            }
            self.put(&entry, &file, Some(&file))
        } else {
            self.change_overlay(|presets| {
                overlay::change_marks(presets, &entry.sound.name, |marks| {
                    if let Some(tags) = tags {
                        marks.insert("tags", strings(&tags));
                    }
                    if let Some(favorite) = favorite {
                        overlay::set_mark(marks, "favorite", favorite);
                    }
                    Ok(())
                })
            })
        }
    }

    pub fn favorite(&self, name: &str, on: bool) -> Result<(), String> {
        self.mark(name, None, Some(on))
    }

    pub fn tag(&self, name: &str, tags: Vec<String>) -> Result<(), String> {
        self.mark(name, Some(tags), None)
    }

    /// Hide a factory sound, or its edited version, before deleting any user file.
    /// An unreadable overlay therefore leaves the user version intact too (R18).
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let _guard = self.transaction()?;
        let entry = self.find(name).ok_or_else(|| format!("No preset {name}"))?;
        let hide = if entry.origin == Origin::Factory {
            Some(&entry.sound.name)
        } else {
            entry.replaces.as_ref()
        };
        if let Some(name) = hide {
            self.change_overlay(|presets| {
                overlay::change_marks(presets, name, |m| {
                    overlay::set_mark(m, "hidden", true);
                    Ok(())
                })
            })?;
        }
        if let Some(file) = entry.file() {
            self.delete(file)?;
        }
        Ok(())
    }

    pub fn revert(&self, name: &str) -> Result<String, String> {
        let _guard = self.transaction()?;
        let entry = self.find(name).ok_or_else(|| format!("No preset {name}"))?;
        let (Some(file), Some(factory)) = (entry.file(), entry.replaces.as_ref()) else {
            return Err(format!("{name} is not an edited factory preset"));
        };
        self.change_overlay(|presets| {
            overlay::change_marks(presets, factory, |m| {
                overlay::set_mark(m, "hidden", false);
                Ok(())
            })
        })?;
        self.delete(file)?;
        Ok(factory.clone())
    }

    pub fn restore(&self) -> Result<Vec<String>, String> {
        let _guard = self.transaction()?;
        let mut restored = Vec::new();
        self.change_overlay(|presets| {
            for (name, item) in presets.iter_mut() {
                if let Some(marks) = item.as_table_like_mut()
                    && marks.get("hidden").and_then(Item::as_bool) == Some(true)
                {
                    marks.remove("hidden");
                    restored.push(name.get().to_owned());
                }
            }
            Ok(())
        })?;
        restored.sort();
        Ok(restored)
    }

    fn delete(&self, file: &Path) -> Result<(), String> {
        std::fs::remove_file(file).map_err(|e| e.to_string())?;
        self.forget();
        Ok(())
    }
}

// After a case-only replacement on a case-insensitive filesystem the directory
// contains only the destination spelling. On a case-sensitive one both remain.
fn same_file_name(old: &Path, new: &Path) -> Result<bool, String> {
    let directory = old.parent().ok_or("A preset needs a parent directory")?;
    let mut old_spelling = false;
    let mut new_spelling = false;
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        old_spelling |= Some(name.as_os_str()) == old.file_name();
        new_spelling |= Some(name.as_os_str()) == new.file_name();
    }
    Ok(!(old_spelling && new_spelling))
}

pub fn matching<P: Clone>(
    list: &[Entry<P>],
    query: &str,
    tags: &[String],
    favorite: bool,
    mine: bool,
) -> Vec<Entry<P>> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    list.iter()
        .filter(|e| {
            let text = format!(
                "{} {} {}",
                e.sound.name,
                e.sound.description,
                e.sound.tags.join(" ")
            )
            .to_lowercase();
            words.iter().all(|word| text.contains(word))
                && tags
                    .iter()
                    .all(|tag| e.sound.tags.iter().any(|t| same(tag, t)))
                && (!favorite || e.favorite)
                && (!mine || e.origin != Origin::Factory)
        })
        .cloned()
        .collect()
}

pub fn tags_of<P>(list: &[Entry<P>]) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in list.iter().flat_map(|e| &e.sound.tags) {
        if !tags.iter().any(|t| same(t, tag)) {
            tags.push(tag.clone());
        }
    }
    tags.sort_by_key(|t| t.to_lowercase());
    tags
}

#[cfg(test)]
mod tests;
