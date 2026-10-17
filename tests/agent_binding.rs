// SPDX-License-Identifier: MPL-2.0
#[cfg(test)]
mod tests {
    use core::error::Error;
    use ibis_commands::State;
    use std::fs;
    use std::process::{Command, Output};
    use std::time::Instant;
    use tempfile::TempDir;

    type Result<T> = core::result::Result<T, Box<dyn Error>>;

    fn call(home: &TempDir, args: &[&str]) -> Result<Output> {
        Ok(Command::new(env!("CARGO_BIN_EXE_ibis-commands-ctl"))
            .args(args)
            .env("TB_HOME", home.path())
            .env("TB_STATE", home.path().join("state.json"))
            .env("TB_LOG", home.path().join("inputs.jsonl"))
            .output()?)
    }

    #[test]
    fn cli_persists_trees_runs_in_the_working_folder_and_copies_history() -> Result<()> {
        let home = TempDir::new()?;
        fs::write(home.path().join("two words.txt"), "first\nsecond\n")?;
        for args in [
            vec!["press", "Y"],
            vec!["say", "cat"],
            vec!["press", "A"],
            vec!["type", "two words.txt"],
            vec!["press", "A"],
        ] {
            let output = call(&home, &args)?;
            assert!(
                output.status.success(),
                "CLI failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let state: State = serde_json::from_slice(&fs::read(home.path().join("state.json"))?)?;
        assert!(state.valid(), "saved state is valid");
        assert_eq!(
            state.history().last().map(|row| row.output.as_str()),
            Some("first\nsecond\n"),
            "execution preserves a path containing spaces"
        );
        let screen = String::from_utf8(call(&home, &["press", "LS_UP", "A"])?.stdout)?;
        for section in [
            "HISTORY", "PREVIEW", "BUILDER", "FOCUS", "LIST", "STATUS", "NOTICE",
        ] {
            assert!(screen.contains(section), "text screen contains {section}");
        }
        let copied: State = serde_json::from_slice(&fs::read(home.path().join("state.json"))?)?;
        assert!(
            copied.draft().ready().is_some(),
            "history acceptance copies an editable tree"
        );
        assert!(call(&home, &["reset"])?.status.success(), "reset succeeds");
        Ok(())
    }

    #[test]
    fn cli_times_out_the_command_process_group() -> Result<()> {
        let home = TempDir::new()?;
        assert!(
            call(&home, &["press", "Y"])?.status.success(),
            "new draft succeeds"
        );
        assert!(
            call(&home, &["type", "sleep 30"])?.status.success(),
            "sleep draft succeeds"
        );
        let started = Instant::now();
        assert!(
            call(&home, &["press", "A"])?.status.success(),
            "timeout is a recorded command result"
        );
        assert!(
            started.elapsed().as_secs() < 15,
            "execution is bounded by the ten second timeout"
        );
        let state: State = serde_json::from_slice(&fs::read(home.path().join("state.json"))?)?;
        assert_eq!(
            state.history().last().map(|row| row.exit),
            Some(124),
            "timeout records status 124"
        );
        Ok(())
    }

    #[test]
    fn cli_rejects_corrupt_state_and_unknown_buttons() -> Result<()> {
        let home = TempDir::new()?;
        assert!(
            !call(&home, &["press", "BOGUS"])?.status.success(),
            "unknown buttons are rejected"
        );
        fs::write(home.path().join("state.json"), "{broken")?;
        assert!(
            !call(&home, &["screen"])?.status.success(),
            "corrupt persisted state is rejected"
        );
        Ok(())
    }
}
