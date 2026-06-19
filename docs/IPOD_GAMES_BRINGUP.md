# iPod Games bring-up plan

This branch is focused on **running clickwheel games on a Mac host** with the
smallest viable amount of emulated iPod machinery.

## Branch choice

This work starts from `ipodlinux-bringup`, not `master`.

Why:

- it is already ahead of `master` with useful emulator fixes
  - ARM block-transfer / CPSR fixes
  - ATA probe / EIDE IRQ fixes
  - watchdog diagnostics for bring-up
- none of those changes are specific liabilities for the games effort
- the extra watchdog tracing is useful if we still need to spike against Apple
  RetailOS during ABI discovery

If this grows into something that should merge independently, we can rebase or
split later.

## What we know from the game archive

The sample archive contains 16 game bundles under `Games_RO/`, including:

- Tetris
- PAC-MAN / Ms. PAC-MAN
- Bejeweled
- Zuma
- Vortex
- Texas Hold'em
- Cubis 2
- Mini Golf
- LOST
- The Sims Bowling / Pool
- Mahjong / Sudoku / Solitaire / iQuiz

Common traits across the bundles:

- each bundle has a `Manifest.plist`
- each bundle has one `Executables/*.bin` payload and one matching `*.sinf`
- all bundles target `PlatformID = 1`, `PlatformVersion = 1`
- the executables share a common `eapp` header
  - apparent load address: `0x10001000`
  - apparent format/runtime version: `5`
  - apparent header size: `0x28`
- executables reference shared runtime-style imports such as:
  - `OpenGLES`
  - `AsyncFileIO`
  - `Audio`
  - `InputEvents`
  - `Settings`
  - `Metadata`
  - `Filesytem` / `miscTBD` in some titles

This strongly suggests the games are **not standalone ROMs**. They are app
binaries that expect Apple's iPod game runtime / ABI.

## What we know from the firmware samples

The provided 5G / 5.5G firmware images are Apple format-v3 firmware archives.
They contain the usual `!ATA` images, including `soso` (OS image).

The firmware strings include:

- `games_RO`
- `gamedata_RW`
- `gamestats_WO`
- `GamesPlatformID`
- `GamesPlatformVersion`
- `AppleDRM`
- user-facing game failure strings like “Connect your iPod to iTunes and reinstall the game.”

That confirms RetailOS knows about the clickwheel game install/runtime model.

## What `clicky` has today

Current repo state is still centered on the **iPod 4G grayscale** target:

- only `sys/ipod4g` exists
- the only LCD device in-tree is the 4G grayscale controller
- RetailOS bring-up is still incomplete even on 4G
- there is no in-tree 5G color display, no 5G system definition, and no Apple
  game-runtime support

## Recommended path forward

### Primary path: direct `eapp` runtime spike

For the stated goal (“just run the games somehow”), the best path is **not** to
finish all of RetailOS first.

Instead, prefer:

1. inventory the game ABI and package format
2. build a small dedicated runner around the existing ARM core
3. implement only the runtime services the games actually import
4. fake just enough filesystem / settings / save-data behavior to satisfy the
   games
5. use RetailOS bring-up only as a reference path when ABI details are missing

Why this is the best tradeoff:

- it minimizes irrelevant hardware work
- it avoids needing a full 5G boot chain before seeing game code execute
- it lets us target one game at a time
- it keeps host-side UX flexible for a Mac-native frontend

### Secondary path: 5G RetailOS reference bring-up

We should still keep a **clean 5G firmware** around as a reference because it is
likely the easiest way to validate:

- expected filesystem layout
- per-title launch flow
- save paths
- platform/version checks
- any runtime metadata we do not yet understand

But RetailOS should be treated as a **debug oracle**, not the first milestone.

## First game targets

Not all games are equally good bring-up targets.

Recommended order:

1. **Tetris (`66666`)**
   - small executable
   - simple asset set (`.pix`, `.wav`, strings)
   - looks like a good low-complexity runtime canary
2. **PAC-MAN (`AAAAA`)** or **Ms. PAC-MAN (`14004`)**
   - conventional save paths visible in strings
   - clear asset layout
3. **Bejeweled / Zuma**
   - more resource sidecars (`.ro`)
4. **Sims / Sudoku / Solitaire / Mahjong**
   - heavier sidecar resource libraries (`.rlb`)
5. **Vortex / Texas Hold'em / iQuiz / Cubis 2**
   - more custom asset formats / localization complexity

## Immediate milestones

### Milestone 0: static inspection tooling

Done in this branch:

- `scripts/games/ipod_games_probe.py`
  - inventories a `Games_RO/` tree
  - parses `Manifest.plist`
  - extracts `eapp` header metadata
  - lists early imported runtime modules
  - surfaces likely save/resource paths
  - parses Apple format-v3 firmware image directories

Example usage:

```bash
python3 scripts/games/ipod_games_probe.py firmware /path/to/5g\ Firmware.bin
python3 scripts/games/ipod_games_probe.py games /path/to/Games_RO
```

