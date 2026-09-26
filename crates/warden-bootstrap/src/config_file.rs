//! P82 — saving `config.toml` without losing what the user wrote by hand. Every save (desktop
//! Settings, the web's ⚙, the CLI's `/models`) goes through `save_config`, which used to write
//! `toml::to_string_pretty` over the whole file and drop every comment and the user's ordering.
//!
//! `render_config` serializes the new `FileConfig` the same way, then merges it into the file
//! that's already there with `toml_edit`: keys whose value didn't change keep their exact text
//! and comments, changed values keep the comments around them, keys the new config doesn't have
//! are removed and new ones are added. Entries of an array of tables (`[[providers]]`,
//! `[[agents]]`, `[[mcp_servers]]`, ...) are matched by their `id`/`name`, so removing or
//! reordering one doesn't move the comments of the others.

use anyhow::Context;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

use crate::FileConfig;

/// The text to write for `config`, keeping the comments and layout of `existing` (the file's
/// current content) wherever the values still match. With no `existing`, or one that isn't
/// valid TOML, it's the plain `toml::to_string_pretty` output — the same as before P82.
pub fn render_config(existing: Option<&str>, config: &FileConfig) -> anyhow::Result<String> {
    let fresh = toml::to_string_pretty(config).context("failed to serialize config")?;
    let Some(mut doc) = existing.and_then(|text| text.parse::<DocumentMut>().ok()) else {
        return Ok(fresh);
    };
    let new_doc = fresh.parse::<DocumentMut>().context("failed to re-read the serialized config")?;
    let mut next_position = max_position(doc.as_table()).unwrap_or(0) + 1;
    merge_table(doc.as_table_mut(), new_doc.as_table(), &mut next_position);
    Ok(doc.to_string())
}

/// `next_position` is where the next table added to the file goes: `toml_edit` prints tables in
/// the order of their position in the original file, and one without a position would land
/// after whichever table happens to precede it in the map, often mid-file.
fn merge_table(old: &mut Table, new: &Table, next_position: &mut isize) {
    let gone: Vec<String> = old.iter().map(|(key, _)| key.to_string()).filter(|key| !new.contains_key(key)).collect();
    for key in gone {
        old.remove(&key);
    }
    for (key, new_item) in new.iter() {
        match old.get_mut(key) {
            Some(old_item) => merge_item(old_item, new_item, next_position),
            // `to_string_pretty` writes `providers = []`, `[api_keys]` and the like even when
            // they're empty; every such field is `#[serde(default)]`, so leaving them out of a
            // file that didn't have them reads back the same.
            None if is_empty(new_item) => {}
            None => {
                let mut item = new_item.clone();
                place_at_end(&mut item, next_position);
                old.insert(key, item);
            }
        }
    }
}

fn merge_item(old: &mut Item, new: &Item, next_position: &mut isize) {
    match (&mut *old, new) {
        (Item::Table(old_table), Item::Table(new_table)) => merge_table(old_table, new_table, next_position),
        (Item::ArrayOfTables(old_array), Item::ArrayOfTables(new_array)) => merge_array_of_tables(old_array, new_array, next_position),
        (Item::Value(old_value), Item::Value(new_value)) => merge_value(old_value, new_value),
        // A table the user wrote inline (`api_keys = { gemini = "..." }`) stays inline.
        (Item::Value(old_value @ Value::InlineTable(_)), Item::Table(new_table)) => {
            merge_value(old_value, &Value::InlineTable(new_table.clone().into_inline_table()));
        }
        _ => {
            *old = new.clone();
            place_at_end(old, next_position);
        }
    }
}

fn is_empty(item: &Item) -> bool {
    match item {
        Item::Table(table) => table.iter().all(|(_, item)| is_empty(item)),
        Item::ArrayOfTables(array) => array.is_empty(),
        Item::Value(Value::Array(array)) => array.is_empty(),
        Item::Value(Value::InlineTable(table)) => table.is_empty(),
        _ => false,
    }
}

/// Keeps `old` untouched when it means the same as `new`; otherwise takes `new`'s value with
/// `old`'s surrounding whitespace and trailing comment.
fn merge_value(old: &mut Value, new: &Value) {
    if same_value(old, new) {
        return;
    }
    let decor = old.decor().clone();
    *old = new.clone();
    *old.decor_mut() = decor;
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(a), Value::String(b)) => a.value() == b.value(),
        (Value::Integer(a), Value::Integer(b)) => a.value() == b.value(),
        (Value::Float(a), Value::Float(b)) => a.value() == b.value(),
        (Value::Boolean(a), Value::Boolean(b)) => a.value() == b.value(),
        (Value::Datetime(a), Value::Datetime(b)) => a.value() == b.value(),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| same_value(a, b)),
        (Value::InlineTable(a), Value::InlineTable(b)) => {
            a.len() == b.len() && a.iter().all(|(key, value)| b.get(key).is_some_and(|other| same_value(value, other)))
        }
        _ => false,
    }
}

