//! In-place update of an existing TOML document with `toml_edit`, so a user's comments, key order and formatting
//! survive when the app saves settings.
//!
//! The new content is produced by plain serde serialization; this module carries it over into the document parsed
//! from the existing file: keys missing from the new content are removed, changed values are replaced (keeping the
//! comments around them), unchanged values keep their original spelling, new keys are appended to their table.

use toml_edit::{DocumentMut, Item, TableLike, Value};

/// Returns `existing` updated to the content of `new`, or `None` if either does not parse.
pub(crate) fn update_document(existing: &str, new: &str) -> Option<String> {
    let mut old: DocumentMut = existing.parse().ok()?;
    let new: DocumentMut = new.parse().ok()?;
    merge_table(old.as_table_mut(), new.as_table());
    Some(old.to_string())
}

fn merge_table(old: &mut dyn TableLike, new: &dyn TableLike) {
    let stale: Vec<String> = old
        .iter()
        .map(|(k, _)| k.to_owned())
        .filter(|k| !new.contains_key(k))
        .collect();
    for key in stale {
        old.remove(&key);
    }
    for (key, new_item) in new.iter() {
        match old.get_mut(key) {
            Some(old_item) => merge_item(old_item, new_item),
            None => {
                old.insert(key, detached(new_item));
            }
        }
    }
}

fn merge_item(old: &mut Item, new: &Item) {
    if let (Some(o), Some(n)) = (old.as_table_like_mut(), new.as_table_like()) {
        merge_table(o, n);
        return;
    }
    match (old, new) {
        (Item::Value(o), Item::Value(n)) => {
            if !same_value(o, n) {
                let decor = o.decor().clone();
                let mut v = n.clone();
                *v.decor_mut() = decor;
                *o = v;
            }
        }
        (old, new) => *old = detached(new),
    }
}

/// Whether two values are equal as data, ignoring spelling (`0x1E` vs `30`, quote style, array layout).
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.value() == y.value(),
        (Value::Integer(x), Value::Integer(y)) => x.value() == y.value(),
        (Value::Float(x), Value::Float(y)) => x.value().to_bits() == y.value().to_bits(),
        (Value::Boolean(x), Value::Boolean(y)) => x.value() == y.value(),
        (Value::Datetime(x), Value::Datetime(y)) => x.value() == y.value(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(v, w)| same_value(v, w))
        }
        (Value::InlineTable(x), Value::InlineTable(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_value(v, w)))
        }
        _ => false,
    }
}

/// A copy of `item` without document positions, so it renders after the tables already in the target document
/// instead of at a position that belonged to the generated document.
fn detached(item: &Item) -> Item {
    let mut item = item.clone();
    clear_positions(&mut item);
    item
}

fn clear_positions(item: &mut Item) {
    match item {
        Item::Table(t) => {
            t.set_position(None);
            for (_, child) in t.iter_mut() {
                clear_positions(child);
            }
        }
        Item::ArrayOfTables(a) => {
            for t in a.iter_mut() {
                t.set_position(None);
                for (_, child) in t.iter_mut() {
                    clear_positions(child);
                }
            }
        }
        Item::None | Item::Value(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(existing: &str, new: &str) -> String {
        update_document(existing, new).expect("both parse")
    }

    #[test]
    fn keeps_comments_and_order_of_unchanged_keys() {
        let existing = "# my settings\nb = 2 # bee\na = 1\n\n# tables\n[t]\nx = \"keep\" # note\n";
        let new = "a = 1\nb = 2\n\n[t]\nx = \"keep\"\n";
        assert_eq!(update(existing, new), existing);
    }

    #[test]
    fn replaces_changed_values_keeping_their_comment() {
        let existing = "# head\nframe_rate = 30 # fps\n";
        let new = "frame_rate = 45\n";
        assert_eq!(update(existing, new), "# head\nframe_rate = 45 # fps\n");
    }

    #[test]
    fn keeps_original_spelling_of_equal_values() {
        let existing = "n = 0x1E\ns = 'lit'\n";
        let new = "n = 0x1E\ns = 'lit'\n";
        assert_eq!(update(existing, new), existing);
    }

    #[test]
    fn removes_stale_and_appends_new_keys() {
        let existing = "a = 1\ngone = true\n\n[t]\nx = 1\n";
        let new = "a = 1\nadded = \"yes\"\n\n[t]\nx = 1\ny = 2\n\n[u]\nz = 3\n";
        let out = update(existing, new);
        assert_eq!(out, "a = 1\nadded = \"yes\"\n\n[t]\nx = 1\ny = 2\n\n[u]\nz = 3\n");
    }

    #[test]
    fn merges_into_inline_tables() {
        let existing = "t = { x = 1, y = 2 } # inline\n";
        let new = "[t]\nx = 1\ny = 3\n";
        assert_eq!(update(existing, new), "t = { x = 1, y = 3 } # inline\n");
    }

    #[test]
    fn keeps_layout_of_equal_arrays_and_replaces_changed_ones() {
        let existing = "a = [
  1,
  2, # two
]
b = [1, 2] # bee
";
        let new = "a = [1, 2]
b = [1, 3]
";
        assert_eq!(
            update(existing, new),
            "a = [
  1,
  2, # two
]
b = [1, 3] # bee
"
        );
    }

    #[test]
    fn rejects_unparsable_input() {
        assert!(update_document("a = = 1", "a = 1").is_none());
    }
}
