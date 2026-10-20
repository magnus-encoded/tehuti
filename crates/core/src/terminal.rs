//! Whether a terminal's shell has a program in the foreground, from `/proc/<pid>/stat` text.
//!
//! Field layout: <https://man7.org/linux/man-pages/man5/proc_pid_stat.5.html>.

/// The `/proc/<pid>/stat` fields the foreground check reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStat {
    pub pid: i32,
    pub command: String,
    pub parent: i32,
    pub process_group: i32,
    pub tty: i32,
    pub foreground_group: i32,
}

/// Programs that run their own sessions, where the local tty cannot tell what has the foreground.
const OPAQUE_SESSIONS: [&str; 7] = ["tmux", "screen", "zellij", "ssh", "mosh-client", "et", "abduco"];

/// Parses one `/proc/<pid>/stat` line; the command may hold spaces and parentheses.
#[must_use]
pub fn parse_stat(text: &str) -> Option<ProcessStat> {
    let (pid, rest) = text.split_once(" (")?;
    let close = rest.rfind(')')?;
    let command = rest.get(..close)?.to_owned();
    let mut fields = rest.get(close.saturating_add(1)..)?.split_whitespace();
    let _state = fields.next()?;
    let parent = fields.next()?.parse().ok()?;
    let process_group = fields.next()?.parse().ok()?;
    let _session = fields.next()?;
    let tty = fields.next()?.parse().ok()?;
    let foreground_group = fields.next()?.parse().ok()?;
    Some(ProcessStat {
        pid: pid.trim().parse().ok()?,
        command,
        parent,
        process_group,
        tty,
        foreground_group,
    })
}

/// The terminal's only child on a tty; a terminal with several shells (tabs) has none.
#[must_use]
pub fn shell_of(terminal: i32, processes: &[ProcessStat]) -> Option<&ProcessStat> {
    let mut children = processes
        .iter()
        .filter(|process| process.parent == terminal && process.tty != 0);
    let shell = children.next()?;
    children.next().is_none().then_some(shell)
}

/// True only when a program other than the shell provably holds the tty's foreground.
///
/// `leader` is the process whose pid is the shell's foreground group. Inside tmux,
/// screen or ssh, or when the leader is unreadable, this is false.
#[must_use]
pub fn foreground_program(shell: &ProcessStat, leader: Option<&ProcessStat>) -> bool {
    let opaque = |process: &ProcessStat| OPAQUE_SESSIONS.contains(&process.command.as_str());
    if opaque(shell) || shell.foreground_group <= 0 || shell.foreground_group == shell.process_group {
        return false;
    }
    leader.is_some_and(|leader| leader.process_group == shell.foreground_group && !opaque(leader))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn stat(pid: i32, command: &str, parent: i32, process_group: i32, foreground_group: i32) -> ProcessStat {
        ProcessStat {
            pid,
            command: command.into(),
            parent,
            process_group,
            tty: 34816,
            foreground_group,
        }
    }

    #[test]
    fn parses_a_command_holding_spaces_and_parentheses() {
        let line = "4242 (my (odd) shell) S 4000 4242 4242 34816 4300 4194304 1 2 3 4 5 6";
        assert_eq!(
            parse_stat(line),
            Some(ProcessStat {
                pid: 4242,
                command: "my (odd) shell".into(),
                parent: 4000,
                process_group: 4242,
                tty: 34816,
                foreground_group: 4300,
            })
        );
    }

    #[test]
    fn truncated_stat_lines_do_not_parse() {
        assert_eq!(parse_stat("4242 (bash) S 4000 4242"), None);
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn an_idle_shell_owns_its_foreground() {
        let shell = stat(10, "bash", 1, 10, 10);
        assert!(!foreground_program(&shell, Some(&shell)));
    }

    #[test]
    fn a_running_program_holds_the_foreground() {
        let shell = stat(10, "bash", 1, 10, 20);
        assert!(foreground_program(&shell, Some(&stat(20, "make", 10, 20, 20))));
    }

    #[test]
    fn tmux_and_ssh_never_count_as_a_foreground_program() {
        for session in ["tmux", "ssh", "screen"] {
            let shell = stat(10, "bash", 1, 10, 20);
            assert!(!foreground_program(&shell, Some(&stat(20, session, 10, 20, 20))));
            let attached = stat(10, session, 1, 10, 20);
            assert!(!foreground_program(&attached, Some(&stat(20, "vim", 10, 20, 20))));
        }
    }

    #[test]
    fn several_shells_in_one_terminal_cannot_be_told_apart() {
        let tabs = [stat(10, "bash", 1, 10, 10), stat(11, "bash", 1, 11, 11)];
        assert_eq!(shell_of(1, &tabs), None);
        assert_eq!(shell_of(1, &tabs[..1]), tabs.first());
    }

    proptest! {
        #[test]
        fn an_unreadable_or_mismatched_leader_never_interrupts(
            shell_group in 1i32..1000,
            foreground in -1i32..1000,
            leader_group in 1i32..1000,
            readable in any::<bool>(),
        ) {
            let shell = stat(shell_group, "bash", 1, shell_group, foreground);
            let leader = stat(foreground, "vim", shell_group, leader_group, foreground);
            let interrupts = foreground_program(&shell, readable.then_some(&leader));
            prop_assert!(
                !interrupts || (readable && foreground != shell_group && leader_group == foreground),
                "only a readable leader of another foreground group interrupts"
            );
        }
    }
}