/// Rebuilds `old` in `new`'s order, reusing (and merging into) the old entry with the same
/// identity — its `id`/`name`, or its position when an entry has neither.
fn merge_array_of_tables(old: &mut ArrayOfTables, new: &ArrayOfTables, next_position: &mut isize) {
    let mut unused: Vec<Option<Table>> = old.iter().cloned().map(Some).collect();
    let mut merged = ArrayOfTables::new();
    for (index, new_table) in new.iter().enumerate() {
        let matched = match identity(new_table) {
            Some(id) => unused.iter().position(|t| t.as_ref().is_some_and(|t| identity(t).as_deref() == Some(id.as_str()))),
            None => unused.get(index).and_then(|t| t.as_ref()).filter(|t| identity(t).is_none()).map(|_| index),
        };
        let table = match matched.and_then(|i| unused[i].take()) {
            Some(mut old_table) => {
                merge_table(&mut old_table, new_table, next_position);
                old_table
            }
            None => {
                let mut table = new_table.clone();
                // Right after the entry before it (ties print in array order), or at the end of
                // the file when it's the first one.
                match merged.iter().last().and_then(Table::position) {
                    Some(previous) => set_position_deep(&mut table, Some(previous)),
                    None => set_positions_from(&mut table, next_position),
                }
                table
            }
        };
        merged.push(table);
    }
    // `toml_edit` prints tables by their position in the original file, so entries the new
    // config reordered would come out in the old order. When that happens, give the whole array
    // its first slot and let the (stable) print order follow the array.
    let positions: Vec<isize> = merged.iter().filter_map(Table::position).collect();
    if positions.windows(2).any(|pair| pair[0] > pair[1]) {
        let first = positions.iter().copied().min();
        for table in merged.iter_mut() {
            set_position_deep(table, first);
        }
    }
    *old = merged;
}

fn identity(table: &Table) -> Option<String> {
    ["id", "name"].iter().find_map(|key| table.get(key).and_then(Item::as_str).map(str::to_string))
}

fn max_position(table: &Table) -> Option<isize> {
    let nested = table.iter().filter_map(|(_, item)| match item {
        Item::Table(nested) => max_position(nested),
        Item::ArrayOfTables(array) => array.iter().filter_map(max_position).max(),
        _ => None,
    });
    table.position().into_iter().chain(nested).max()
}

fn place_at_end(item: &mut Item, next_position: &mut isize) {
    match item {
        Item::Table(table) => set_positions_from(table, next_position),
        Item::ArrayOfTables(array) => array.iter_mut().for_each(|table| set_positions_from(table, next_position)),
        _ => {}
    }
}

/// Numbers `table` and every table under it in print order, starting at `next_position`.
fn set_positions_from(table: &mut Table, next_position: &mut isize) {
    table.set_position(Some(*next_position));
    *next_position += 1;
    for (_, item) in table.iter_mut() {
        place_at_end(item, next_position);
    }
}

