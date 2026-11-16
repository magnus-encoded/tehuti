//! Sections shared between contexts: `shared/<name>.toml`, opted into with `include = ["<name>"]`.
//!
//! A name is one word or `folder/word`; words use letters, digits, `-` and `_`.

use std::path::{Path, PathBuf};
use toml::{Table, Value};
use toml_edit::{DocumentMut, Item};

/// The folder of shared sections inside a config directory.
const DIRECTORY: &str = "shared";

/// A word of a shared name cannot be this: `shared/context.toml` would read as a context.
const RESERVED: &str = "context";

/// A context with its includes merged in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The context source to parse; unchanged when the context includes nothing.
    pub source: String,
    /// Shared fields the context's own entry replaced, one line each.
    pub overridden: Vec<String>,
}

#[must_use]
pub fn shared_directory(directory: &Path) -> PathBuf {
    directory.join(DIRECTORY)
}

/// The names of the shared sections that exist, sorted.
#[must_use]
pub fn names(directory: &Path) -> Vec<String> {
    let root = shared_directory(directory);
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return found;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            let Some(folder) = path.file_name().and_then(|name| name.to_str()).map(str::to_owned) else {
                continue;
            };
            let inner = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .filter_map(Result::ok);
            for file in inner {
                if let Some(word) = toml_stem(&file.path()) {
                    found.push(format!("{folder}/{word}"));
                }
            }
        } else if let Some(word) = toml_stem(&path) {
            found.push(word);
        }
    }
    found.retain(|name| valid_name(name));
    found.sort();
    found
}

fn toml_stem(path: &Path) -> Option<String> {
    (path.extension()? == "toml").then(|| path.file_stem()?.to_str().map(str::to_owned))?
}

fn valid_word(word: &str) -> bool {
    word != RESERVED
        && !word.is_empty()
        && word
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
}

fn valid_name(name: &str) -> bool {
    let words: Vec<&str> = name.split('/').collect();
    words.len() <= 2 && words.iter().all(|word| valid_word(word))
}

fn shared_path(directory: &Path, name: &str) -> PathBuf {
    shared_directory(directory).join(format!("{name}.toml"))
}

/// Merges the shared sections a context includes into its source.
///
/// The context's own fields win; fields it does not write come from the shared sections.
///
/// # Errors
///
/// An include names a missing or unreadable section, or a shared section includes another.
pub fn resolve(source: &str, directory: &Path) -> Result<Resolved, String> {
    let unchanged = || Resolved {
        source: source.to_owned(),
        overridden: Vec::new(),
    };
    let Ok(mut context) = source.parse::<Table>() else {
        // Leave the syntax error to the context parser, which words it.
        return Ok(unchanged());
    };
    let Some(include) = context.remove("include") else {
        return Ok(unchanged());
    };
    let Value::Array(items) = include else {
        return Err("include must be a list of shared section names".into());
    };
    let mut overridden = Vec::new();
    for item in items {
        let Value::String(name) = item else {
            return Err("include must be a list of shared section names".into());
        };
        let section = read_section(directory, &name)?;
        merge(&mut context, section, &mut Vec::new(), &name, &mut overridden);
    }
    let merged = toml::to_string(&context).map_err(|error| error.to_string())?;
    Ok(Resolved {
        source: merged,
        overridden,
    })
}

fn read_section(directory: &Path, name: &str) -> Result<Table, String> {
    let listing = || {
        let existing = names(directory);
        if existing.is_empty() {
            "there are no shared sections".to_owned()
        } else {
            format!("shared sections: {}", existing.join(", "))
        }
    };
    if !valid_name(name) {
        return Err(format!("include {name:?} is not a valid shared section name"));
    }
    let text = crate::contexts::read_context(&shared_path(directory, name)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("include {name:?} names no shared section; {}", listing())
        } else {
            format!("shared section {name:?}: {error}")
        }
    })?;
    let table: Table = text
        .parse()
        .map_err(|error: toml::de::Error| format!("shared section {name:?}: {error}"))?;
    if table.contains_key("include") {
        return Err(format!(
            "shared section {name:?} cannot include other shared sections"
        ));
    }
    Ok(table)
}

/// Adds the shared fields the context lacks, at every table depth. A field the context
/// already writes stays and is reported by its dotted path.
fn merge(own: &mut Table, shared: Table, path: &mut Vec<String>, name: &str, overridden: &mut Vec<String>) {
    for (key, value) in shared {
        path.push(key.clone());
        match (own.get_mut(&key), value) {
            (None, value) => {
                let _previous = own.insert(key, value);
            }
            (Some(Value::Table(inner)), Value::Table(shared_inner)) => {
                merge(inner, shared_inner, path, name, overridden);
            }
            (Some(_), _) => overridden.push(format!(
                "{} from shared section {name:?} is replaced by the context's own",
                path.join(".")
            )),
        }
        let _popped = path.pop();
    }
}

