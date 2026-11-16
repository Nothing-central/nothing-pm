# nothing package manager 📦

**The smart package manager for Node, Bun and Deno**

nothing package manager (CLI: `nth`) is a fast, offline-first package manager that actually thinks before it downloads. it maintains a global local store on your machine so packages you've downloaded once are available forever — no internet required.

---

## why nothing package manager?

every other package manager pings the internet for everything, every time, even if you installed that exact package yesterday on another project. nothing package manager doesn't do that. it favors your local store first and only reaches out to npm or jsr when it absolutely has to.

- **offline first** — works without internet if packages are already in your store
- **smart deduplication** — never downloads the same package twice across projects
- **global local store** — one copy of axios serves every project on your machine
- **many versions at once** — project A keeps libc 1.4.5 while project B uses 2.2.3; both zips stay, neither overwrites the other
- **parallel downloads** — up to 20 simultaneous downloads with safe sequential db writes
- **rollback built in** — every update keeps the previous version, one command to go back
- **clean projects** — your project stays normal, nothing package manager hides its internals away from you
- **registry flexible** — supports npm and jsr, you choose per install
- **lockfile that is a database** — `agree.db` records the exact tree, `nth lock json` exports it
- **npm crate compatibility** — `optionalDependencies`, `peerDependencies` and `os`/`cpu`/`libc` guards are honoured

---

## installation

```bash
# linux
curl -fsSL https://raw.githubusercontent.com/Nothing-central/nothing-pm/main/install.sh | bash

# windows (powershell)
# irm https://raw.githubusercontent.com/Nothing-central/nothing-pm/main/install.ps1 | iex
```

prebuilt binaries ship for linux and windows (x86_64). the script detects the
machine, downloads the matching build, verifies it against the release
checksums and installs `nth` to `/usr/local/bin` when run as root, or
`~/.local/bin` otherwise.

build from source instead:

```bash
cargo build --release
./target/release/nth --help
```

---

## quick start

```bash
# install a package into current project
nth install axios

# install from jsr
nth install jsr @hono/hono

# install multiple at once
nth install axios, express, elysia

# mix npm and jsr in one command — just put jsr before the scoped package
nth install axios, express, jsr @hono/hono, jsr @some/lib

# install all deps from package.json
nth i

# stock up before going offline
nth add axios, express, baileys, elysia

# stock up with mixed registries
nth add axios, express, baileys, jsr @hono/hono, jsr @some/lib
```

---

## how it works

nothing package manager maintains a global store at `/local/` on your machine:

```
/local/
  nothingdatabase/
    nothingda.db          ← metadata for every version ever downloaded
  libraries/
    axios/
      2.8.6.zip           ← pure npm code, untouched
      2.3.4.zip           ← another project needs this one; both stay
    express/
      4.18.2.zip
    react/
      18.3.1.zip          ← lives here independently, not inside nextjs
    nextjs/
      14.0.0.zip

    @next/                ← scoped packages get their own scope folder
      server/
        14.0.0.zip
      env/
        14.0.0.zip

    @hono/
      hono/
        4.6.3.zip
```

every package *version* is its own independent zip. scoped packages like `@next/server` live under `@next/server/<version>.zip`. nothing is ever bundled together.

this is what lets two projects disagree. project one can need `libc 2.2.3` while project two needs `libc 1.4.5`, and the store holds `libc/2.2.3.zip` and `libc/1.4.5.zip` side by side. neither can overwrite the other, neither has to re-download, and installing one never disturbs the other.

the db marks exactly one version per package as `active` — the one the last install chose — and remembers the previous one for `nth clear <pkg> new`. every other version is kept and shown as `kept` in `nth view db`.

when nextjs and your other project both need react, there is still only one `react/18.3.1.zip`. nothing package manager fetches react once and every project on your machine uses that same zip. deleting nextjs never touches react. everything is cleanly isolated.

the zips contain pure upstream package code exactly as the registry serves it. nothing package manager metadata lives only in `nothingda.db`, never inside the zips themselves.

when you install a package nothing package manager checks the store first. if the package is there and the version satisfies your package.json requirement, it copies it straight to your node_modules. no internet. no waiting. done.

only when a package is completely missing from the store does nothing package manager go online to fetch it.

