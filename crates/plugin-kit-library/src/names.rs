//! Portable preset names and tags, from the CA-72's R18.

use crate::same;

pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.chars().count() > 64
        || name.starts_with(['.', ' '])
        || name.ends_with(['.', ' '])
        || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(char::is_control)
    {
        return Err("A name is 1 to 64 characters, without / \\ : * ? \" < > |".into());
    }
    if same(name, "factory") {
        return Err("The library keeps the name factory for its overlay".into());
    }
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
    if device {
        return Err("Windows keeps that name for a device".into());
    }
    Ok(())
}

pub fn parse_tags(text: &str) -> Result<Vec<String>, String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if tag.chars().count() > 32 || tag.chars().any(char::is_control) {
            return Err("A tag is 1 to 32 characters".into());
        }
        if !tags.iter().any(|t| same(t, tag)) {
            tags.push(tag.to_owned());
        }
    }
    if tags.len() > 16 {
        return Err("At most 16 tags".into());
    }
    Ok(tags)
}
