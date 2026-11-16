//! Sections shared between contexts: `shared/<name>.toml`, opted into with `include = ["<name>"]`.

use std::path::{Path, PathBuf};
use toml::{Table, Value};

/// The folder of shared sections inside a config directory.
const DIRECTORY: &str = "shared";

/// A shared section cannot take this name: `shared/context.toml` would read as a context.
const RESERVED: &str = "context";

/// A context with its includes merged in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The context source to parse; unchanged when the context includes nothing.
    pub source: String,
    /// Shared bindings the context's own entry replaced, one line each.
    pub overridden: Vec<String>,
}

#[must_use]
pub fn shared_directory(directory: &Path) -> PathBuf {
    directory.join(DIRECTORY)
}

/// The names of the shared sections that exist, sorted.
#[must_use]
pub fn names(directory: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(shared_directory(directory)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let stem = path.file_stem()?.to_str()?.to_owned();
            (path.extension()? == "toml" && valid_name(&stem)).then_some(stem)
        })
        .collect();
    names.sort();
    names
}

fn valid_name(name: &str) -> bool {
    name != RESERVED
        && !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
}

fn shared_path(directory: &Path, name: &str) -> PathBuf {
    shared_directory(directory).join(format!("{name}.toml"))
}

/// Merges the shared sections a context includes into its source.
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
    let mut sections = Vec::new();
    for item in items {
        let Value::String(name) = item else {
            return Err("include must be a list of shared section names".into());
        };
        sections.push((name.clone(), read_section(directory, &name)?));
    }
    let overridden = merge(&mut context, sections);
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

/// Adds each section's entries to `context`; an entry already there stays and is reported.
///
/// Entries are the top-level keys, and the keys of a top-level table such as one button or one
/// speech phrase, so a context can replace a single binding of an included `[buttons]` table.
fn merge(context: &mut Table, sections: Vec<(String, Table)>) -> Vec<String> {
    let mut overridden = Vec::new();
    for (name, section) in sections {
        for (key, value) in section {
            match (context.get_mut(&key), value) {
                (None, value) => {
                    let _previous = context.insert(key, value);
                }
                (Some(Value::Table(own)), Value::Table(shared)) => {
                    for (child, entry) in shared {
                        if own.contains_key(&child) {
                            overridden.push(format!(
                                "{key}.{child} from shared section {name:?} is replaced by the context's own"
                            ));
                        } else {
                            let _previous = own.insert(child, entry);
                        }
                    }
                }
                (Some(_), _) => overridden.push(format!(
                    "{key} from shared section {name:?} is replaced by the context's own"
                )),
            }
        }
    }
    overridden
}

/// Moves `section` (a dotted path such as `buttons.left_bumper`) out of the context folder's
/// `context.toml` into `shared/<name>.toml`, leaving an include. Returns a report line.
///
/// The name defaults to the last part of the path. Nothing is overwritten: the context file is
/// first copied to a backup next to it.
///
/// # Errors
///
/// The context or section is missing, the name is invalid or taken, or a file cannot be written.
pub fn share(directory: &Path, context: &str, section: &str, name: Option<&str>) -> Result<String, String> {
    if context.is_empty() || context.contains(['/', '\\']) || context == ".." || context == "." {
        return Err(format!("{context:?} is not a context folder name"));
    }
    let path: Vec<&str> = section.split('.').collect();
    let last = path.last().copied().unwrap_or_default();
    let name = name.unwrap_or(last);
    if !valid_name(name) {
        return Err(format!(
            "{name:?} is not a valid shared section name: use letters, digits, - and _ (not {RESERVED:?}); choose one with --as NAME"
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
    let mut table: Table = original
        .parse()
        .map_err(|error: toml::de::Error| format!("{}: {error}", file.display()))?;
    let moved =
        take(&mut table, &path).ok_or_else(|| format!("{} has no section {section:?}", file.display()))?;
    let mut shared = Table::new();
    let mut nested = moved;
    for key in path.iter().rev() {
        let mut parent = Table::new();
        let _previous = parent.insert((*key).to_owned(), nested);
        nested = Value::Table(parent);
    }
    if let Value::Table(whole) = nested {
        shared = whole;
    }
    let mut includes = match table.remove("include") {
        Some(Value::Array(items)) => items,
        Some(_) => return Err(format!("{}: include must be a list", file.display())),
        None => Vec::new(),
    };
    includes.push(Value::String(name.to_owned()));
    let _previous = table.insert("include".into(), Value::Array(includes));
    let rewritten = toml::to_string(&table).map_err(|error| error.to_string())?;
    let shared_text = toml::to_string(&shared).map_err(|error| error.to_string())?;
    let backup = backup_path(&file);
    std::fs::write(&backup, &original).map_err(|error| format!("{}: {error}", backup.display()))?;
    std::fs::create_dir_all(shared_directory(directory)).map_err(|error| error.to_string())?;
    write_new(&target, &shared_text)?;
    std::fs::write(&file, rewritten).map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(format!(
        "Moved {section} to {} and included it as {name:?}; the previous file is kept at {}",
        target.display(),
        backup.display()
    ))
}

/// Removes and returns the value at the dotted path, dropping tables it leaves empty.
fn take(table: &mut Table, path: &[&str]) -> Option<Value> {
    let (first, rest) = path.split_first()?;
    if rest.is_empty() {
        return table.remove(*first);
    }
    let Value::Table(child) = table.get_mut(*first)? else {
        return None;
    };
    let taken = take(child, rest)?;
    if child.is_empty() {
        let _removed = table.remove(*first);
    }
    Some(taken)
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
