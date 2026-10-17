// SPDX-License-Identifier: MPL-2.0
#[cfg(test)]
mod tests {
    use std::path::Path;

    fn visit(directory: &Path, files: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            if matches!(name, "target" | ".git" | "corpus" | "artifacts") {
                continue;
            }
            if path.is_dir() {
                visit(&path, files)?;
            } else {
                files.push(path);
            }
        }
        Ok(())
    }

    #[test]
    fn every_file_is_mpl_and_secondary_compatible() -> std::io::Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        visit(root, &mut files)?;
        let manifest = std::fs::read_to_string(root.join("Cargo.toml"))?;
        assert!(
            manifest.contains("license = \"MPL-2.0\""),
            "crate licence field is MPL-2.0"
        );
        let licence = std::fs::read_to_string(root.join("LICENSE"))?;
        assert!(
            licence.starts_with("Mozilla Public License Version 2.0"),
            "LICENSE holds the MPL-2.0 text"
        );
        for path in files {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if relative == Path::new("LICENSE") {
                continue;
            }
            assert!(
                !text.contains(concat!("Incompatible With ", "Secondary Licenses")),
                "{} must not carry the Exhibit B notice",
                relative.display()
            );
            let data = relative.extension().is_some_and(|extension| extension == "json")
                || relative.ends_with("Cargo.lock")
                || relative.starts_with("proptest-regressions")
                || relative.starts_with(".directory");
            assert!(
                data || text
                    .lines()
                    .take(2)
                    .any(|line| line.contains("SPDX-License-Identifier: MPL-2.0")),
                "{} carries an SPDX MPL-2.0 header",
                relative.display()
            );
        }
        Ok(())
    }
}
