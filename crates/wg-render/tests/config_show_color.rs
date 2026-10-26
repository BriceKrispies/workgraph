//! WG-3800: `config show` on a colouring console must write the bytes Python's rich writes.
//!
//! `golden/config_show/force_color.json` holds the Python CLI's output for one sample config,
//! captured with stdout piped and `FORCE_COLOR=1` (and `NO_COLOR=1` for the third field).

use serde_json::Value as Json;
use wg_render::{Console, ConsoleEnv};

struct Golden {
    plain: String,
    force_color: String,
    force_color_no_color: String,
}

fn golden() -> Golden {
    let path = format!(
        "{}/../../golden/config_show/force_color.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let doc: Json = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"));
    let field = |name: &str| {
        doc[name]
            .as_str()
            .unwrap_or_else(|| panic!("golden field {name} is not a string"))
            .to_owned()
    };
    Golden {
        plain: field("plain"),
        force_color: field("force_color"),
        force_color_no_color: field("force_color_no_color"),
    }
}

/// The console `config show` gets when it writes to a terminal (or `FORCE_COLOR` is set).
fn colouring(no_color: Option<&str>) -> Console {
    Console::from_env(&ConsoleEnv {
        columns: Some("80"),
        no_color,
        is_terminal: true,
        ..ConsoleEnv::default()
    })
}

/// Removes SGR sequences (`ESC[...m`), leaving the text they wrapped.
fn strip_sgr(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn force_color_matches_python_bytes() {
    let g = golden();
    assert_eq!(colouring(None).print_json(&g.plain), g.force_color);
}

#[test]
fn no_color_keeps_attributes_and_drops_colours_like_python() {
    let g = golden();
    assert_eq!(
        colouring(Some("1")).print_json(&g.plain),
        g.force_color_no_color
    );
}

#[test]
fn an_empty_no_color_still_colours() {
    let g = golden();
    assert_eq!(colouring(Some("")).print_json(&g.plain), g.force_color);
}

#[test]
fn stripping_the_escapes_gives_the_plain_text() {
    let g = golden();
    assert_eq!(strip_sgr(&g.force_color), g.plain);
}

#[test]
fn a_console_that_is_not_a_terminal_writes_plain_text() {
    let g = golden();
    let piped = Console::from_env(&ConsoleEnv {
        columns: Some("80"),
        is_terminal: false,
        ..ConsoleEnv::default()
    });
    assert_eq!(piped.print_json(&g.plain), g.plain);
    assert!(!g.plain.contains('\u{1b}'));
}
