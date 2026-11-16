# nothing — technical specification

**version:** 0.1.0-draft  
**language:** Rust  
**CLI binary:** `nth`

---

## 1. global store structure

nothing maintains one global store for the entire machine. no per-project hidden folders, no dot directories. just a clean `/local/` directory:

```
/local/
  nothingdatabase/
    nothingda.db
  libraries/
    axios.zip
    express.zip
    react.zip
    react-dom.zip
    nextjs.zip
    
    @next/                ← scoped packages get their own folder
      server.zip
      hmr.zip
      env.zip
      types.zip
      
    @hono/
      hono.zip
      
    prev/
      axios.zip
      express.zip
      ...
```

### 1.1 store rules

- `libraries/` holds the current active version of every package as a zip
- `libraries/prev/` holds the most recent previous version of packages that have been updated
- prev only ever holds ONE version back, not full history
- package zips are named by package name only e.g. `axios.zip` not `axios-2.2.3.zip`
- the actual version metadata lives in the db, not the filename
- scoped packages like `@next/server` live in a subfolder named after their scope e.g. `libraries/@next/server.zip`
- every package is its own independent zip, nothing is ever bundled together
- zips contain pure upstream package code exactly as the registry serves it — nothing metadata never goes inside a zip, it lives only in the db
- each project keeps `agree.db`, a SQLite lockfile next to its package.json (section 5)
- package.json is the source of truth for ranges; `agree.db` records exactly what was resolved from them

---

## 2. database schema

the database is SQLite stored at `/local/nothingdatabase/nothingda.db`

### packages table

```sql
CREATE TABLE packages (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT NOT NULL,
  version     TEXT NOT NULL,
  prev_version TEXT,
  source      TEXT NOT NULL DEFAULT 'npm',
  size_kb     INTEGER,
  beta        INTEGER NOT NULL DEFAULT 0,
  installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  updated_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### fields

| field | description |
|---|---|
| `name` | package name e.g. axios |
| `version` | current active version e.g. 2.2.3 |
| `prev_version` | previous version before last update, nullable |
| `source` | npm or jsr |
| `size_kb` | compressed zip size in kb |
| `beta` | whether this is a prerelease version |

---

## 3. CLI commands

### 3.1 `nth install <pkg>`

installs a package into the current project's node_modules

```
nth install axios
nth install jsr @hono/hono
nth install axios, express, elysia
nth install axios, jsr @hono/hono, express, jsr @some/lib
```

the `jsr` keyword applies only to the package name that immediately follows it. everything without `jsr` in front defaults to npm. npm and jsr packages can be freely mixed in a single command.

**parsing rules:**
- `axios` → fetch from npm
- `jsr @hono/hono` → fetch from jsr
- `axios, jsr @hono/hono, express` → axios and express from npm, @hono/hono from jsr

**flow:**
1. parse each package name and its registry (jsr or npm default)
2. check nothingda.db for each package
3. if found and version satisfies requirement → copy zip to node_modules, done
4. if not found → fetch from correct registry → zip → store → copy to node_modules
5. update db with metadata including source field

### 3.2 `nth i`

installs all dependencies listed in the current project's package.json

**flow:**
1. parse package.json dependencies
2. for each dependency check db
3. report what was found locally vs what needs fetching
4. fetch missing packages in parallel (up to 20 simultaneous downloads)
5. db writes happen sequentially in a queue after downloads complete
6. copy all packages to node_modules

**output format:**
```
scanning package.json... found 12 dependencies

checking local store...
✓ react 18.2.0      in store
✓ axios 2.2.3       in store
✗ elysia 1.0.0      not in store, fetching...
✗ hono 4.0.0        not in store, fetching...

10 from local store
fetching 2 from npm...

elysia 1.0.0    ██████████ done ✓
  → 17 deps found, 4 in store, fetching 13...
  → axios         already downloading ✓
  → zod           ████░░░░░░ 40%

hono 4.0.0      ████░░░░░░ 40%
  → 12 deps found, 8 in store, fetching 4...
  → zod           elysia is fetching this, standing by...