**the internet is opt-in, not opt-out.** nothing package manager will never automatically ping npm to check if a newer version exists. not when you are online. not ever. if axios 2.2.4 dropped five minutes ago nothing package manager does not care and does not check. your store has 2.2.3, your project gets 2.2.3, end of story. if you want nothing package manager to look for updates you tell it explicitly:

```bash
nth update database      ← update everything in store
nth check updates        ← just see what is available, download nothing
nth install axios@2.2.4  ← fetch a specific version you know exists
```

any other time nothing package manager keeps its hands off the internet completely.

---

## commands

| command | description |
|---|---|
| `nth install <pkg>` | install package into current project |
| `nth install jsr <pkg>` | install from jsr registry |
| `nth install axios, jsr @hono/hono` | mix npm and jsr in one command |
| `nth i` | install all deps from package.json |
| `nth i --forceall` | CI mode — no prompts, strict versions, no db writes |
| `nth uninstall <pkg>` | remove a package from this project (stays in the store) |
| `nth add <pkg>, <pkg>` | download packages to store without installing |
| `nth add axios, jsr @hono/hono` | stock up from mixed registries in one command |
| `nth update database` | check all stored packages for updates |
| `nth check updates` | see what updates are available without installing |
| `nth view db` | see every version in your local store, and which is active |
| `nth delete <pkg>` | remove a package from the store completely |
| `nth clear <pkg> prev` | delete the previous version of a package |
| `nth clear <pkg> new` | roll back to previous version |
| `nth clear <pkg>` | remove all versions of a package |
| `nth info` | generate nothing-info.json for current project |
| `nth lock` | show agree.db, the project lockfile |
| `nth lock json` | write agree.json, the same lockfile as json |

---

## offline behavior

when nothing package manager detects no internet connection:

```
nth i

it appears you are offline
scanning local store for required packages...

✓ axios 2.2.3     found in store, version matches
✓ express 4.18.2  found in store, version matches
⚠ elysia 1.0.0   found in store, locked version differs (store has 1.2.0)
  use current version 1.2.0? (y/n):
```

---

## conflict resolution

when two packages need different versions of the same dependency nothing package manager tells you exactly what it costs:

```
⚠ dependency conflict detected

  nextjs requires react 18.9.8
  some-old-lib requires react 15.0.0

  1. use latest for all (react 18.9.8) — some-old-lib might break, saves 37mb
  2. download both versions — everything works, costs extra 37mb in node_modules

choose (1/2):
```

---

## the lockfile — `agree.db`

nothing package manager's lockfile is a SQLite database called `agree.db`, written next to your `package.json`. it is not a text file full of indentation: it is the same format the store already uses, so it opens in `sqlite3`, needs no parser of its own, and stays small with thousands of packages.

one thing it is **not**: line-diffable. a SQLite file is a binary; every rewrite touches pages throughout it, so `git diff` shows you «binary files differ» and nothing useful. that is exactly what `nth lock json` is for — commit `agree.json` when you want a lockfile reviewers can read and diff, and keep `agree.db` when you want the machine copy `nth i` replays without parsing anything.

it holds the whole agreed tree: every root range from package.json, every resolved package, its version, source, tarball URL, integrity hash and checksum, its dependencies, and which optional packages were skipped for this platform.

```bash
nth install axios     # writes package.json and agree.db
nth i                 # replays agree.db when it still matches package.json
nth lock              # show what is locked
nth lock json         # write agree.json, the same lockfile as json
```

on the next `nth i` the lockfile is consulted first. if the ranges in package.json are still the ones that produced the lock, the exact locked tree is installed — no resolution, no prompts, no registry call when the packages are in the store:

```
nth i

agree.db matches package.json — installing the locked versions

✓ axios 2.2.3     in store
✓ follow-redirects 1.15.6 in store

2 packages installed
```

if a range in package.json changed, the lock is reported as stale and only the moved parts are resolved again before the lock is rewritten. deleting `agree.db` is always safe: `nth i` will build a new one.

`agree.json` looks like this (`nth lock json`):

