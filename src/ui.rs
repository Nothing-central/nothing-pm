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
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn confirm(question: &str) -> bool {
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