all downloads complete
updating database...
arranging folders...
done! ✓
```

### 3.3 `nth add <pkg>, <pkg>`

downloads packages to the global store WITHOUT installing into any project. for stocking up before going offline.

```
nth add axios, elysia, express, baileys
nth add axios, express, baileys, jsr @hono/hono, jsr @some/lib
```

the same `jsr` prefix rule applies here as in `nth install` — put `jsr` before any package name to fetch it from jsr, everything else defaults to npm. mix freely in one command.

behavior is otherwise identical to the install fetch flow but skips the node_modules copy step entirely.

### 3.4 `nth update database`

checks every package in the store against its registry for newer versions

**flow:**
1. read all packages from db
2. ping registry for each package (parallel)
3. for each package with a newer version:
   - move current zip to `prev/`
   - update `prev_version` in db
   - download new version
   - update `version` in db
4. report summary

### 3.5 `nth check updates`

same as update database but READ ONLY. just reports what updates are available without downloading anything. respects your data.

### 3.6 `nth view db`

displays all packages in the local store

```
📦 nothing package store
───────────────────────────────────────────────
 name          version    prev       size    source
───────────────────────────────────────────────────
 axios         2.2.3      2.2.1      124kb   npm
 elysia        1.2.0      1.0.0      89kb    npm
 hono          4.0.0      —          67kb    jsr
 react         18.2.0     —          234kb   npm
───────────────────────────────────────────────────
 4 packages    514kb total
───────────────────────────────────────────────────
```

### 3.7 `nth delete <pkg>`

removes a package completely from the store. deletes zip, prev zip, and db entry.

### 3.8 `nth clear <pkg> prev`

deletes only `prev/pkg.zip`. current version untouched.

### 3.9 `nth clear <pkg> new`

deletes current version. promotes prev version back to `libraries/`. updates db version fields.
if no prev exists, asks user to confirm full deletion.

### 3.10 `nth clear <pkg>`

removes everything. current zip, prev zip, db entry. nuclear option.

### 3.11 `nth lock` / `nth lock json`

`nth lock` prints the project lockfile: every locked package, its version, source, whether it was optional, and which package it is nested under.

`nth lock json` writes `agree.json` next to `agree.db` — the same lockfile as JSON, for scripts, CI checks and readable diffs.

both are read-only with respect to the lockfile; only `nth i` and `nth install` rewrite `agree.db`.

### 3.12 `nth info`

must be run inside a project where nothing has been used to install dependencies.

generates `nothing-info.json` in the project root. reads entirely from `nothingda.db` — no network requests, no node_modules scanning.

output format:

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

includes every top level package and all nested dependencies recursively. checksums come straight from the db and can be used to verify the integrity of what is in node_modules against what nothing stored.

---

## 4. offline detection and behavior

nothing checks for internet connectivity before every operation that would require a network call.

when offline:

```
nth i

it appears you are offline
scanning local store for required packages...

✓ axios 2.2.3     found, version matches
✓ express 4.18.2  found, version matches
⚠ elysia 1.0.0   found but locked version differs
  store has: 1.2.0
  package.json wants: 1.0.0
  use current version 1.2.0? (y/n):
  
  → y: copies 1.2.0 to node_modules
  → n: skips elysia, warns user
```

---

## 5. the lockfile — `agree.db`

every project gets a lockfile called `agree.db`, written in the project root next to `package.json`.

it is a SQLite database, not a text file. the same format as `nothingda.db`, so it opens in any sqlite client, stays compact for thousands of packages and needs no hand-written parser.

### contents

```sql
meta      (key, value)                       -- lockfileVersion, generatedBy
roots     (name, range, source)              -- the package.json ranges that produced the lock
packages  (name, version, source, tarball, integrity, checksum,
           beta, optional, nested_under, deps)
