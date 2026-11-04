//! The existing TOML parser, with value encoding for storage. Comments and original
//! spacing are not retained; every unknown value, including dates, is retained.

use toml_edit::{Item, Table, Value};

pub fn parse(text: &str) -> Result<Table, String> {
    text.parse::<toml_edit::DocumentMut>()
        .map(toml_edit::DocumentMut::into_table)
        .map_err(|e| format!("TOML cannot be read: {e}; left as it is"))
}

pub fn encode(table: &Table) -> Result<String, String> {
    let mut out = String::new();
    write_table(table, &[], &mut out)?;
    // Fail before touching any destination if a caller built an invalid TOML value.
    parse(&out)?;
    Ok(out)
}

/// Keep unchanged table sections byte for byte, including their comments and
/// line endings. This generalises the CA-72's opaque effects section (R-FX-PRESETS):
/// a reader which cannot host a section must not normalise its saved state.
pub fn encode_preserving(table: &Table, original: &str) -> Result<String, String> {
    fn headers(item: &Item, root: &str, starts: &mut Vec<(usize, String)>) {
        fn table_headers(table: &Table, root: &str, starts: &mut Vec<(usize, String)>) {
            if !table.is_implicit()
                && !table.is_dotted()
                && let Some(span) = table.span()
            {
                let start = table
                    .decor()
                    .prefix()
                    .and_then(|p| p.span())
                    .map_or(span.start, |s| s.start);
                starts.push((start, root.to_owned()));
            }
            for (_, item) in table {
                headers(item, root, starts);
            }
        }
        if let Some(table) = item.as_table() {
            table_headers(table, root, starts);
        }
        if let Some(tables) = item.as_array_of_tables() {
            for table in tables {
                table_headers(table, root, starts);
            }
        }
    }
    let document = toml_edit::Document::parse(original).map_err(|e| e.to_string())?;
    let mut starts = Vec::new();
    for (root, item) in document.iter() {
        headers(item, root, &mut starts);
    }
    starts.sort_by_key(|(start, _)| *start);
    starts.push((original.len(), String::new()));
    let mut kept = std::collections::BTreeMap::<String, String>::new();
    for pair in starts.windows(2) {
        let key = &pair[0].1;
        let Some(current) = table.get(key) else {
            continue;
        };
        let Some(previous) = document.get(key) else {
            continue;
        };
        let single = |item: &Item| {
            let mut t = Table::new();
            t.insert(key, item.clone());
            encode(&t)
        };
        if single(current)? == single(previous)? {
            kept.entry(key.clone())
                .or_default()
                .push_str(&original[pair[0].0..pair[1].0]);
        }
    }
    // Dotted assignments can precede the first table header. A preserved set of
    // header spans must contain the complete root, or removing that root from the
    // newly encoded table would silently drop those earlier assignments.
    kept.retain(|key, raw| {
        let Ok(parsed) = parse(raw) else {
            return false;
        };
        let (Some(section), Some(previous)) = (parsed.get(key), document.get(key)) else {
            return false;
        };
        let single = |item: &Item| {
            let mut table = Table::new();
            table.insert(key, item.clone());
            encode(&table).ok()
        };
        single(section)
            .zip(single(previous))
            .is_some_and(|(a, b)| a == b)
    });
    let mut changed = table.clone();
    for key in kept.keys() {
        changed.remove(key);
    }
    let mut out = encode(&changed)?;
    for raw in kept.values() {
        out.push('\n');
        out.push_str(raw);
    }
    parse(&out)?;
    Ok(out)
}

fn quote(text: &str) -> Result<String, String> {
    // JSON's quoted strings use escapes accepted by TOML basic strings too.
    serde_json::to_string(text).map_err(|e| e.to_string())
}

fn write_table(table: &Table, path: &[String], out: &mut String) -> Result<(), String> {
    for (key, item) in table {
        if let Item::Value(value) = item {
            out.push_str(&quote(key)?);
            out.push_str(" = ");
            write_value(value, out)?;
            out.push('\n');
        }
    }
    for (key, item) in table {
        let mut child = path.to_vec();
        child.push(quote(key)?);
        let name = child.join(".");
        match item {
            Item::Table(table) => {
                out.push_str(&format!("\n[{name}]\n"));
                write_table(table, &child, out)?;
            }
            Item::ArrayOfTables(tables) => {
                for table in tables {
                    out.push_str(&format!("\n[[{name}]]\n"));
                    write_table(table, &child, out)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn write_value(value: &Value, out: &mut String) -> Result<(), String> {
    match value {
        Value::String(v) => out.push_str(&quote(v.value())?),
        Value::Integer(v) => out.push_str(&v.value().to_string()),
        Value::Boolean(v) => out.push_str(&v.value().to_string()),
        Value::Datetime(v) => out.push_str(&v.value().to_string()),
        Value::Float(v) => {
            let f = *v.value();
            let word = if f.is_nan() {
                "nan".to_owned()
            } else if f == f64::INFINITY {
                "inf".to_owned()
            } else if f == f64::NEG_INFINITY {
                "-inf".to_owned()
            } else {
                // Debug retains the point for integral floats and negative zero.
                format!("{f:?}")
            };
            out.push_str(&word);
        }
        Value::Array(array) => {
            out.push('[');
            for (i, value) in array.iter().enumerate() {
                if i != 0 {
                    out.push_str(", ");
                }
                write_value(value, out)?;
            }
            out.push(']');
        }
        Value::InlineTable(table) => {
            out.push('{');
            for (i, (key, value)) in table.iter().enumerate() {
                if i != 0 {
                    out.push_str(", ");
                }
                out.push_str(&quote(key)?);
                out.push_str(" = ");
                write_value(value, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}
