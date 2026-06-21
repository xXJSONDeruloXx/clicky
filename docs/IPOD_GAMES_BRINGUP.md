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
- the same runtime changes also help broader titles: headless PAC-MAN now
  resolves virtual-root audio paths like `/audio/extra life.wav` instead of
  immediately failing path lookup, and a `20,000,000`-cycle smoke test also
  completes without fatal memory exceptions
- current blocker has moved again: the runner is no longer dying on the first
  constructor/import path or the first late menu/resource dereference, but it is
  still missing real file/resource decoding semantics and real audio/runtime ABI
  behavior, so this is a stability checkpoint rather than a genuinely playable
  title yet

#### Cross-game headed smoke tests (2026-06-19)

Short headed smoke runs against sibling `Games_RO/*` bundles show that the
current Tetris-focused OpenGLES HLE does **not** regress every other title, but
it also exposes two clear shared engine gaps.

Tested with:

- `CLICKY_EXPERIMENTAL_GL_HLE=1`
- `CLICKY_GL_GATE_B=1`
- `CLICKY_GL_LIVE_CONTINUOUS=1`
- `CLICKY_GL_PRESENT_VFLIP=1`

Observed results:

- `55555` **Bejeweled**
  - launches and presents frames
  - repeatedly hits `OpenGLES:37 mode=7 count=28`
  - then fatals on an unmapped write:
    - `pc = 0x18001730`
    - write to `0x1080000c`
- `44444` **Zuma**
  - same failure signature as Bejeweled:
    - `pc = 0x18001730`
    - write to `0x1080000c`
- `77777` **Mahjong**
  - survives a 12s headed run
  - repeatedly hits `live_draw skipped: unsupported mode=7 first=0 count=16`
- `99999` **Cubis 2**
  - survives a 12s headed run
  - repeatedly hits `live_draw skipped: unsupported mode=7 first=0 count=16/20/40`
- `88888` **Mini Golf**
  - survives a 12s headed run
  - renders some solid quads
  - then hits `live_draw skipped: unsupported mode=7 first=0 count=8`
- `AAAAA` **PAC-MAN**
  - survives a 12s headed run
  - separate issue: `draw11..14 skipped: position array unusable handle=0x19`

Most useful cross-title conclusion so far:

- `OpenGLES:37 mode=7` is **not** a Tetris-specific oddity.
- In sibling titles it appears as `count = 8, 16, 20, 28, 40`, which strongly
  suggests the same primitive token is being used for **batched quads** rather
  than only the single-quad Tetris case.
- The shared unmapped write at `0x1080000c` is likely a broader runtime/device
  mapping issue, not title-specific content corruption.

That makes `mode=7` support and the `0x1080000c` path the highest-leverage
non-Tetris bring-up targets.

Follow-up after implementing grouped-quad handling for `OpenGLES:37 mode=7`:

- `unsupported mode=7` warnings disappear in headed reruns of:
  - `55555` Bejeweled
  - `77777` Mahjong
  - `99999` Cubis 2
  - `88888` Mini Golf
- **Bejeweled** now gets farther into real rasterization before still hitting the
  same unmapped write at `0x1080000c`.
- **Mini Golf** continues to render at least some quads/solid fills without the
  old mode-7 rejection.
- **Mahjong** and **Cubis 2** then exposed a narrower issue:
  - `drawN skipped: position array unusable`
  - detailed logs showed the array definitions were valid, but our tracked
    `enabled_arrays` set was empty or only `[1]`

Follow-up after fixing the array-enable assumption:

- ordinal 137 now auto-enables a valid array slot when it defines one
- this is evidence-backed by headed traces from titles that issue `DrawArrays`
  immediately after ordinal 137 with no separate explicit enable for slot 0

Before → after headed results:

