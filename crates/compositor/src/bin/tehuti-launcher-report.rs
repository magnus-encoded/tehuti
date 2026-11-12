//! Reads a Tehuti log on stdin and prints how launcher uses ended.
use std::io::{BufRead as _, Write as _};

fn main() -> std::io::Result<()> {
    let uses: Vec<_> = std::io::stdin()
        .lock()
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| tehuti::launcher_log::parse(&line))
        .collect();
    let report = tehuti_core::launcher::Report::from_uses(&uses);
    std::io::stdout()
        .lock()
        .write_all(tehuti::launcher_log::render(&report).as_bytes())
}