```json
{
  "lockfileVersion": 1,
  "generatedBy": "nth 0.2.0",
  "roots": { "esbuild": "^0.28.2" },
  "packages": {
    "esbuild": {
      "version": "0.28.2",
      "source": "npm",
      "tarball": "https://registry.npmjs.org/esbuild/-/esbuild-0.28.2.tgz",
      "integrity": "sha512-HKVLS8dvI...",
      "checksum": "sha256:5abb3b49...",
      "optional": false,
      "nestedUnder": null,
      "dependencies": {
        "@esbuild/linux-x64": { "range": "0.28.2", "version": "0.28.2", "optional": true }
      }
    }
  }
}
```

`--forceall` never writes `agree.db` (or the store), so CI stays untouched.

---

## optional, peer and platform dependencies

nothing package manager follows npm's rules for the awkward dependency kinds that break naive installers:

- **`optionalDependencies`** are installed when possible. if the version does not exist, needs another platform, or the network is down, nothing package manager warns and keeps going instead of failing the install.
- **`os` / `cpu` / `libc` guards** are checked against the machine (`linux`/`darwin`/`win32`, `x64`/`arm64`, `glibc`/`musl`). this is what makes `esbuild`, `rollup`, `@swc/core` and `lightningcss` work: twenty `@esbuild/*` optional packages are skipped and the one for this machine is installed.
- **`peerDependencies`** are installed alongside the package that declares them, like npm 7+. peers marked optional in `peerDependenciesMeta` are left alone.
- asking for a platform-specific package directly (`nth install fsevents` on linux) is a clear error, not a silent no-op.

```bash
nth install esbuild

⚠ 25 optional package(s) skipped: built for a different platform (linux/x64 (glibc))
✗ esbuild 0.28.2            not in store, fetching...
✗ @esbuild/linux-x64 0.28.2 not in store, fetching...

✓ 2 packages installed
```

---

## nth info

run inside any project where you have used nothing package manager to install dependencies:

```bash
nth info
```

generates a `nothing-info.json` in your project root containing every package, every nested dependency, and every checksum pulled straight from `nothingda.db`:

```json
{
  "project": "my-awesome-project",
  "generated": "2024-11-15T10:32:00Z",
  "packages": [
    {
      "name": "axios",
      "version": "2.2.3",
      "source": "npm",
      "checksum": "sha256:a3f8c2e1d4b7f9...",
      "dependencies": [
        {
          "name": "follow-redirects",
          "version": "1.15.0",
          "checksum": "sha256:9c4e1b2d7f3a8..."
        }
      ]
    },
    {
      "name": "@hono/hono",
      "version": "4.0.0",
      "source": "jsr",
      "checksum": "sha256:f2a9c4e8b1d3...",
      "dependencies": []
    }
  ]
}
```

useful for auditing exactly what your project is using and verifying checksums against what is in the store. the data comes entirely from the db — no network requests, no scanning node_modules.

---

## ci mode — `--forceall`

> ⚠ this flag is dangerous and not recommended for normal development. it exists exclusively for CI automation environments.

```bash
nth i --forceall
```

**what it does:**
- no prompts, no questions, no y/n, no conflict menus — just installs and moves on
- checks the db but only for **explicit exact version matches** — if package.json says `1.1.2` and the store has `1.1.1` that is a miss, it fetches fresh regardless of what semver would normally allow
- strict version matching — `^1.1.1` and `^1.1.2` are treated as different even if semver would normally allow it, whatever package.json says is law
- **never writes to the db** — the store is read only in this mode, nothing is saved, when the pipeline is done nothing has no memory of it

**why it exists:**

in a CI environment there is nobody sitting at a keyboard to answer prompts. `--forceall` lets automated pipelines like github actions or any codecat system run `nth i --forceall` and get a clean install every time with no human interaction required.

```
nth i --forceall

checking store for explicit version matches...
✓ react 18.2.0      exact match in store, using local
✓ axios 2.2.3       exact match in store, using local
✗ elysia 1.1.2      store has 1.1.1, not exact, fetching...
✗ nextjs 14.0.0     not in store, fetching...

2 from store, 2 fetching...
elysia 1.1.2        ██████████ done ✓
nextjs 14.0.0       ██████████ done ✓

47 packages installed
⚠ forceall mode: nothing was saved to store
```

do not use this in normal development. your local store is what makes nothing package manager fast and smart. `--forceall` throws all of that away on purpose.

---

## built with

- **Rust** — fast, safe, no runtime dependency
- **SQLite** — lightweight local metadata storage
- **semver** — reimplemented in Rust for version resolution

---

## license

MIT
