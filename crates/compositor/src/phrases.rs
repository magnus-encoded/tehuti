//! Global spoken settings commands loaded at startup.

use tehuti_core::dispatch::Phrases;

fn parse(source: &str) -> Result<Phrases, String> {
    let configuration: serde_json::Value = serde_json::from_str(source).map_err(|error| error.to_string())?;
    let fields = configuration.as_object().ok_or("phrases must be an object")?;
    if fields
        .keys()
        .any(|key| key != "settings" && key != "unlock_wallpapers")
    {
        return Err("unknown voice phrase".into());
    }
    let phrase = |key: &str, default: &'static str| -> Result<String, String> {
        fields.get(key).map_or_else(
            || Ok(default.into()),
            |value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{key} must be a string"))
            },
        )
    };
    Phrases::new(
        &phrase("settings", "pop the hood")?,
        &phrase("unlock_wallpapers", "birds of a feather")?,
    )
}

pub fn from_env() -> Phrases {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")));
    let Some(root) = root else {
        return Phrases::default();
    };
    match std::fs::read_to_string(root.join("tehuti/phrases.json")) {
        Ok(source) => parse(&source).unwrap_or_else(|error| {
            tracing::warn!(%error, "voice phrases invalid; using defaults");
            Phrases::default()
        }),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%error, "voice phrases unavailable");
            }
            Phrases::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use tehuti_core::dispatch::Phrases;

    #[test]
    fn phrase_configuration_validates_types_and_distinct_words() -> Result<(), String> {
        assert_eq!(parse("{}")?, Phrases::default());
        assert_eq!(
            parse(r#"{"settings":"Settings!", "unlock_wallpapers":"All art"}"#)?,
            Phrases::new("settings", "all art")?
        );
        for source in [
            r#"{"settings": 3}"#,
            r#"{"settings":"!"}"#,
            r#"{"settings":"birds of a feather"}"#,
            r#"{"unknown":"words"}"#,
        ] {
            assert!(parse(source).is_err());
        }
        Ok(())
    }
}
