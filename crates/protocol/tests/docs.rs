//! The documented examples parse with the current parsers.

#[cfg(test)]
mod docs {
    use tehuti_protocol::{Action, Message, parse_context, parse_manifest, parse_message};

    const CONTEXT_GUIDE: &str = include_str!("../../../docs/contexts.md");
    const PROTOCOL_GUIDE: &str = include_str!("../../../docs/protocol/README.md");

    /// The bodies of the fenced blocks tagged `language`.
    fn blocks<'a>(source: &'a str, language: &str) -> Vec<&'a str> {
        let opening = format!("```{language}\n");
        source
            .split(opening.as_str())
            .skip(1)
            .map(|rest| rest.split_once("```").expect("fenced block is closed").0)
            .collect()
    }

    /// A context file, a shared section or a fragment of one, made complete.
    fn as_context(block: &str) -> String {
        let body = block
            .lines()
            .filter(|line| !line.trim_start().starts_with("include"))
            .fold(String::new(), |mut text, line| {
                text.push_str(line);
                text.push('\n');
                text
            });
        if body.lines().any(|line| line.starts_with("app_id")) {
            body
        } else {
            let mut complete = String::from("app_id = \"org.example.App\"\n");
            complete.push_str(&body);
            complete
        }
    }

    #[test]
    fn every_toml_example_in_the_docs_parses() {
        let mut contexts = 0;
        for guide in [CONTEXT_GUIDE, PROTOCOL_GUIDE] {
            for block in blocks(guide, "toml") {
                if block.contains("[[destinations]]")
                    || block.contains("[[entry]]")
                    || block.contains("[remote]")
                {
                    let _table: toml::Table =
                        block.parse().unwrap_or_else(|error| panic!("{error}\n{block}"));
                    continue;
                }
                let _context =
                    parse_context(&as_context(block)).unwrap_or_else(|error| panic!("{error}\n{block}"));
                contexts += 1;
            }
        }
        assert!(contexts >= 15, "found only {contexts} context examples");
    }

    #[test]
    fn shipped_example_contexts_parse() {
        for (name, source) in [
            (
                "emulator",
                include_str!("../../../examples/emulator/context.toml"),
            ),
            ("foot", include_str!("../../../examples/foot/context.toml")),
            ("global", include_str!("../../../examples/global/context.toml")),
            ("mpv", include_str!("../../../examples/mpv/context.toml")),
            ("neovim", include_str!("../../../examples/neovim/context.toml")),
        ] {
            let _context = parse_context(source).unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }

    #[test]
    fn shipped_example_manifest_parses() {
        let manifest = parse_manifest(include_bytes!("../../../examples/plugins/east-to-tab.json")).unwrap();
        assert_eq!(manifest.app_ids, ["test"]);
    }

    #[test]
    fn every_json_example_in_the_protocol_guide_parses() {
        let mut manifests = 0;
        let mut actions = 0;
        let mut messages = 0;
        for block in blocks(PROTOCOL_GUIDE, "json") {
            for line in block.lines().map(str::trim).filter(|line| line.starts_with('{')) {
                if line.contains("\"executable\"") {
                    let _manifest =
                        parse_manifest(line.as_bytes()).unwrap_or_else(|error| panic!("{error}\n{line}"));
                    manifests += 1;
                } else if line.contains("\"jsonrpc\"") {
                    let message =
                        parse_message(line.as_bytes()).unwrap_or_else(|error| panic!("{error}\n{line}"));
                    assert!(matches!(
                        message,
                        Message::Call(_) | Message::Response(_) | Message::Error(_)
                    ));
                    messages += 1;
                } else {
                    let _action: Action =
                        serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}\n{line}"));
                    actions += 1;
                }
            }
        }
        assert!(
            manifests >= 2 && actions >= 7 && messages >= 15,
            "{manifests} {actions} {messages}"
        );
    }
}
