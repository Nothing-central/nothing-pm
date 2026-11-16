//! nothing — smart offline-first package manager for Node, Bun and Deno.
//!
//! The binary is `nth`. Only two things here are not hand written: TLS/HTTP
//! (ureq) and SQLite (rusqlite). JSON, semver, gzip/inflate, tar, zip,
//! SHA-256/512, base64 and the CLI parser all live in this crate.

mod base64;
mod cli;
mod commands;
mod config;
mod db;
mod hash;
mod http;
mod inflate;
mod install;
mod json;
mod lockfile;
mod parallel;
mod project;
mod registry;
mod resolver;
mod semver;
mod store;
mod tar;
#[cfg(test)]
mod testutil;
mod ui;
mod zipstore;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let code = match cli::parse(&args) {
        Ok(command) => match commands::dispatch(command) {
            Ok(()) => 0,
            Err(message) => {
                eprintln!("{} {message}", ui::CROSS);
                1
            }
        },
        Err(message) => {
            eprintln!("{} {message}", ui::CROSS);
            eprintln!();
            println!("{}", commands::HELP);
            1
        }
    };

    std::process::exit(code);
}
