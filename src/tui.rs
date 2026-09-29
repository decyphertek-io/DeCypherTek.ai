//! TUI — the only visible output. Plain ANSI colors + box drawing + wrapping,
//! over any pty from xterm to Termux. dialoguer is used for interactive
//! prompts (selections, passwords, confirmations) in setup and confirmations.

use std::io::IsTerminal;

const RESET: &str = "\x1b[0m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const BOLD: &str = "\x1b[1m";

fn paint(color: &str, text: &str) -> String {
    if std::io::stdout().is_terminal() {
        format!("{color}{text}{RESET}")
    } else {
        text.to_string()
    }
}

pub fn term_width() -> usize {
    // Term::size() returns (rows, cols).
    let (_, cols) = console::Term::stdout().size();
    if cols == 0 {
        78
    } else {
        (cols as usize).min(100)
    }
}

/// Draw a boxed, wrapped panel with a title in the top rule.
pub fn panel(title: &str, body: &str, color: &str) {
    let width = term_width();
    let inner = width.saturating_sub(4); // total inner area, incl. border pads
    let text_w = inner.saturating_sub(4); // "│ " + text + " │"
    let title_len = title.chars().count() + 4;
    let top = format!("┌─ {title} {}", "─".repeat(inner.saturating_sub(title_len)));
    println!("{}", paint(color, &top));
    for line in crate::util::wrap_text(body, text_w) {
        let pad = text_w.saturating_sub(line.chars().count());
        println!(
            "{} {}{} {}",
            paint(color, "│"),
            line,
            " ".repeat(pad),
            paint(color, "│")
        );
    }
    println!("{}", paint(color, &format!("└{}┘", "─".repeat(inner))));
}

pub fn report(title: &str, body: &str) {
    panel(title, body, CYAN);
}

pub fn info(title: &str, body: &str) {
    panel(title, body, GREEN);
}

pub fn warn(title: &str, body: &str) {
    panel(title, body, YELLOW);
}

pub fn error(body: &str) {
    panel("ERROR", body, RED);
}

pub fn banner(version: &str) {
    let w = term_width();
    println!("{}", paint(CYAN, &format!("╔{}╗", "═".repeat(w - 2))));
    let line = |text: &str| {
        let pad = w.saturating_sub(4 + text.chars().count());
        println!(
            "{}{}{}{}",
            paint(CYAN, "║ "),
            paint(BOLD, &paint(CYAN, text)),
            " ".repeat(pad),
            paint(CYAN, " ║")
        );
    };
    line(&format!("DeCypherTek.ai  v{version}"));
    line("The agent that runs anywhere. Decoding technology, so you don't have to.");
    println!("{}", paint(CYAN, &format!("╚{}╝", "═".repeat(w - 2))));
}

/// Prompt for the @-shell: a classic terminal look — `decyphertek.ai:~$ `.
pub fn draw_prompt(cwd: &str) {
    print!(
        "{}:{}$ ",
        paint(GREEN, "decyphertek.ai"),
        paint(CYAN, cwd),
    );
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    #[test]
    fn panel_smoke() {
        super::panel(
            "t",
            "hello world, this line is long enough to wrap around nicely in a narrow terminal",
            "cyan",
        );
        super::banner("0.1.0-test");
    }
}
