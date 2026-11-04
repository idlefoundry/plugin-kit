//! The factory's tags, favourites and hidden sounds live only in factory.toml.

use crate::{Item, Library, PresetCodec, Table, TableLike, same, value};
use plugin_kit_settings::{check_format, encode, read, write_atomic};

pub(crate) fn marks<'a>(overlay: &'a Table, name: &str) -> Option<&'a dyn TableLike> {
    let presets = overlay.get("presets")?.as_table_like()?;
    presets
        .get(name)
        .or_else(|| {
            presets
                .iter()
                .find(|(key, _)| same(key, name))
                .map(|(_, v)| v)
        })?
        .as_table_like()
}

pub(crate) fn change_marks(
    presets: &mut dyn TableLike,
    name: &str,
    change: impl FnOnce(&mut dyn TableLike) -> Result<(), String>,
) -> Result<(), String> {
    let key = presets
        .iter()
        .find(|(key, _)| *key == name)
        .or_else(|| presets.iter().find(|(key, _)| same(key, name)))
        .map(|(key, _)| key.to_owned())
        .unwrap_or_else(|| name.to_owned());
    if presets.get(&key).is_none() {
        presets.insert(&key, Item::Table(Table::new()));
    }
    let marks = presets
        .get_mut(&key)
        .and_then(Item::as_table_like_mut)
        .ok_or("A factory preset's marks are not a table; left as they are")?;
    change(marks)
}

pub(crate) fn set_mark(marks: &mut dyn TableLike, key: &str, on: bool) {
    if on {
        marks.insert(key, value(true));
    } else {
        marks.remove(key);
    }
}

impl<C: PresetCodec> Library<C> {
    pub(super) fn overlay(&self) -> Result<Table, String> {
        let table = read(&self.dir.join("factory.toml"))?.unwrap_or_default();
        check_format(&table)?;
        if table
            .get("presets")
            .is_some_and(|p| p.as_table_like().is_none())
        {
            return Err("factory.toml: `presets` is not a table; left as it is".into());
        }
        Ok(table)
    }

    pub(super) fn change_overlay(
        &self,
        change: impl FnOnce(&mut dyn TableLike) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut table = self.overlay()?;
        let before = encode(&table)?;
        let had_presets = table.get("presets").is_some();
        if !had_presets {
            table.insert("presets", Item::Table(Table::new()));
        }
        let presets = table
            .get_mut("presets")
            .and_then(Item::as_table_like_mut)
            .ok_or("Factory marks need a table")?;
        change(presets)?;
        let empty: Vec<_> = presets
            .iter()
            .filter(|(_, item)| item.as_table_like().is_some_and(|marks| marks.is_empty()))
            .map(|(key, _)| key.to_owned())
            .collect();
        for key in empty {
            presets.remove(&key);
        }
        if !had_presets && presets.is_empty() {
            table.remove("presets");
        }
        if encode(&table)? == before {
            return Ok(());
        }
        table.insert("format", value(crate::FORMAT));
        write_atomic(&self.dir.join("factory.toml"), &encode(&table)?)?;
        self.forget();
        Ok(())
    }
}
