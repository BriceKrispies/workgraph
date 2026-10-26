//! The console width rich gives workgraph's output and typer's panels on a legacy Windows
//! console (rich's `Console.legacy_windows`), against the widths the Python CLI prints there.

#![allow(clippy::expect_used)]

use wg_render::{Console, ConsoleEnv};

fn env<'a>(columns: Option<&'a str>, is_terminal: bool, legacy: bool) -> ConsoleEnv<'a> {
    ConsoleEnv {
        columns,
        term: None,
        no_color: None,
        is_terminal,
        terminal_width: None,
        legacy_windows: legacy,
    }
}

// covers: E12
#[test]
fn columns_loses_the_legacy_column_whether_or_not_it_is_a_terminal() {
    // `task list | cat` at COLUMNS=60 printed 59 cells on the legacy console.
    let piped = Console::from_env(&env(Some("60"), false, true));
    assert_eq!(piped.width(), 59);
    let terminal = Console::from_env(&env(Some("60"), true, true));
    assert_eq!(terminal.width(), 59);
    // Without a legacy console nothing is taken off.
    assert_eq!(
        Console::from_env(&env(Some("60"), false, false)).width(),
        60
    );
}

// covers: E12
#[test]
fn an_unset_width_loses_the_column_only_on_a_terminal() {
    // `task list | cat` with no COLUMNS printed the 80-cell table on the legacy console.
    let piped = Console::from_env(&env(None, false, true));
    assert_eq!(piped.width(), 80);
    // The usage panel is typer's plain console, which always takes the column: 79 cells.
    assert_eq!(piped.plain_width(), 79);
    let terminal = Console::from_env(&env(None, true, true));
    assert_eq!(terminal.width(), 79);
    assert_eq!(terminal.plain_width(), 79);
}

// covers: E12
#[test]
fn the_usage_panel_follows_columns_on_a_legacy_console() {
    // typer's panel at COLUMNS=60 and COLUMNS=100 printed 59 and 99 cells.
    assert_eq!(
        Console::from_env(&env(Some("60"), false, true)).plain_width(),
        59
    );
    assert_eq!(
        Console::from_env(&env(Some("100"), false, true)).plain_width(),
        99
    );
}