| Game | Before | After / current gap |
|---|---|---|
| `77777` Mahjong | repeated `position array unusable` | position arrays fixed; now blocked on degenerate UVs (`no live upload matched UV span None`) |
| `99999` Cubis 2 | repeated `position array unusable` | thousands of rasterized draws; remaining gap is unsupported upload format `src_fmt=0x190a, pix_type=0x1401` |
| `14004` Ms. PAC-MAN | repeated `position array unusable` | thousands of rasterized draws; some draws still have zero UVs and skip |
| `88888` Mini Golf | mode-7 rejection / later skips | stable and rendering |
| `1500C` Sims Bowling | not yet characterized | stable 12s run, no position-array failure, but no completed GL frame diagnostics yet |
| `1500E` Sims Pool | not yet characterized | stable 12s run, no position-array failure, but no completed GL frame diagnostics yet |
| `50513` Sudoku | not yet characterized | stable 12s run, no position-array failure, but no completed GL frame diagnostics yet |
| `33333` Texas Hold'em | not yet characterized | position arrays are fine; next blocker is `OpenGLES:37 mode=5 count=11` |

Tetris regression check:

- `66666` **Tetris** reaches the stable headed startup/menu state with live GL
  enabled.
- Startup visuals now include the loading bar plus the splash → white flash/fade
  transition.
- Pointer-backed text groups now rasterize, use the right font-atlas families,
  and advance in screen space:
  - the UTF-16 pointer-backed loop (`draw21–29`) uses recovered texgen/text data
  - the scalar-formatted pointer-backed group (`draw9–14`) uses the same bounded
    generated-UV recovery after validating the active text object
  - a pointer-text transform carry preserves per-glyph translation deltas inside
    a glyph run, fixing the prior collapse of later glyphs to the top edge
  - generated text atlas selection now prefers matching cell-grid A8 font
    atlases, avoiding unrelated small A8 strips with the same UV extents
- Remaining Tetris text work is accuracy-focused: the text is visible but not
  content-correct yet (current evidence still shows placeholder/wrong glyphs
  like `9 ABCDE` where menu strings should be). Replace the fallback cursor /
  text-object recovery with the exact guest formatter/texgen state path.
- This cross-game array fix did not regress the Tetris path.

So the shared `position array unusable` blocker was a real engine bug and is now
fixed. The next cross-title blockers are narrower and more valuable to tackle
individually:

