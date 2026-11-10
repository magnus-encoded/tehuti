//! `tehuti --install-skill`: write the embedded write-a-context agent skill.

use std::{
    io,
    path::{Path, PathBuf},
};

const SKILL: &str = include_str!("../../../docs/agents/skill-write-a-context.md");
const SKILL_NAME: &str = "write-a-tehuti-context";
const SOURCE_LINK: &str = "https://github.com/magnus-encoded/tehuti/blob/main/";

const OPENAI_YAML: &str = "\
interface:
  display_name: \"Write a Tehuti context\"
  short_description: \"Write or tune a Tehuti context.toml for one application\"
policy:
  allow_implicit_invocation: true
";

/// The skill text with its name set and its GitHub links pinned to `commit`.
#[must_use]
pub fn render_skill(commit: &str) -> String {
    SKILL
        .replacen("name: write-a-context\n", &format!("name: {SKILL_NAME}\n"), 1)
        .replace(
            SOURCE_LINK,
            &format!("https://github.com/magnus-encoded/tehuti/blob/{commit}/"),
        )
}

/// Where the skill directory lands: `<skills_root>/write-a-tehuti-context`.
#[must_use]
pub fn skill_directory(skills_root: &Path) -> PathBuf {
    skills_root.join(SKILL_NAME)
}

/// Paths written by [`install`], and whether they replaced an earlier install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub skill: PathBuf,
    pub openai_yaml: PathBuf,
    pub updated: bool,
}

/// Writes `SKILL.md` and `agents/openai.yaml` under `skills_root`, overwriting any earlier install.
///
/// # Errors
/// Returns an error when a directory or file cannot be written.
pub fn install(skills_root: &Path, commit: &str) -> io::Result<Installed> {
    let directory = skill_directory(skills_root);
    std::fs::create_dir_all(directory.join("agents"))?;
    let skill = directory.join("SKILL.md");
    let openai_yaml = directory.join("agents/openai.yaml");
    let updated = skill.exists() || openai_yaml.exists();
    std::fs::write(&skill, render_skill(commit))?;
    std::fs::write(&openai_yaml, OPENAI_YAML)?;
    Ok(Installed {
        skill,
        openai_yaml,
        updated,
    })
}

/// `~/.agents/skills` for the given home directory.
#[must_use]
pub fn default_root(home: &Path) -> PathBuf {
    home.join(".agents/skills")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_path_receives_both_files() {
        let root = tempfile::tempdir().unwrap();
        let installed = install(root.path(), "abc123").unwrap();
        assert!(!installed.updated);
        let path = installed.skill;
        assert_eq!(path, root.path().join("write-a-tehuti-context/SKILL.md"));
        let skill = std::fs::read_to_string(&path).unwrap();
        assert!(skill.contains("name: write-a-tehuti-context\n"));
        assert!(!skill.contains("name: write-a-context\n"));
        assert!(skill.contains("/blob/abc123/docs/contexts.md"));
        assert!(!skill.contains("/blob/main/"));
        let yaml =
            std::fs::read_to_string(root.path().join("write-a-tehuti-context/agents/openai.yaml")).unwrap();
        assert!(yaml.contains("display_name:"));
        assert!(yaml.contains("short_description:"));
        assert!(yaml.contains("allow_implicit_invocation: true"));
    }

    #[test]
    fn rerunning_overwrites_and_reports_an_update() {
        let root = tempfile::tempdir().unwrap();
        let first = install(root.path(), "old").unwrap();
        assert!(!first.updated);
        let installed = install(root.path(), "new").unwrap();
        assert!(installed.updated);
        let skill = std::fs::read_to_string(&installed.skill).unwrap();
        assert!(skill.contains("/blob/new/") && !skill.contains("/blob/old/"));
        assert!(installed.openai_yaml.ends_with("agents/openai.yaml"));
    }

    #[test]
    fn default_root_is_under_home() {
        assert_eq!(
            default_root(Path::new("/home/u")),
            PathBuf::from("/home/u/.agents/skills")
        );
    }
}
