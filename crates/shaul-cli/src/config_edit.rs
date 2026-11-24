//! Format-preserving `config set` support.
//!
//! `Config::save` serializes the whole typed config, which drops comments and
//! key ordering. `config set` instead validates through the typed config and
//! then updates only the target key in a [`toml_edit::DocumentMut`].

use std::path::Path;

use shaul::{Config, Error, Result};

/// Validates and sets `key` in the config file at `path`, preserving comments
/// and unrelated formatting.
pub fn set_value(path: &Path, key: &str, value: &str) -> Result<()> {
    let mut config = Config::load_file_only(path)?;
    config.set_value(key, value)?;

    // Use the typed, validated config as the source of the new TOML value so
    // booleans, integers and arrays keep their native types.
    let serialized = toml::to_string(&config)?;
    let full: toml::Value = toml::from_str(&serialized)?;
    let new_value = lookup(&full, key);

    let text = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| Error::Config(format!("invalid config file: {error}")))?;
    set_doc_value(&mut doc, key, new_value)?;

    let text = doc.to_string();
    toml::from_str::<toml::Value>(&text)
        .map_err(|error| Error::Config(format!("generated config is invalid: {error}")))?;
    Config::save_text(path, &text)
}

fn lookup<'a>(full: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    if let Some(name) = key.strip_prefix("headers.") {
        return full.get("headers").and_then(|headers| headers.get(name));
    }
    let mut parts = key.split('.');
    let first = parts.next()?;
    match parts.next() {
        None => full.get(first),
        Some(leaf) => full.get(first).and_then(|section| section.get(leaf)),
    }
}

fn set_doc_value(
    doc: &mut toml_edit::DocumentMut,
    key: &str,
    value: Option<&toml::Value>,
) -> Result<()> {
    if let Some(name) = key.strip_prefix("headers.") {
        let table = ensure_table(doc, "headers")?;
        match value {
            Some(value) => set_table_value(table, name, edit_value(value)?),
            None => {
                table.remove(name);
            }
        }
        return Ok(());
    }

    let mut parts = key.split('.');
    let first = parts
        .next()
        .ok_or_else(|| Error::Config(format!("invalid config key: {key}")))?;
    match parts.next() {
        None => match value {
            Some(value) => set_table_value(doc, first, edit_value(value)?),
            None => {
                doc.remove(first);
            }
        },
        Some(leaf) => {
            if parts.next().is_some() {
                return Err(Error::Config(format!("unsupported config key: {key}")));
            }
            let table = ensure_table(doc, first)?;
            match value {
                Some(value) => set_table_value(table, leaf, edit_value(value)?),
                None => {
                    table.remove(leaf);
                }
            }
        }
    }
    Ok(())
}

/// Replaces an existing value in place so the key's leading comments and
/// spacing survive; the value's own trailing decoration is also preserved.
fn set_table_value(table: &mut toml_edit::Table, key: &str, value: toml_edit::Value) {
    if let Some(existing) = table.get_mut(key) {
        if let Some(old) = existing.as_value() {
            let mut value = value;
            *value.decor_mut() = old.decor().clone();
            *existing = toml_edit::Item::Value(value);
        } else {
            *existing = toml_edit::Item::Value(value);
        }
    } else {
        table.insert(key, toml_edit::Item::Value(value));
    }
}

fn ensure_table<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    section: &str,
) -> Result<&'a mut toml_edit::Table> {
    if doc.get(section).is_none() {
        doc.insert(section, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    doc.get_mut(section)
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| Error::Config(format!("[{section}] must be a table")))
}

fn edit_value(value: &toml::Value) -> Result<toml_edit::Value> {
    Ok(match value {
        toml::Value::String(value) => toml_edit::Value::from(value.clone()),
        toml::Value::Integer(value) => toml_edit::Value::from(*value),
        toml::Value::Float(value) => toml_edit::Value::from(*value),
        toml::Value::Boolean(value) => toml_edit::Value::from(*value),
        toml::Value::Array(values) => {
            let mut array = toml_edit::Array::new();
            for value in values {
                array.push(edit_value(value)?);
            }
            toml_edit::Value::Array(array)
        }
        toml::Value::Table(values) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in values {
                table.insert(key, edit_value(value)?);
            }
            toml_edit::Value::InlineTable(table)
        }
        toml::Value::Datetime(_) => {
            return Err(Error::Config(
                "datetime config values are not supported".to_string(),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_values_in_place_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# top comment\nstate_dir = \"old\"\n\n[download]\n# connection comment\nconnections = 4 # trailing\n",
        )
        .unwrap();

        set_value(&path, "download.connections", "12").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# top comment"));
        assert!(text.contains("# connection comment"));
        assert!(text.contains("connections = 12 # trailing"), "{text}");

        set_value(&path, "hub.hf_endpoint", "https://hf-mirror.com").unwrap();
        set_value(&path, "state_dir", "new").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("hf_endpoint = \"https://hf-mirror.com\""));
        assert!(text.contains("state_dir = \"new\""));
        assert!(text.contains("# top comment"));
        Config::load_file_only(&path).unwrap();
    }
}
