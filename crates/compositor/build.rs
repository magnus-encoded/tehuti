pub mod build_metadata;

use core::error::Error;
use std::{
    io::{self, Write},
    process::Command,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut stdout = io::stdout().lock();
    let (hash, paths) = build_metadata::git_metadata(|arguments| {
        let result = Command::new("git").args(arguments).output().ok()?;
        if !result.status.success() {
            return None;
        }
        Some(String::from_utf8(result.stdout).ok()?.trim().to_owned())
    });
    for path in paths {
        writeln!(stdout, "cargo:rerun-if-changed={path}")?;
    }
    writeln!(stdout, "cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH")?;
    let date = build_metadata::build_date(std::env::var("SOURCE_DATE_EPOCH").ok().as_deref());
    writeln!(stdout, "cargo:rustc-env=TEHUTI_GIT_HASH={hash}")?;
    writeln!(stdout, "cargo:rustc-env=TEHUTI_BUILD_DATE={date}")?;
    Ok(())
}