/// Moves `sections` (dotted paths such as `buttons.left_bumper`) out of the context folder's
/// `context.toml` into `shared/<name>.toml`, leaving an include. Returns a report line.
///
/// The name defaults to the last part of a single path; several paths need a name. Nothing is
/// overwritten: the context file is first copied to a backup next to it, and its comments stay.
///
/// # Errors
///
/// The context or a section is missing, the name is invalid or taken, no name was given for
/// several paths, or a file cannot be written.
pub fn share(
    directory: &Path,
    context: &str,
    sections: &[&str],
    name: Option<&str>,
) -> Result<String, String> {
    if context.is_empty() || context.contains(['/', '\\']) || context == ".." || context == "." {
        return Err(format!("{context:?} is not a context folder name"));
    }
    let name = match (name, sections) {
        (Some(name), _) => name,
        (None, [only]) => only.rsplit('.').next().unwrap_or_default(),
        (None, _) => return Err("sharing several paths needs a name: add --as NAME".into()),
    };
    if !valid_name(name) {
        return Err(format!(
            "{name:?} is not a valid shared section name: use letters, digits, - and _, with at most one folder (not {RESERVED:?}); choose one with --as NAME"
        ));
    }
    let target = shared_path(directory, name);
    if target.exists() {
        return Err(format!(
            "shared section {name:?} already exists; choose another name with --as NAME"
        ));
    }
    let file = directory.join(context).join("context.toml");
    let original = std::fs::read_to_string(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    let mut document: DocumentMut = original
        .parse()
        .map_err(|error: toml_edit::TomlError| format!("{}: {error}", file.display()))?;
    let mut shared = DocumentMut::new();
    for section in sections {
        let path: Vec<&str> = section.split('.').collect();
        let moved = take(document.as_item_mut(), &path)
            .ok_or_else(|| format!("{} has no section {section:?}", file.display()))?;
        put(shared.as_item_mut(), &path, moved);
    }
    add_include(&mut document, name).map_err(|error| format!("{}: {error}", file.display()))?;
    let backup = backup_path(&file);
    std::fs::write(&backup, &original).map_err(|error| format!("{}: {error}", backup.display()))?;
    if let Some(folder) = target.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    write_new(&target, &shared.to_string())?;
    std::fs::write(&file, document.to_string()).map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(format!(
        "Moved {} to {} and included it as {name:?}; the previous file is kept at {}",
        sections.join(", "),
        target.display(),
        backup.display()
    ))
}

fn add_include(document: &mut DocumentMut, name: &str) -> Result<(), String> {
    if let Some(item) = document.get_mut("include") {
        item.as_array_mut().ok_or("include must be a list")?.push(name);
    } else {
        let mut list = toml_edit::Array::new();
        list.push(name);
        let _previous = document.insert("include", toml_edit::value(list));
    }
    Ok(())
}

/// Removes and returns the item at the path, dropping tables it leaves empty.
fn take(item: &mut Item, path: &[&str]) -> Option<Item> {
    let (first, rest) = path.split_first()?;
    let table = item.as_table_like_mut()?;
    if rest.is_empty() {
        return table.remove(first);
    }
    let child = table.get_mut(first)?;
    let taken = take(child, rest)?;
    if child.as_table_like().is_some_and(toml_edit::TableLike::is_empty) {
        let _removed = table.remove(first);
    }
    Some(taken)
}

/// Places `value` at the path, creating the tables above it without headers of their own.
fn put(item: &mut Item, path: &[&str], value: Item) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    let Some(table) = item.as_table_like_mut() else {
        return;
    };
    if rest.is_empty() {
        let _previous = table.insert(first, value);
        return;
    }
    if !table.contains_key(first) {
        let mut parent = toml_edit::Table::new();
        parent.set_implicit(true);
        let _previous = table.insert(first, Item::Table(parent));
    }
    if let Some(child) = table.get_mut(first) {
        put(child, rest, value);
    }
}

fn write_new(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// `context.toml.bak`, or the first `context.toml.bak.N` that does not exist yet.
fn backup_path(file: &Path) -> PathBuf {
    let mut candidate = file.with_extension("toml.bak");
    let mut number = 1_u32;
    while candidate.exists() {
        number = number.saturating_add(1);
        candidate = file.with_extension(format!("toml.bak.{number}"));
    }
    candidate
}