### Milestone 1: loader spike

Implemented in this branch:

- `clicky-core/src/sys/eapp/mod.rs`
  - minimal single-core `eapp` runner
  - parses the `eapp` header and import-module chain
  - maps the executable at file VMA `0x18000000`
  - provides scratch/work RAM at `0x10000000`
  - patches import literal tables to synthetic HLE trampolines
  - logs runtime import calls with module + ordinal + register args
- `clicky-desktop/src/bin/eapp.rs`
  - experimental desktop runner for a single `Games_RO/<id>` bundle
  - minifb window with basic key bindings
  - headless mode for quick import/bring-up tracing

Example usage:

```bash
cargo run -p clicky-desktop --bin eapp -- /path/to/Games_RO/66666
cargo run -p clicky-desktop --bin eapp -- /path/to/Games_RO/66666 --headless --cycles 200000
```

Current observed behavior for **Tetris**:

- the binary loads and begins executing native ARM code from the parsed entrypoint
- the runner now treats the initial `eapp` entry as a bootstrap/init phase and
  then repeatedly pumps the app's `aux` callback as a synthetic frame loop
- synthetic completion callbacks for `AsyncFileIO:3` let the game advance past
  its early asynchronous asset-open flow
- host-side path resolution now covers:
  - bundle-root assets
  - `Resources/`
  - virtual-root bundle paths like `/audio/foo.wav`
  - synthetic writable save files under `.clicky-saves/`
- the runner now also includes two deliberately-hacky but useful bring-up aids:
  - a tiny HLE for the game's `svc 0x123456` syscall wrapper so debug/error
    printing no longer immediately falls into the unmapped exception vector
  - placeholder resource-slot seeding for a later Tetris-only guest path that
    was previously dereferencing null entries during menu/resource setup
- observed imports now include:
  - `miscTBD:0`
  - `miscTBD:1`
  - `miscTBD:6`
  - `miscTBD:9`
  - `miscTBD:12`
  - `miscTBD:13`
  - `miscTBD:14`
  - `InputEvents:0`
  - `Settings:0`
  - `Audio:0`
  - `Audio:40`
  - `Audio:43`
  - `Audio:48`
  - `Audio:51`
  - `Audio:52`
  - `Audio:53`
  - `Audio:55`
  - `Audio:56`
  - `Metadata:62`
  - `Metadata:134`
  - `OpenGLES:12`
  - `OpenGLES:13`
  - `OpenGLES:35`
  - `OpenGLES:36`
  - `OpenGLES:37`
  - `OpenGLES:40`
  - `OpenGLES:125`
  - `OpenGLES:137`
  - `OpenGLES:157`
  - `OpenGLES:158`
  - `OpenGLES:159`
  - `OpenGLES:165`
  - `OpenGLES:167`
  - `OpenGLES:169`
  - `OpenGLES:175`
  - `AsyncFileIO:3`
- Tetris now successfully walks a long real asset-open sequence including:
  - `Strings.dta`
  - many `.pix` UI/image assets
  - `.wav` audio assets
  - save paths like `prefs.sav` and `game.sav`
- with the current bring-up hacks in place, headless Tetris runs now survive at
  least `20,000,000` cycles without fatal memory exceptions
- the same runtime changes also help broader titles: a quick headless PAC-MAN
  smoke test now resolves virtual-root audio paths like `/audio/extra life.wav`
  instead of immediately failing path lookup
- current blocker has moved again: the runner is no longer dying on the first
  constructor/import path or the first late menu/resource dereference, but it is
  still missing real file/resource decoding semantics and real audio/runtime ABI
  behavior, so this is a stability checkpoint rather than a genuinely playable
  title yet

This is the first meaningful checkpoint for the direct-runtime path.

### Milestone 2: minimal runtime services

Prioritize stubs/HLE for the imports that repeatedly show up:

- `AsyncFileIO`
- `InputEvents`
- `Settings`
- `Metadata`
- `Audio` (stub acceptable at first)
- `OpenGLES` (likely software-backed shim / command adapter)
- `Filesytem` / `miscTBD` as discovered

### Milestone 3: first playable title

Target **Tetris** first.

Success bar:

- executable enters main loop
- assets load from the bundle
- input can move through menus / game state
- screen updates render in a host window
- save data can be written somewhere under a host-side per-game directory

## Open questions

- what exactly is the `eapp` header contract beyond the obvious first words?
- how are imports resolved at runtime?
- is `OpenGLES` a literal GL-style API, a command buffer, or just Apple naming?
- how much of the `*.sinf` / DRM story is already bypassed by the supplied
  firmware and bundles?
- which services are pure userspace runtime ABI vs which ones implicitly depend
  on RetailOS kernel behavior?

## Working rule for this branch

When in doubt, prefer:

- **game-facing runtime shims**
- **host-native replacements**
- **small focused instrumentation**

…over broad hardware modeling that does not move a real game closer to booting.
