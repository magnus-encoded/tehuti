use core::error::Error;
use std::{
    io::{self, Write},
    process::Command,
};

fn output(program: &str, arguments: &[&str]) -> Result<String, Box<dyn Error>> {
    let result = Command::new(program).args(arguments).output()?;
    if !result.status.success() {
        return Err(format!("{program} failed: {}", result.status).into());
    }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut stdout = io::stdout().lock();
    for reference in ["HEAD", "packed-refs"] {
        let path = output("git", &["rev-parse", "--git-path", reference])?;
        writeln!(stdout, "cargo:rerun-if-changed={path}")?;
    }
    let reference = output("git", &["rev-parse", "--symbolic-full-name", "HEAD"])?;
    if reference != "HEAD" {
        let path = output("git", &["rev-parse", "--git-path", &reference])?;
        writeln!(stdout, "cargo:rerun-if-changed={path}")?;
    }
    let hash = output("git", &["rev-parse", "--short=9", "HEAD"])?;
    let date = output("date", &["-u", "+%Y-%m-%d"])?;
    writeln!(stdout, "cargo:rustc-env=TEHUTI_GIT_HASH={hash}")?;
    writeln!(stdout, "cargo:rustc-env=TEHUTI_BUILD_DATE={date}")?;
    Ok(())
}