fn set_position_deep(table: &mut Table, position: Option<isize>) {
    table.set_position(position);
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(nested) => set_position_deep(nested, position),
            Item::ArrayOfTables(array) => array.iter_mut().for_each(|nested| set_position_deep(nested, position)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Provider, ProviderConfig};

    fn provider(id: &str, model: &str) -> ProviderConfig {
        ProviderConfig { id: id.to_string(), kind: Provider::Gemini, api_key: None, base_url: None, model: Some(model.to_string()) }
    }

    fn config_with(providers: Vec<ProviderConfig>, active: &str) -> FileConfig {
        FileConfig { vault_path: Some("/home/me/vault".to_string()), providers, active_provider: Some(active.to_string()), ..Default::default() }
    }

    const HAND_WRITTEN: &str = r#"# Minha configuração do Warden
vault_path = "/home/me/vault" # onde ficam as notas

# qual provedor usar
active_provider = "work"

# o do trabalho
[[providers]]
id = "work"
kind = "gemini"
model = "gemini-3.5-flash"

# o de casa
[[providers]]
id = "home"
kind = "gemini"
model = "gemini-3.5-pro"
"#;

    fn original() -> FileConfig {
        config_with(vec![provider("work", "gemini-3.5-flash"), provider("home", "gemini-3.5-pro")], "work")
    }

    fn round_trips(text: &str, config: &FileConfig) {
        let parsed: FileConfig = toml::from_str(text).unwrap();
        assert_eq!(&parsed, config, "saved text:\n{text}");
    }

    #[test]
    fn saving_the_same_config_keeps_the_file_byte_for_byte() {
        assert_eq!(render_config(Some(HAND_WRITTEN), &original()).unwrap(), HAND_WRITTEN);
    }

    #[test]
    fn changing_a_value_keeps_every_comment_including_the_one_on_that_line() {
        let mut config = original();
        config.vault_path = Some("/data/vault".to_string());
        config.providers[1].model = Some("gemini-4".to_string());
        let text = render_config(Some(HAND_WRITTEN), &config).unwrap();
        assert_eq!(text, HAND_WRITTEN.replace("/home/me/vault", "/data/vault").replace("gemini-3.5-pro", "gemini-4"));
        round_trips(&text, &config);
    }

    #[test]
    fn removing_a_provider_takes_its_comment_along_and_leaves_the_others() {
        let config = config_with(vec![provider("home", "gemini-3.5-pro")], "home");
        let text = render_config(Some(HAND_WRITTEN), &config).unwrap();
        assert!(!text.contains("o do trabalho"), "{text}");
        assert!(text.contains("# o de casa\n[[providers]]\nid = \"home\""), "{text}");
        assert!(text.contains("# qual provedor usar\nactive_provider = \"home\""), "{text}");
        round_trips(&text, &config);
    }

    #[test]
    fn reordering_providers_moves_each_comment_with_its_own_entry() {
        let config = config_with(vec![provider("home", "gemini-3.5-pro"), provider("work", "gemini-3.5-flash")], "work");
        let text = render_config(Some(HAND_WRITTEN), &config).unwrap();
        let home = text.find("# o de casa\n[[providers]]\nid = \"home\"").expect(&text);
        let work = text.find("# o do trabalho\n[[providers]]\nid = \"work\"").expect(&text);
        assert!(home < work, "{text}");
        round_trips(&text, &config);
    }

    #[test]
    fn new_keys_and_entries_are_added_and_cleared_keys_removed() {
        let mut config = original();
        config.vault_path = None;
        config.enable_shell = Some(true);
        config.providers.push(provider("spare", "gemini-3.5-flash"));
        let text = render_config(Some(HAND_WRITTEN), &config).unwrap();
        // The comments written above and beside a removed key go with it, like a removed
        // provider's; the rest of the file stays as it was.
        assert!(text.starts_with("\n# qual provedor usar\nactive_provider = \"work\"\nenable_shell = true\n\n# o do trabalho\n"), "{text}");
        assert!(text.ends_with("# o de casa\n[[providers]]\nid = \"home\"\nkind = \"gemini\"\nmodel = \"gemini-3.5-pro\"\n\n[[providers]]\nid = \"spare\"\nkind = \"gemini\"\nmodel = \"gemini-3.5-flash\"\n"), "{text}");
        round_trips(&text, &config);
    }

    #[test]
    fn a_new_entry_follows_its_array_and_a_new_table_goes_to_the_end() {
        let existing = format!("{HAND_WRITTEN}\n# segredos\n[api_keys]\ngemini = \"g\"\n");
        let mut config = original();
        config.api_keys.gemini = Some("g".to_string());
        assert_eq!(render_config(Some(&existing), &config).unwrap(), existing);

        config.providers.push(provider("spare", "gemini-3.5-flash"));
        config.embedded_server = Some(crate::EmbeddedServerConfig::new(7420, "k".repeat(64)));
        let text = render_config(Some(&existing), &config).unwrap();
        let spare = text.find("id = \"spare\"").expect(&text);
        let api_keys = text.find("# segredos\n[api_keys]").expect(&text);
        let embedded = text.find("[embedded_server]").expect(&text);
        assert!(text.find("id = \"home\"").unwrap() < spare && spare < api_keys && api_keys < embedded, "{text}");
        round_trips(&text, &config);
    }

    #[test]
    fn an_inline_table_written_by_hand_stays_inline() {
        let existing = "api_keys = { gemini = \"old\" } # chaves\n";
        let mut config = FileConfig::default();
        config.api_keys.gemini = Some("new".to_string());
        let text = render_config(Some(existing), &config).unwrap();
        assert_eq!(text, "api_keys = { gemini = \"new\" } # chaves\n");
        round_trips(&text, &config);
    }

    #[test]
    fn no_file_or_an_unreadable_one_gets_the_plain_output() {
        let plain = toml::to_string_pretty(&original()).unwrap();
        assert_eq!(render_config(None, &original()).unwrap(), plain);
        assert_eq!(render_config(Some("this is = = not toml"), &original()).unwrap(), plain);
    }
}
