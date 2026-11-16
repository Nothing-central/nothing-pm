# nothing 📦

**The smart package manager for Node, Bun and Deno**

nothing (CLI: `nth`) is a fast, offline-first package manager that actually thinks before it downloads. it maintains a global local store on your machine so packages you've downloaded once are available forever — no internet required.

---

## why nothing?

every other package manager pings the internet for everything, every time, even if you installed that exact package yesterday on another project. nothing doesn't do that. nothing favors your local store first and only reaches out to npm or jsr when it absolutely has to.

- **offline first** — works without internet if packages are already in your store
- **smart deduplication** — never downloads the same package twice across projects
- **global local store** — one copy of axios serves every project on your machine
- **parallel downloads** — up to 20 simultaneous downloads with safe sequential db writes
- **rollback built in** — every update keeps the previous version, one command to go back
- **clean projects** — your project stays normal, nothing hides its internals away from you
- **registry flexible** — supports npm and jsr, you choose per install
- **no lock file** — package.json is the source of truth, the db handles the rest, your git history stays clean

---

## installation

```bash
curl -fsSL https://raw.githubusercontent.com/Nothing-central/nothing-pm/main/install.sh | bash
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

nothing maintains a global store at `/local/` on your machine:

```
/local/
  nothingdatabase/
    nothingda.db          ← metadata for every package ever downloaded
  libraries/
    axios.zip             ← pure npm code, untouched
    express.zip
    react.zip             ← lives here independently, not inside nextjs
    react-dom.zip
    nextjs.zip            ← just the nextjs core, small on its own
    
    @next/                ← scoped packages get their own folder
      server.zip
      hmr.zip
      env.zip
      types.zip
      
    @hono/
      hono.zip
      
    prev/
      axios.zip           ← previous version, safe for rollback
      express.zip
```

every package is its own independent zip. scoped packages like `@next/server` live in a folder named after their scope. nothing is ever bundled together.

this means when nextjs and your other project both need react, there is only ever ONE `react.zip` in the store. nothing fetches react once and every project on your machine uses that same zip forever. deleting nextjs never touches react. updating react never touches nextjs. everything is cleanly isolated.

the zips contain pure upstream package code exactly as the registry serves it. nothing metadata lives only in `nothingda.db`, never inside the zips themselves.

when you install a package nothing checks the store first. if the package is there and the version satisfies your package.json requirement, it copies it straight to your node_modules. no internet. no waiting. done.

only when a package is completely missing from the store does nothing go online to fetch it.

**the internet is opt-in, not opt-out.** nothing will never automatically ping npm to check if a newer version exists. not when you are online. not ever. if axios 2.2.4 dropped five minutes ago nothing does not care and does not check. your store has 2.2.3, your project gets 2.2.3, end of story. if you want nothing to look for updates you tell it explicitly:

```bash
nth update database      ← update everything in store
nth check updates        ← just see what is available, download nothing
nth install axios@2.2.4  ← fetch a specific version you know exists
```

any other time nothing keeps its hands off the internet completely.

---

## commands

| command | description |
|---|---|
| `nth install <pkg>` | install package into current project |
| `nth install jsr <pkg>` | install from jsr registry |
| `nth install axios, jsr @hono/hono` | mix npm and jsr in one command |
| `nth i` | install all deps from package.json |
| `nth i --forceall` | CI mode — no prompts, strict versions, no db writes |
| `nth add <pkg>, <pkg>` | download packages to store without installing |
| `nth add axios, jsr @hono/hono` | stock up from mixed registries in one command |
| `nth update database` | check all stored packages for updates |
| `nth check updates` | see what updates are available without installing |
| `nth view db` | see everything in your local store |
| `nth delete <pkg>` | remove a package from the store completely |
| `nth clear <pkg> prev` | delete the previous version of a package |
| `nth clear <pkg> new` | roll back to previous version |
| `nth clear <pkg>` | remove all versions of a package |
| `nth info` | generate nothing-info.json for current project |

---

## offline behavior

when nothing detects no internet connection:

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

when two packages need different versions of the same dependency nothing tells you exactly what it costs:

```
⚠ dependency conflict detected

  nextjs requires react 18.9.8
  some-old-lib requires react 15.0.0

  1. use latest for all (react 18.9.8) — some-old-lib might break, saves 37mb
  2. download both versions — everything works, costs extra 37mb in node_modules

choose (1/2):
```

---

## no lock file

nothing has no lock file. not `nothing.lock`, not `package-lock.json`, not anything.

a lock file stores exact versions, checksums and dependency trees. nothing already stores all of that in `/local/nothingdatabase/nothingda.db`. a lock file would just be a copy of data that already exists — completely redundant.

**package.json is the source of truth.** the versions you specify there are exactly what nothing fetches. on a fresh machine with an empty store:

```
nth i

scanning package.json...
✗ axios 2.2.3     not in store, fetching from npm...
✗ elysia 1.1.2    not in store, fetching from npm...
✗ hono 4.0.0      not in store, fetching from npm...
done ✓
```

no lock file needed. nothing reads the versions straight from package.json and fetches exactly those. over time as the machine's store fills up `nth i` gets faster and faster because more packages are already cached locally.

the side effect is your git history stays clean forever:

```bash
git status
  modified: index.ts       ← actual work
  # that's it. no 3000 line package-lock.json diff. no yarn.lock noise.
```

---

## nth info

run inside any project where you have used nothing to install dependencies:

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

do not use this in normal development. your local store is what makes nothing fast and smart. `--forceall` throws all of that away on purpose.

---

## built with

- **Rust** — fast, safe, no runtime dependency
- **SQLite** — lightweight local metadata storage
- **semver** — reimplemented in Rust for version resolution

---

## license

MIT
