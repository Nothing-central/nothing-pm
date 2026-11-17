//! Command implementations.
//!
//! Shared flow for every command that needs packages: consult the db, plan
//! the tree, download only what is missing, then let the command decide
//! whether to copy into node_modules or just leave it in the store.

pub mod info;
pub mod init;
pub mod install;
pub mod lock;
pub mod store_ops;
pub mod uninstall;
pub mod update;

use crate::cli::{Command, Source};
use crate::config::Store;
use crate::db::Db;
use crate::http::Client;
use crate::install as installer;
use crate::parallel::{self, Downloaded, Job};
use crate::resolver::{Conflict, ConflictChoice, Plan, PlanError, Planner, Root};
use crate::ui;
use std::path::Path;

pub struct Context {
    pub store: Store,
    pub db: Db,
}

pub fn context() -> Result<Context, String> {
    let store = Store::discover()?;
    store
        .ensure()
        .map_err(|e| format!("cannot prepare store at {}: {e}", store.root.display()))?;
    let db = Db::open(&store.db_path())?;
    // Pre-0.2 stores hold one zip per package name; move them to the
    // versioned layout now that the db knows which version they are.
    let _ = crate::store::migrate_legacy(&store, &db);
    Ok(Context { store, db })
}

pub fn dispatch(cmd: Command) -> Result<(), String> {
    match cmd {
        Command::Install {
            specs,
            forceall,
            ignore_scripts,
        } => {
            if forceall {
                install::run_install_all(true, ignore_scripts)
            } else {
                install::run_install(specs, ignore_scripts)
            }
        }
        Command::InstallAll {
            forceall,
            ignore_scripts,
        } => install::run_install_all(forceall, ignore_scripts),
        Command::Uninstall { names } => uninstall::run(names),
        Command::Add { specs } => install::run_add(specs),
        Command::UpdateDatabase => update::run_update(),
        Command::CheckUpdates => update::run_check(),
        Command::ViewDb => store_ops::run_view(),
        Command::Delete { name } => store_ops::run_delete(&name),
        Command::Clear { name, part } => store_ops::run_clear(&name, part),
        Command::Info => info::run(),
        Command::Lock { json } => lock::run(json),
        Command::Init => init::run(),
        Command::Help => {
            println!("{HELP}");
            Ok(())
        }
        Command::Version => {
            println!("nth {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

pub const HELP: &str = "\
nothing package manager — smart offline-first installer for Node, Bun and Deno

usage:
  nth install <pkg>, <pkg>     install packages into this project
  nth install jsr <pkg>        install from the jsr registry
  nth i [--forceall]           install everything in package.json
  nth i --ignore-scripts       install without running package lifecycle scripts
  nth uninstall <pkg>          remove packages from this project
  nth add <pkg>, <pkg>         download to the store without installing
  nth update database          check every stored package and update it
  nth check updates            show available updates, download nothing
  nth view db                  list everything in the store
  nth delete <pkg>             remove a package from the store
  nth clear <pkg> prev         delete the previous version
  nth clear <pkg> new          roll back to the previous version
  nth clear <pkg>              remove every version of a package
  nth info                     write nothing-info.json for this project
  nth lock                     show agree.db, the project lockfile
  nth lock json               write agree.json, the same lockfile as json
  nth init                     create a starter package.json

the store lives in /local (or $HOME/local, or $NTH_STORE when set).
nothing package manager only contacts a registry when a package is missing from the store —
`nth update database` and `nth check updates` are the only opt-in checks.";

/// Plan the full dependency tree and download whatever is missing.
///
/// Returns the plan plus the bytes of everything that was downloaded (the
/// bytes matter to `--forceall`, which must not touch the store).
pub fn ensure_packages(
    ctx: &Context,
    roots_in: &[Root],
    forceall: bool,
) -> Result<(Plan, Vec<Downloaded>), String> {
    let client = Client::new();
    let offline = !crate::http::is_online();
    if offline {
        ui::info("it appears you are offline");
        ui::info("scanning local store for required packages...");
        ui::blank();
    }

    let mut roots = roots_in.to_vec();
    let planner = Planner::new(&ctx.db, &client, forceall, offline);

    let plan = loop {
        let mut decide = |c: &Conflict| -> ConflictChoice {
            if !ui::interactive() {
                ui::info(&format!(
                    "conflict on {}: {} wants {}, keeping {} (non-interactive)",
                    c.name, c.required_by, c.wanted_range, c.chosen
                ));
                return ConflictChoice::Keep;
            }
            ui::blank();
            ui::warn("dependency conflict detected");
            ui::blank();
            ui::info(&format!(
                "  {} requires {} {}",
                c.required_by, c.name, c.wanted_range
            ));
            ui::info(&format!(
                "  {} is already pinned at {}",
                c.name, c.chosen
            ));
            ui::blank();
            let first = format!(
                "use latest for all ({} {}) — {} might break",
                c.name, c.chosen, c.required_by
            );
            let second = "download both versions — everything works, costs extra space".to_string();
            match ui::choose("", &[&first, &second]) {
                Some(2) => ConflictChoice::Both,
                _ => ConflictChoice::Keep,
            }
        };
        let mut on_mismatch = |name: &str, stored: &str, wanted: Option<&str>| -> bool {
            if !ui::interactive() {
                ui::warn(&format!(
                    "{name}: store has {stored}, wanted {} — using the stored version (non-interactive)",
                    wanted.unwrap_or("*")
                ));
                return true;
            }
            ui::blank();
            ui::warn(&format!("{name} found in store, but the version differs"));
            ui::info(&format!("  store has:  {stored}"));
            ui::info(&format!("  wanted:     {}", wanted.unwrap_or("*")));
            ui::confirm(&format!("  use {stored} instead?"))
        };

        match planner.plan(&roots, &mut decide, &mut on_mismatch) {
            Ok(plan) => break plan,
            Err(PlanError::Missing { name, error }) => {
                if !ui::interactive() {
                    return Err(format!("{name}: {} (non-interactive, stopping)", error.message()));
                }
                ui::blank();
                ui::miss(&format!("{name}: {}", error.message()));
                ui::blank();
                ui::info(&format!(
                    "it appears the specified version of {name} is not yet to exist."
                ));
                ui::info("would you like me to keep attempting to fetch it? be sure that");
                ui::info("version is on the registry or nth will be chasing something unreal.");
                ui::blank();
                let latest = match error.latest() {
                    Some(l) => format!("install latest available version instead ({l})"),
                    None => "install latest available version instead".to_string(),
                };
                let options = [
                    "keep trying (retry every 30 seconds)",
                    "stop chasing",
                    &latest,
                ];
                match ui::choose("", &options) {
                    Some(1) => {
                        ui::info("retrying in 30 seconds — ctrl-c to stop");
                        std::thread::sleep(std::time::Duration::from_secs(30));
                        continue;
                    }
                    Some(3) => {
                        for root in roots.iter_mut() {
                            if root.name == name {
                                root.range = None;
                            }
                        }
                        continue;
                    }
                    _ => return Err(format!("stopped: {name}: {}", error.message())),
                }
            }
            Err(e) => return Err(e.message()),
        }
    };

    for warning in &plan.warnings {
        ui::warn(warning);
    }
    for item in plan.primary() {
        if item.in_store {
            ui::status(ui::CHECK, &item.label(), "in store");
        } else {
            ui::status(ui::CROSS, &item.label(), "not in store, fetching...");
        }
    }
    if !plan.conflicts.is_empty() {
        ui::info(&format!(
            "{} dependency conflict(s) resolved",
            plan.conflicts.len()
        ));
    }

    let downloaded = fetch_plan(ctx, &plan, forceall)?;
    Ok((plan, downloaded))
}

/// Download everything in `plan` that is not already in the store.
pub fn fetch_plan(
    ctx: &Context,
    plan: &Plan,
    forceall: bool,
) -> Result<Vec<Downloaded>, String> {
    let client = Client::new();
    let jobs: Vec<Job> = plan
        .to_fetch()
        .iter()
        .map(|item| Job {
            name: item.name.clone(),
            version: item.version.clone(),
            source: item.source,
            tarball: item.tarball.clone().unwrap_or_default(),
            integrity: item.integrity.clone(),
            beta: item.beta,
            deps: item.deps.clone(),
            peers: item.peers.clone(),
        })
        .collect();

    if !jobs.is_empty() {
        ui::blank();
        ui::info(&format!(
            "{} from local store, fetching {}...",
            plan.store_hits(),
            jobs.len()
        ));
        ui::blank();
    }

    let mut downloaded = Vec::new();
    if !jobs.is_empty() {
        let events = ui::Events::new();
        let outcome = parallel::run(
            &ctx.store,
            &client,
            jobs,
            parallel::MAX_WORKERS,
            &ctx.store.db_path(),
            !forceall,
            &events,
        );
        if !outcome.failed.is_empty() {
            for (name, err) in &outcome.failed {
                ui::miss(&format!("{name}: {err}"));
            }
            return Err(format!("{} download(s) failed", outcome.failed.len()));
        }
        downloaded = outcome.done;
    }

    Ok(downloaded)
}

/// The registry a stored package came from, so a replan does not need to ask.
pub fn stored_source(ctx: &Context, name: &str) -> Source {
    ctx.db
        .get(name)
        .ok()
        .flatten()
        .map(|p| p.source)
        .unwrap_or(Source::Npm)
}

/// Copy every planned package into `node_modules`.
pub fn install_plan(
    ctx: &Context,
    plan: &Plan,
    node_modules: &Path,
    downloaded: &[Downloaded],
    forceall: bool,
) -> Result<usize, String> {
    let mut count = 0usize;

    for item in plan.primary() {
        install_one(ctx, item, node_modules, downloaded, forceall)
            .map_err(|e| format!("{}: {e}", item.name))?;
        count += 1;
    }

    // Second copies of conflicted dependencies live under their requester.
    for item in plan.items.iter().filter(|i| i.nested_under.is_some()) {
        let requester = item.nested_under.as_deref().unwrap_or_default();
        if requester.is_empty() {
            continue;
        }
        let nested = installer::package_dir(node_modules, requester).join("node_modules");
        install_one(ctx, item, &nested, downloaded, forceall)
            .map_err(|e| format!("{}: {e}", item.name))?;
        count += 1;
    }

    Ok(count)
}

fn install_one(
    ctx: &Context,
    item: &crate::resolver::PlanItem,
    node_modules: &Path,
    downloaded: &[Downloaded],
    forceall: bool,
) -> Result<(), String> {
    if forceall {
        // Downloaded bytes only: --forceall must not touch the store.
        if let Some(d) = downloaded
            .iter()
            .find(|d| d.name == item.name && d.version == item.version)
        {
            installer::install_bytes(&item.name, &d.zip, node_modules)
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    installer::install_from_store(&ctx.store, &item.name, &item.version, node_modules)
        .map_err(|e| e.to_string())?;
    Ok(())
}
