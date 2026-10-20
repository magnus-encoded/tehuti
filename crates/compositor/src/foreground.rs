//! Reads `/proc` to tell whether a terminal's shell has a program in the foreground.

use tehuti_core::terminal::{ProcessStat, foreground_program, parse_stat, shell_of};

/// True only when the terminal with this pid provably runs a program in the foreground.
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate visibility satisfies unreachable_pub for a function the sibling state module calls"
)]
pub(crate) fn runs_program(terminal: i32) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    let processes: Vec<ProcessStat> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("stat")).ok())
        .filter_map(|text| parse_stat(&text))
        .collect();
    let Some(shell) = shell_of(terminal, &processes) else {
        return false;
    };
    let leader = processes
        .iter()
        .find(|process| process.pid == shell.foreground_group);
    foreground_program(shell, leader)
}