1. zero/degenerate UV streams in some titles (Mahjong, parts of Ms. PAC-MAN)
2. unsupported upload format `src_fmt=0x190a, pix_type=0x1401` (Cubis 2)
3. unsupported draw token `mode=5` (Texas Hold'em)
4. shared unmapped write at `0x1080000c` (Bejeweled / Zuma)
5. Tetris exact formatter/texgen state modeling for pointer-backed text

#### Full headed matrix snapshot (2026-06-20)

A full headed pass over all 16 bundles was run after the input/event-list and
Tetris text-rendering fixes. This matrix is intended as a durable baseline for
future sessions.

Run setup:

- artifact root: `/tmp/clicky_headed_matrix_unique_20260620_201555`
- contact sheet: `/tmp/clicky_headed_matrix_unique_20260620_201555/contact_sheet.png`
- command shape: `./scripts/tetris.sh --no-build --timeout 7 --bundle /Users/kurt/Downloads/16-ipod-games/Games_RO/<id>`
- live GL env: `CLICKY_EXPERIMENTAL_GL_HLE=1`, `CLICKY_GL_GATE_B=1`,
  `CLICKY_GL_LIVE_CONTINUOUS=1`, `CLICKY_GL_PRESENT_VFLIP=1`
- capture cadence: `CLICKY_STARTUP_CAPTURE_PERIOD=20`, max 500 guest frames /
  80 dumps per title
- log level: `EAPP_GL=info,EAPP=info,EAPP_PROGRESS=info,EAPP_IMPORT=warn,EAPP_INPUT=info`
- performance estimates below are `startup_progress frame / host_us`; they
  include very heavy logging and are useful only as relative health indicators.

| ID | Game | Artifact log | Latest visual | Result | Approx. perf | Current blocker / useful finding |
|---|---|---|---|---|---:|---|
| `11002` | iQuiz | `/tmp/clicky_headed_matrix_unique_20260620_201555/11002/logs/tetris_run_20260620_201555.log` | no capture | quick fatal | n/a | unmapped write `pc=0x18001b08 off=0x0000000c` after early GL setup/upload |
| `12345` | Vortex | `/tmp/clicky_headed_matrix_unique_20260620_201555/12345/logs/tetris_run_20260620_201555.log` | no capture | quick fatal | n/a | unmapped write `pc=0x18014d58 off=0x00000004` after several inline GL uploads |
| `14004` | Ms. PAC-MAN | `/tmp/clicky_headed_matrix_unique_20260620_201555/14004/logs/tetris_run_20260620_201556.log` | `14004_latest.png` in artifact root | good loading screen | ~58 fps | mostly renders; remaining skips are `no live upload matched UV span None` for handle `0x2`/related zero-UV cases |
| `1500C` | The Sims Bowling | `/tmp/clicky_headed_matrix_unique_20260620_201555/1500C/logs/tetris_run_20260620_201603.log` | no capture | runs/idles | ~138 fps | no completed GL frames yet; likely lifecycle/timer/settings/resource-callback gap |
| `1500E` | The Sims Pool | `/tmp/clicky_headed_matrix_unique_20260620_201555/1500E/logs/tetris_run_20260620_201610.log` | no capture | runs/idles | ~140 fps | no completed GL frames yet; likely same family as Bowling |
| `1B200` | LOST | `/tmp/clicky_headed_matrix_unique_20260620_201555/1B200/logs/tetris_run_20260620_201617.log` | no capture | runs but no visible frame | ~360 fps | unsupported upload `src_fmt=0x190a pix_type=0x1401` |
| `33333` | Texas Hold'em | `/tmp/clicky_headed_matrix_unique_20260620_201555/33333/logs/tetris_run_20260620_201624.log` | `33333_latest.png` | partial loading text | ~17 fps | repeated `OpenGLES:37 mode=5 count=11` |
| `44444` | Zuma | `/tmp/clicky_headed_matrix_unique_20260620_201555/44444/logs/tetris_run_20260620_201631.log` | `44444_latest.png` | tiny partial text/sprite bits, then fatal | ~6 fps before fatal | shared PopCap unmapped write `pc=0x18001720 off=0x1080000c` |
| `50513` | Sudoku | `/tmp/clicky_headed_matrix_unique_20260620_201555/50513/logs/tetris_run_20260620_201634.log` | no capture | runs/idles | ~208 fps | no completed GL frames yet; lifecycle/runtime gap |
| `50514` | Royal Solitaire | `/tmp/clicky_headed_matrix_unique_20260620_201555/50514/logs/tetris_run_20260620_201641.log` | no capture | early/stalled | ~2 fps early | likely waiting in early async/runtime state |
| `55555` | Bejeweled | `/tmp/clicky_headed_matrix_unique_20260620_201555/55555/logs/tetris_run_20260620_201648.log` | `55555_latest.png` | partial white loading text, then fatal | ~4 fps before fatal | shared PopCap unmapped write `pc=0x18001730 off=0x1080000c` |
| `66666` | Tetris | `/tmp/clicky_headed_matrix_unique_20260620_201555/66666/logs/tetris_run_20260620_201652.log` | `66666_latest.png` | strong startup/menu render | ~29 fps | text is visible but content is still wrong; generated-text UV misses remain for pointer handles `0x100e38e0` / `0x100e5260` |
| `77777` | Mahjong | `/tmp/clicky_headed_matrix_unique_20260620_201555/77777/logs/tetris_run_20260620_201659.log` | `77777_latest.png` | black despite presented frames | ~70 fps | GL frames present, but no rasterized draws; UV/upload matching blocker around handle `0x19` |
| `88888` | Mini Golf | `/tmp/clicky_headed_matrix_unique_20260620_201555/88888/logs/tetris_run_20260620_201707.log` | `88888_latest.png` | loading-bar outline | ~63 fps | mostly stable; small UV/upload miss for handle `0x27` |
| `99999` | Cubis 2 | `/tmp/clicky_headed_matrix_unique_20260620_201555/99999/logs/tetris_run_20260620_201714.log` | `99999_latest.png` | recognizable title screen, upside down | ~18 fps | many draws rasterized; unsupported upload `src_fmt=0x190a pix_type=0x1401`; orientation/presentation issue also visible |
| `AAAAA` | PAC-MAN | `/tmp/clicky_headed_matrix_unique_20260620_201555/AAAAA/logs/tetris_run_20260620_201721.log` | `AAAAA_latest.png` | visible maze side art + text | ~44 fps | mostly renders; remaining no-upload/UV misses around handle `0x19` |

Cross-game north-star priorities from this matrix:

1. ~~implement `GL_LUMINANCE_ALPHA` / `src_fmt=0x190a pix_type=0x1401` texture uploads; this is a discrete GL ES 1.1 format gap and should help Cubis 2 and LOST immediately~~ **Done after this matrix:** see validation note below.
2. ~~model or safely map the shared PopCap write target at `0x1080000c`; this blocks both Bejeweled and Zuma after they already reach real uploads/draws~~ **Resolved as a RAM-aperture issue:** see 64 MiB validation note below.
3. implement/identify `OpenGLES:37 mode=5` for Texas Hold'em instead of treating it as an unknown draw token
4. improve UV/upload matching for zero/degenerate UV cases; this affects Mahjong, PAC-MAN, Ms. PAC-MAN, Mini Golf, and the remaining Tetris pointer-text misses
5. investigate titles that pump frames but produce no completed GL frames (Sims Bowling/Pool, Sudoku, Solitaire, iQuiz, Vortex) as runtime/lifecycle coverage rather than renderer-only work

Follow-up validation for `GL_LUMINANCE_ALPHA` (`src_fmt=0x190a pix_type=0x1401`):

- code path added as renderer format `TextureFormat::LuminanceAlpha88`, decoded as GL ES `LA` byte pairs (`rgb = luminance`, `a = alpha`)
- targeted headed artifact root: `/tmp/clicky_la_validate_20260620_202557`
- `cargo test -p clicky-core --test eapp_gl_decode` passed after adding format/payload-size coverage
- Cubis 2 (`99999`) validation:
  - log: `/tmp/clicky_la_validate_20260620_202557/99999/logs/tetris_run_20260620_202557.log`
  - latest screenshot: `/tmp/clicky_la_validate_20260620_202557/99999_latest.png`
  - prior matrix: 18 skipped draws and repeated unsupported `0x190a/0x1401`
  - after fix: unsupported-format count is 0; skip count drops to 2; many uploads now log as `Some(LuminanceAlpha88)`, e.g. `images/menubg.raw`, `jewel/sheet-*.raw`, `classic/sheet-*.raw`, `metallic/sheet-*.raw`
  - remaining visual issue: title screen is still upside down / presentation-orientation related, and two UV/upload misses remain
- LOST (`1B200`) validation:
  - log: `/tmp/clicky_la_validate_20260620_202557/1B200/logs/tetris_run_20260620_202604.log`
  - unsupported-format count is 0; early inline uploads now log as `Some(LuminanceAlpha88)`
  - still no completed GL frames/captures in the 7s headed run, so the blocker has moved from texture decode to lifecycle/draw-path coverage

Follow-up validation for the PopCap `0x1080000c` / RAM-aperture blocker:

- root cause: the previous synthetic eapp work-RAM window was only 8 MiB
  (`0x1000_0000..0x1080_0000`). Bejeweled and Zuma faulted inside memcpy-like
  asset-copy routines exactly at/just past that boundary, not in device IO.
- first experiment: 32 MiB moved Zuma's fault from `0x1080000c` to
  `0x1200000c`, the new boundary, while Bejeweled survived the 7s window. This
  strongly indicated a too-small guest RAM aperture rather than a special
  register.
- fix: increase the synthetic eapp work RAM to 64 MiB, matching high-memory
  5G-class iPods targeted by many clickwheel games.
- targeted headed artifact root: `/tmp/clicky_ram64_validate_20260620_203011`
- Bejeweled (`55555`) validation:
  - log: `/tmp/clicky_ram64_validate_20260620_203011/55555/logs/tetris_run_20260620_203011.log`
  - latest screenshot: `/tmp/clicky_ram64_validate_20260620_203011/55555_latest.png`
  - prior matrix: fatal at `pc=0x18001730 off=0x1080000c`
  - after fix: no `FatalMemException` in the 7s headed window; remaining skips are UV/upload matching for handles `0x16` and `0x10`
- Zuma (`44444`) validation:
  - log: `/tmp/clicky_ram64_validate_20260620_203011/44444/logs/tetris_run_20260620_203018.log`
  - latest screenshot: `/tmp/clicky_ram64_validate_20260620_203011/44444_latest.png`
  - prior matrix: fatal at `pc=0x18001720 off=0x1080000c`
  - 32 MiB experiment: fatal moved to `off=0x1200000c`
  - after 64 MiB fix: no `FatalMemException` in the 7s headed window; remaining visible output is still minimal due renderer/UV gaps
- Tetris (`66666`) regression:
  - log: `/tmp/clicky_ram64_validate_20260620_203011/66666/logs/tetris_run_20260620_203025.log`
  - latest screenshot: `/tmp/clicky_ram64_validate_20260620_203011/66666_latest.png`
  - startup/menu state remains stable with the same known pointer-text UV/content gaps

#### Honest status (stable but green)

Running the desktop (non-headless) runner today shows a flat green window. That
is expected and is *us*, not the game. Concretely:

What is **actually working**:

- `eapp` binary loading: header, import-module chain, entry/init/aux pointers
- real ARM execution on the existing core (not a stub loop)
- bootstrap lifecycle: entry → constructor → synthetic frame pump
- import interception: every guest import is trapped via patched literal
  tables and routed to an HLE handler
- `AsyncFileIO:3` path resolution across bundle-root / `Resources/` /
  virtual-root `/audio/...` / synthetic `.clicky-saves/` saves
- stability: `20,000,000` headless cycles without fatal exceptions for both
  Tetris and PAC-MAN

What is **NOT working** (and why it is a green screen):

- `OpenGLES` is a pure no-op. Every GL import just fills the whole framebuffer
  solid green (`HLE_OPENGL_FRAMEBUFFER = 0xff205020`) and returns 0. There is:
  - no texture upload (`.pix` / `.tga` never become GL textures)
  - no draw calls, no vertex data, no matrices
  - no clear / viewport / scissor
  - no guest-drawn framebuffer at all
- ~~File contents never reach guest memory.~~ **Now fixed as of the latest
  commit:** `AsyncFileIO:3` reads the host file and copies the bytes directly
  into the guest-provided destination buffer (`[req+0x14]`, length `[req+0x18]`)
  before firing the completion callback. Reverse-engineered request-object layout:
  - `[req+0x04]` call type (=6)
  - `[req+0x14]` destination buffer pointer (guest-allocated; reused as a
    staging buffer across loads)
  - `[req+0x18]` expected byte count (matches file size)
  - `[req+0x34]` completion callback pc
  - `[req+0x38]` completion callback context
  The guest's own `.pix` / `.tga` / `.wav` parsers now receive real bytes.
- The completion callback is still synthetic in the sense that we drive it
  directly, but it now runs against a request object whose destination buffer
  is genuinely populated. The Tetris placeholder-slot hack is still in place for
  a separate late menu/resource null-deref path.
- `Audio` is fully stubbed (returns 0, nothing plays).
- `InputEvents:0` is now wired through both observed ABI shapes:
  - it returns/writes the compact pointer-output bitfield for callers that read
    the two out-pointers
  - it also builds the guest button-event linked list consumed by Tetris'
    wrapper (`input_obj+0x30`) instead of only returning a value in r0
  - desktop mapping remains arrows = directional input, Enter = action/select,
    `M` = iPod Menu, but the exact id-to-button mapping is still provisional
- Empirical headless input smoke evidence:
  - raw `event=1` at the menu is accepted by the guest and enters the menu/exit
    path, including `prefs.sav` AsyncFileIO:12/14 calls, then currently crashes
    in standalone teardown/refcount cleanup because there is no RetailOS shell
    to return to and a later object destructor path is still unmapped
  - raw `event=2` is also accepted and changes the rendered state, but the
    resulting screen is mostly black due remaining graphics/state gaps
  - old return-only bitfield injection (`bits=...`) produced no visible state
    change, confirming the linked-list event path is the important one
- `Metadata` / `Settings` return dummy zeros (fine for startup, may matter later).
- Saves are empty shells: `.clicky-saves/*.sav` are created zero-byte; we never
  read or write real save data.

#### Critical path to "something visible"

In order:

1. ~~Make `AsyncFileIO:3` actually load file bytes into a guest buffer the
   resource layer points at~~ **Done.** The guest's own resource parsers now
   receive real file bytes.
2. ~~Implement the handful of `OpenGLES` ordinals needed to blit a texture~~
   **Option A diagnostic completed; Option B now scoped.** See below.

---

## Option A diagnostic findings

### A.1 Surface-blit shortcut: not viable

- Work-RAM scan after 5M cycles: **no region matching 320×240×2 (153600 B
  RGB565) or ×4 (307200 B RGBA8888)** exists. Largest region = 102 KB.
- The "surface handle" `0x0003f001` in `OpenGLES:158 r0` is a **constant
  token** built as `1 + 0x3F000` in guest code — a capability bitmask, not an
  address.
- The frame loop writes no pixels into guest RAM because GL is stubbed. Nothing
  to blit.

### A.2 OpenGLES is standard GL ES 1.1

The API uses Apple’s own ordinal numbering but the format constants are
standard:

```
GL_ALPHA      = 0x1906  (_a8   assets: font bitmaps, UI alpha masks)
GL_RGB        = 0x1907  (_565  assets: full-screen backgrounds)
GL_RGBA       = 0x1908  (_5551 / _4444 assets: sprites, logos)
GL_TEXTURE_2D = 0x0DE1
GL_FIXED      = 0x140C  (vertex element type: 16.16 fixed-point)
GL_QUADS      = 0x0007  (draw primitive confirmed from disassembly)
```

### A.3 Confirmed ordinal → GL function mapping

| Ordinal   | Function                 | Key args / evidence |
|-----------|--------------------------|---------------------|
| `GL:4`    | glTexParameteri / bind   | r0=GL_TEXTURE_2D; called once/texture before upload |
| `GL:12`   | glClear (init only)      | r0=0x4000=GL_COLOR_BUFFER_BIT |
| `GL:13`   | glClearColor (init only) | r0,r1,r2,r3 = 0,0,0,1.0 (black) |
| `GL:37`   | **glDrawArrays** ✓       | disasm confirmed: mode=GL_QUADS(7), first=0, count=4 |
| `GL:40`   | enableClientArray        | r0=array_index |
| `GL:45`   | createTexture / initObj  | r0=tex_name, r1=descriptor_ptr, r2=width, r3=height; once/texture |
| `GL:99`   | **glTexImage2D** ✓       | r0=GL_TEXTURE_2D, r1=level=0, r2=GL_RGB/RGBA/ALPHA, r3=width; once/texture |
| `GL:125`  | prepareDraw              | r0=0, r1=1, r2=0, r3=state_ptr |
| `GL:137`  | setVertexArrayFormat     | r0=array_idx, r1=components, r2=GL_FIXED, r3=stride, stack:ptr |
| `GL:157`  | **submitFrame**          | r0=0, r1=0, r2=5, r3=ptr; LAST call in aux |
| `GL:158`  | **presentFrame / swap**  | r0=0x3f001 (token), r2=work_ram_ptr; FIRST call in aux |
| `GL:159`  | bindTexture + vtxSetup   | r0=GL_tex_name (small int), r1=vtx_buf_ptr, r2=float |
| `GL:165`  | beginFrame / bindContext | r0=ctx, r1=ptr, r2=vtx_buf_ptr; once/frame after present |
| `GL:169`  | setPosition / translate  | r0=ctx, r1=x_float, r2=y_float, r3=0; screen-space coords |
| `GL:175`  | bindDrawState            | r0,r1,r2=static state ptrs |
| `GL:36`   | postDraw cleanup         | r0=1 or 2 |

### A.4 Texture dimensions from upload calls

GL:99 (glTexImage2D) called once per asset during loading phase (frame 2):

```
320 × 240  GL_RGB    screenBG_565.pix   (full-screen background)
 50 ×  50  GL_RGBA   eaLogo / small sprite
250 × 162  GL_RGBA   tetrisLogoT and similar
784 ×  20  GL_ALPHA  font-strip atlas ×3 variants (wide, thin, 1-bpp)
```

The `.pix` header is approximately 72 bytes before the raw pixel data
(153672 − 320×240×2 = 72 for the screenBG). The guest parses this header
itself after receiving bytes via `AsyncFileIO:3`.

### A.5 Per-frame GL call sequence (steady-state, ~4 quads/frame)

Double-buffered render loop confirmed from frame-40 GL trace:

```
aux(frame N):
  GL:158  — presentFrame(token=0x3f001, 1, vtx_buf_ptr)
             display frame N-1; FIRST call in aux
  GL:165  — beginFrame(ctx, ptr, vtx_buf_ptr)

  [repeated ×4 per frame:]
    GL:169  — setPosition(ctx, x, y, 0)    [x/y = screen-space float coords]
    GL:159  — bindTexture(tex_id, vtx_buf, size)
    GL:137  — setArrayFmt(0, 4, GL_FIXED, stride, ptr)  [position: XYZW]
    GL:40   — enableArray(0)
    GL:137  — setArrayFmt(1, 2, GL_FIXED, stride, ptr)  [texcoord: ST]
    GL:40   — enableArray(1)
    GL:175  — bindDrawState(state_ptr, vtx_arr_ptr, ctx_ptr)
    GL:125  — prepareDraw(0, 1, 0, state_ptr)
    GL:37   — glDrawArrays(GL_QUADS=7, first=0, count=4)   ← DRAW
    GL:36   — postDraw(1 or 2)

  GL:157  — submitFrame(0, 0, 5, ptr)
             commit frame N for presentation; LAST call in aux
```

### A.6 Key facts for Option B

- Vertex format: **GL_FIXED (16.16 fixed-point)**, not floats.
  4-component position (XYZW), 2-component texcoord (ST).
- Texture names are small integers (3, 14, 19, 27, ...) allocated sequentially.
- All texture pixel data is already in guest work RAM (delivered by
  `AsyncFileIO:3` to `[req+0x14]` buffers). The guest parsed .pix headers and
  the raw pixel data is at a known offset within those buffers.
- `GL:175`, `GL:125`, `GL:36` can be no-op stubs initially.
- `GL:12`, `GL:13` (clear/clearcolor) are init-only (2 calls total);
  can fill host framebuffer black on beginFrame instead.

---

## Option B: minimum viable GL ES subset (now the active milestone)

Not "implement all of GL ES 1.1" — implement exactly these ordinals:

**Priority 1** — required for any pixels:

1. `GL:45 + GL:4 + GL:99` — texture upload. Build a host-side texture cache
   keyed by GL name. Formats: GL_RGB/GL_RGBA/GL_ALPHA; dimensions from GL:99.
2. `GL:169` — setPosition(x, y). Track current sprite position.
3. `GL:159` — bindTexture + setVertexBuffer. Route to texture cache entry.
4. `GL:37`  — drawArrays. Rasterize: sample texture using GL_FIXED texcoords,
   write pixels to a 320×240 host framebuffer.
5. `GL:158` — presentFrame. Blit host framebuffer to minifb window.

**Priority 2** — correct quad geometry:

6. `GL:137 + GL:40` — vertex/texcoord array format + pointer. Decode GL_FIXED
   arrays: 4-component XYZW position, 2-component ST texcoord.
7. `GL:157` — submitFrame. Mark frame ready for next present.
8. `GL:165` — beginFrame. Clear host framebuffer.

**Priority 3** — correctness polish (not needed for first pixels):

- `GL:175`, `GL:125`, `GL:36` — stub as no-op
- Correct alpha-blending for `_a8` / `_4444` / `_5551` textures
- Save-data read/write (empty = new game, playable without it)

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
  *(partially answered: entry/init/aux confirmed; words 6-7 at offsets 0x1c/0x20
  are still zero across all titles, unknown purpose)*
- how are imports resolved at runtime? *(answered: patched literal tables; stubs
  at `stubs_addr` do `ldr pc, [lit]`; we patch the lit to our trampolines)*
- ~~is `OpenGLES` a literal GL-style API, a command buffer, or just Apple naming?~~
  **Answered:** it is standard GL ES 1.1 with Apple's proprietary ordinal
  numbering. Standard format constants (GL_RGBA=0x1908, GL_FIXED=0x140C etc.)
  are confirmed.
- how much of the `*.sinf` / DRM story is already bypassed by the supplied
  firmware and bundles? *(still unknown; games run without DRM checks so far)*
- which services are pure userspace runtime ABI vs which ones implicitly depend
  on RetailOS kernel behavior? *(still unknown; GL/IO seem pure userspace)*
- what is the exact `.pix` file header format? *(72-byte header before raw
  pixels; structure not yet decoded — not needed since guest parses it itself)*
- what does `GL:157` r2=5 mean? *(unknown; could be quad count or a flags field)*
- what exactly does `GL:165` bind? *(context + vertex buffer ptr, but the
  double-ptr indirection via static globals is not fully traced)*

## Working rule for this branch

When in doubt, prefer:

- **game-facing runtime shims**
- **host-native replacements**
- **small focused instrumentation**

…over broad hardware modeling that does not move a real game closer to booting.
