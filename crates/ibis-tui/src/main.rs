#![expect(
    clippy::multiple_crate_versions,
    reason = "Ratatui requires older rustix and unicode-width alongside workspace dependencies; its macros also use syn 3"
)]
use core::error::Error;
use core::time::Duration;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ibis_commands::{Environment, Input, Screen, State, specs, step};
use ibis_tui::InputLine;
use ibis_tui::agent::{Session, Socket, socket_path};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use std::{path::Path, time::Instant};

fn environment(cwd: &Path) -> Environment {
    let mut environment = Environment::default();
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            if let Ok(entries) = std::fs::read_dir(path) {
                environment.programs.extend(
                    entries
                        .flatten()
                        .map(|entry| entry.file_name().to_string_lossy().into_owned()),
                );
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(cwd) {
        environment.names.extend(
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned()),
        );
    }
    environment.programs.sort();
    environment.programs.dedup();
    environment
}

fn powerline(parts: impl IntoIterator<Item = (String, bool)>) -> Line<'static> {
    let mut spans = Vec::new();
    for (label, focused) in parts {
        let background = if focused { Color::Cyan } else { Color::DarkGray };
        spans.push(Span::styled(
            format!(" {} ", clean(&label)),
            Style::default().fg(Color::Black).bg(background),
        ));
        spans.push(Span::styled("", Style::default().fg(background)));
    }
    Line::from(spans)
}

fn draw(frame: &mut Frame<'_>, screen: &Screen, pending: &str, scroll: u16) {
    let [history_area, preview_area, builder_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2), Constraint::Length(10)])
            .areas(frame.area());
    let mut history = Vec::new();
    for (index, row) in screen.history.iter().enumerate() {
        history.push(powerline([(
            format!("$ {}  [exit {}]", row.text, row.exit),
            screen.history_focus == screen.history.len().saturating_sub(index),
        )]));
        history.extend(row.output.lines().map(|line| Line::raw(clean(line))));
    }
    frame.render_widget(
        Paragraph::new(history)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        history_area,
    );
    frame.render_widget(
        Paragraph::new(format!("$ {}\nsay> {pending}", clean(&screen.preview)))
            .style(Style::default().fg(Color::Yellow)),
        preview_area,
    );
    let mut builder = vec![powerline(
        screen
            .segments
            .iter()
            .map(|segment| (clean(&segment.label), segment.focused)),
    )];
    builder.push(Line::raw(clean(&screen.description)));
    let start = screen.list_focus.saturating_sub(2);
    for (index, candidate) in screen.list.iter().enumerate().skip(start).take(4) {
        builder.push(Line::styled(
            format!(
                "{} {:?}  {}  {}",
                if index == screen.list_focus { "▶" } else { " " },
                candidate.confidence,
                clean(&candidate.value),
                clean(&candidate.note)
            ),
            Style::default().fg(Color::Green),
        ));
    }
    builder.push(Line::styled(
        if screen.ready {
            "READY: A / Enter runs"
        } else {
            "Fill empty slots; Y starts a command"
        },
        Style::default().add_modifier(Modifier::BOLD),
    ));
    builder.push(Line::raw(clean(&screen.notice)));
    builder.push(Line::raw(
        "F1 A  F2 B  F3 X  F4 Y  F5 LB  F6 RB | F7–F10: LS | Ctrl-C: quit",
    ));
    frame.render_widget(Paragraph::new(builder), builder_area);
}

fn clean(text: &str) -> String {
    text.chars().filter(|character| !character.is_control()).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let cwd = std::env::current_dir()?;
    let mut state = State::new(cwd.to_string_lossy().into_owned(), specs()?);
    (state, _) = step(state, Input::Environment(environment(&cwd)));
    let mut socket = Socket::bind(&socket_path(
        std::env::var_os("IBIS_TUI_SOCKET"),
        std::env::var_os("XDG_RUNTIME_DIR"),
    )?)?;
    let mut terminal = ratatui::try_init()?;
    let result = session(&mut terminal, state, &cwd, &mut socket);
    ratatui::restore();
    result
}

fn session(
    terminal: &mut ratatui::DefaultTerminal,
    initial: State,
    cwd: &Path,
    socket: &mut Socket,
) -> Result<(), Box<dyn Error>> {
    let mut session = Session::new(initial, cwd.to_owned());
    let mut line = InputLine::default();
    let mut last_key = Instant::now();
    let mut scroll = 0_u16;
    loop {
        let previous_history = session.state().history().len();
        socket.poll(&mut session)?;
        if session.state().history().len() != previous_history {
            scroll = history_lines(session.state());
        }
        let state = session.state();
        let _frame = terminal.draw(|frame| draw(frame, &state.screen(), line.text(), scroll))?;
        let input = if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    last_key = Instant::now();
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        return Ok(());
                    }
                    if scroll_key(key.code, &mut scroll, state) {
                        None
                    } else {
                        line.key(key)
                    }
                }
                Event::Key(_)
                | Event::FocusGained
                | Event::FocusLost
                | Event::Mouse(_)
                | Event::Paste(_)
                | Event::Resize(_, _) => None,
            }
        } else {
            line.quiet(last_key.elapsed())
        };
        if let Some(input) = input {
            let history = session.state().history().len();
            session.input(input);
            if session.state().history().len() != history {
                scroll = history_lines(session.state());
            }
        }
    }
}

fn scroll_key(code: KeyCode, scroll: &mut u16, state: &State) -> bool {
    if matches!(code, KeyCode::PageUp) {
        *scroll = scroll.saturating_sub(3);
    } else if matches!(code, KeyCode::PageDown) {
        *scroll = scroll.saturating_add(3);
    } else if matches!(code, KeyCode::Home) {
        *scroll = 0;
    } else if matches!(code, KeyCode::End) {
        *scroll = history_lines(state).saturating_sub(3);
    } else {
        return false;
    }
    true
}

fn history_lines(state: &State) -> u16 {
    u16::try_from(
        state
            .history()
            .iter()
            .map(|row| row.output.lines().count().saturating_add(1))
            .sum::<usize>(),
    )
    .unwrap_or(u16::MAX)
}