```

`packages` records the complete resolved tree: transitive dependencies, optional packages kept for this platform, second copies of conflicted dependencies (`nested_under`), the tarball URL and npm integrity hash needed to refetch, and a `sha256:` checksum of what actually landed in the store.

### rules

- `nth i` reads `agree.db` first. if `roots` still matches the ranges in package.json, the exact locked tree is installed — no resolution, no prompts, no registry call for anything already in the store
- if any range changed, the lock is stale: nothing resolves again (store first) and rewrites the lock before installing
- deleting `agree.db` is always safe — the next install rebuilds it
- `nth lock` prints the lock; `nth lock json` writes `agree.json`, the same data as JSON, for scripts and diff review
- `nth i --forceall` never writes `agree.db`, matching its promise to leave no trace

the lock is a record of agreement, not a second source of truth: package.json still decides the ranges, and the store db still decides what this machine holds.

---

## 6. internet usage policy

nothing NEVER pings a registry unless:

- a package is not found in the local store
- the user explicitly runs `nth update database` or `nth check updates`
- the user explicitly runs `nth add` or `nth install` with a package not in store

on a normal `nth i` with everything already in store: **zero network requests.**

---

## 7. parallel download system

### download thread pool
- up to 20 simultaneous downloads
- implemented using Rust async (tokio)
- shared download registry: `Arc<Mutex<HashSet<String>>>` tracking in-flight downloads
- before starting any download a thread checks this registry
- if a package is already being downloaded by another thread, the thread waits for it instead of downloading again

### db write queue
- strictly sequential, one write at a time
- implemented as a channel (tokio mpsc)
- downloads send completed package data to the queue
- a single db writer thread processes the queue in order
- user sees: `install completed, updating database with new version metadata and arranging folders`

---

## 8. dependency resolution

### 7.1 recursive resolution

when fetching a package nothing also fetches its dependencies recursively:

```
fetching nextjs 14.0.0
nextjs has 47 deps

1/47 - react 18.2.0       → in store ✓
2/47 - @next/server       → not in store, fetching...
  @next/server has 8 deps
  1/8 - react             → already handled ✓
  2/8 - typescript        → in store ✓
  ...
```

### 7.2 version matching

before using a cached package nothing checks if the cached version satisfies the semver requirement:

- exact match `2.2.3` → must be exactly 2.2.3
- caret `^2.2.3` → 2.x.x where x >= 2.3
- tilde `~2.2.3` → 2.2.x where x >= 3
- if cached version satisfies → use it, no download
- if cached version does not satisfy → fetch correct version

### 7.3 conflict resolution

when two packages require incompatible versions of the same dependency:

```
⚠ dependency conflict detected

  nextjs requires react 18.9.8
  some-old-lib requires react 15.0.0

  1. use latest for all (react 18.9.8)
     → some-old-lib might break
     → saves 37mb in node_modules

  2. download both versions
     → everything works as expected
     → costs extra 37mb in node_modules

choose (1/2):
```

if option 2 is chosen, both versions are stored and cached for future use.

---

## 9. version not found handling

when a user requests a version that does not exist on the registry:

```
nth install baileys@2.2.4

fetching baileys 2.2.4 from npm...
❌ version 2.2.4 does not exist yet on npm

it appears the specified version of baileys is not yet
to exist. would you like me to keep attempting to fetch
it? be sure that version is on npm or you will be
sending nth into a chase that does not exist.

1. keep trying (retry every 30 seconds)
2. stop chasing
3. install latest available version instead (2.2.3)
```

---

## 10. semver implementation

semver is reimplemented from scratch in Rust. reference: node-semver source.

must handle:
- exact versions `1.2.3`
- caret ranges `^1.2.3`
- tilde ranges `~1.2.3`
- wildcard `1.x` `*`
- prerelease tags `1.2.3-beta.1`
- range comparators `>=1.2.7 <1.3.0`
- edge case: `^0.0.x` behaves differently from `^1.x.x`

---

## 11. registry support

| registry | default | how to use |
|---|---|---|
| npm | yes | `nth install axios` |
| jsr | no | `nth install jsr @hono/hono` |

**inline registry mixing:**

the `jsr` keyword is a per-package inline prefix, not a global flag. it applies only to the package name that immediately follows it in the command. this means npm and jsr packages can be freely mixed in any command that accepts package names:

```bash
nth install axios, jsr @hono/hono, express, jsr @some/lib
nth add baileys, jsr @hono/hono, elysia, jsr @some/lib
```

no config files, no global registry setting, no separate commands. just say where each package comes from inline.

registry is stored per package in the db `source` field so offline resolution always knows where a package originally came from.

---

## 12. supported runtimes

nothing produces standard node_modules output compatible with:

- **Node.js** — primary target
- **Bun** — fully compatible, same node_modules structure
- **Deno 2.0+** — compatible via node_modules and npm/jsr support

---

## 13. compression

all packages are stored as zip files in the global store. zip is per package not per store. this keeps individual package operations clean and isolated.

extraction happens at install time when copying to node_modules.

---

## 14. built with

| component | technology |
|---|---|
| language | Rust |
| async runtime | tokio |
| database | SQLite (compiled in) |
| compression | zip (Rust stdlib) |
| semver | custom Rust implementation |
| CLI | custom Rust |

no external package manager required to build nothing. nothing is self contained.
