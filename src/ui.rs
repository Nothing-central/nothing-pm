//! Terminal output: the little symbols the README uses, aligned pairs,
//! tables and the few prompts nothing asks.

use std::io::{self, IsTerminal, Write};
use std::sync::Mutex;

pub const CHECK: &str = "✓";
pub const CROSS: &str = "✗";
pub const WARN: &str = "⚠";
pub const ARROW: &str = "→";

pub fn ok(msg: &str) {
    println!("{CHECK} {msg}");
}

pub fn miss(msg: &str) {
    println!("{CROSS} {msg}");
}

pub fn warn(msg: &str) {
    println!("{WARN} {msg}");
}

pub fn info(msg: &str) {
    println!("{msg}");
}

pub fn blank() {
    println!();
}

/// `✓ axios 2.2.3            in store`
pub fn status(symbol: &str, label: &str, detail: &str) {
    println!("{symbol} {:<28} {}", label, detail);
}

pub fn rule(width: usize) {
    println!("{}", "─".repeat(width));
}

pub fn table(headers: &[&str], widths: &[usize], rows: &[Vec<String>]) {
    let mut line = String::new();
    for (i, header) in headers.iter().enumerate() {
        if i == 0 {
            line.push_str(&format!(" {:<width$}", header, width = widths[i]));
        } else {
            line.push_str(&format!("{:<width$}", header, width = widths[i]));
        }
    }
    println!("{line}");

    let total: usize = widths.iter().sum::<usize>() + 1;
    rule(total);
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            let width = widths.get(i).copied().unwrap_or(12);
            if i == 0 {
                line.push_str(&format!(" {:<width$}", cell, width = width));
            } else {
                line.push_str(&format!("{:<width$}", cell, width = width));
            }
        }
        println!("{line}");
    }
    rule(total);
}

/// Human-readable size, always in kb unless it grows past 1024kb.
pub fn size(kb: u64) -> String {
    if kb >= 1024 {
        format!("{:.1}mb", kb as f64 / 1024.0)
    } else {
        format!("{kb}kb")
    }
}

/// Prompts only make sense with a real terminal. In a pipeline or CI run
/// nothing must never sit waiting for input nobody can type.
///
/// `NTH_YES=1` (or `nth i -y`) makes every prompt answer itself — the
/// default/first option is taken without ever reading stdin — so a big
/// install that would normally ask twenty-seven times finishes untouched.
pub fn interactive() -> bool {
    if yes_mode() {
        return false;
    }
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Answer-everything mode: set once by the CLI when `-y`/`--yes` is passed
/// or `NTH_YES` is in the environment.
pub fn set_yes(on: bool) {
    YES.store(on, std::sync::atomic::Ordering::SeqCst);
}

pub fn yes_mode() -> bool {
    YES.load(std::sync::atomic::Ordering::SeqCst)
}

static YES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn confirm(question: &str) -> bool {
    if yes_mode() {
        println!("{question} (y/n): y  [auto, --yes]");
        return true;
    }
    if !interactive() {
        return false;
    }
    print!("{question} (y/n): ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Returns the 1-based index of the chosen option, or None if the answer
/// was not one of them.
pub fn choose(question: &str, options: &[&str]) -> Option<usize> {
    if yes_mode() {
        // Answer-everything mode: take the default (first) option and say
        // so on the line, so the log still shows what was picked.
        if let Some(first) = options.first() {
            println!("{question}\n  chose 1. {first}  [auto, --yes]");
        }
        return Some(1);
    }
    if !interactive() {
        return None;
    }
    println!("{question}");
    for (i, option) in options.iter().enumerate() {
        println!("  {}. {}", i + 1, option);
    }
    print!("choose (1-{}): ", options.len());
    let _ = io::stdout().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return None;
    }
    let n: usize = answer.trim().parse().ok()?;
    if n >= 1 && n <= options.len() {
        Some(n)
    } else {
        None
    }
}

/// Thread-safe one-line-per-event output for the download pool.
pub struct Events {
    lock: Mutex<()>,
}

impl Events {
    pub fn new() -> Events {
        Events {
            lock: Mutex::new(()),
        }
    }

    pub fn line(&self, symbol: &str, label: &str, detail: &str) {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        println!("{symbol} {:<28} {}", label, detail);
        let _ = io::stdout().flush();
    }
}
