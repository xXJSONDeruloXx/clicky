# Tetris Text Rendering (and general clickwheel text/texgen)

Fix the pointer-backed text rendering in Tetris (and, where general, all
clickwheel eapp games). Run **headed** so the user can see the window and
confirm/deny visual claims after each iteration; logs and run-data drive the
iteration even though the agent has no vision.

## Goals
- Make Tetris menu text content-correct (right glyphs, right font atlas, right
  screen-space advance) for BOTH pointer-backed text groups:
  - draws 9-14 (handle `0x100e38e0`, 10x12 font, scalar-formatter path)
  - draws 21-29 (handle `0x100e5260`, 16x16 font, UTF-16 cursor path)
- Do NOT hardcode glyph rectangles or strings. Model the guest texgen/formatter
  state transitions the game actually uses.
- Prefer general ABI/renderer/runtime fixes over Tetris-specific hacks; keep
  Tetris as the golden regression (0 fatals, skip/rasterized counts stable).
- Verify each change with a headed Tetris run + log inspection, then ask the
  user to confirm or disprove the visual result.
- Cross-game check: apply general fixes' benefits to PAC-MAN / Ms. PAC-MAN /
  Texas Hold'em / etc. where they share the same runtime.

## Root cause (RE, this session)

`0x1801616c(text_obj=r0, char=r1)` is the shared eapp text-runtime per-char
push helper. Disassembly of the scalar formatter at `0x18008480..0x1800857c`
shows it computes chars in registers (HH:MM AM/PM: `add r1,r0,#0x30` for
digits, `mov r1,#0x3a` for `:`, `moveq r1,#0x50`/`movne r1,#0x41` for A/P,
`mov r1,#0x4d` for M) and passes them as `r1` to `0x1801616c`. The char is
**never stored in a UTF-16 buffer** for the scalar path. 14 callers of
`0x1801616c` exist: the scalar formatter (`0x1800846c..0x18008574`) and UTF-16
string loops (`0x18009398`, `0x180094e4`, `0x18009574` which use `ldrhne r9,[r5]`
to read a halfword cursor into r1). So the helper serves BOTH paths.

The prior `live_find_texgen_text_cursor` heuristic scanned work-RAM for a
plausible UTF-16 buffer. For the scalar path it locked onto a stale
`text_ptr=0x101aa51a` always reading `'(' (0x28)` → same wrong glyph `(` for
all 6 draws, sampling the wrong `menuTetrisLogo_4444.pix` (132x91) atlas.

Watch-tool evidence (write-watchpoint RE tool, commits d0ce3fe + this one):
- `state+0x48..0x68` (per-vertex UV slots): only ever init to `(0.5,-0.5)`
  texel-center defaults; never updated per-glyph for the scalar path.
- UV array `0x101aa0a8`: written ONCE with full-atlas span
  `(0.5,90.5)→(132.5,-0.5)` by PC `0x1801e8a4`; never updated per-glyph.
- Position array `0x101aa218`: written ONCE with unit-cell GL_FIXED coords;
  per-glyph screen advance is via `translation` only (HLE carry works).
- text_obj `0x101aa140` vtable slot (`+0x00`) written 4× per frame:
  `0x18023efc` (init) → `0x18023e44` (char-writer dispatch) → `0x18023ac8`
  → `0x18023590` (restore). `0x1801dfd0` is trivial `str r1,[r0]; pop`.
- Sibling text objects form a pool (`0x101aa5e0`, `0x101aa3f0`, …); each is a
  per-glyph scratch. `0x101aa334`/`0x101aa2c4` count `1..0x24(36)` (monotonic
  per-frame glyph accumulator across both text runs).

## Fix implemented (this iteration)

General ABI hook, not a Tetris-specific hack:

1. `LiveGlState.text_char_seqs: HashMap<u32, Vec<u32>>` — per-frame, per
   text_obj, the ordered sequence of chars pushed via the helper. Reset each
   frame in `reset_for_frame`.
2. `LiveGlState.text_char_consumed: HashMap<u32, usize>` — per-run consumption
   index, advanced one char per draw.
3. PC hook in `Eapp::step()`: when `pc == TEXT_PUSH_CHAR_PC (0x1801616c)`,
   read `r0`/`r1` and call `record_text_char_push(text_obj, char)`. The
   constant is a per-binary address but the convention is shared-runtime;
   non-Tetris titles never hit this PC and are unaffected.
4. `live_decode_generated_text_uvs`: prefer `take_text_char_for_draw(text_obj)`
   (returns the next recorded char, advancing the index). Falls back to the
   cursor scan only when no char was recorded. One char consumed per draw =
   one glyph, matching the guest's push-then-draw cadence.

This models the guest formatter state the docs (`POINTER_BACKED_DRAWS_ANALYSIS.md`
§7.2, §7.4, §8) called for, without hardcoding strings.

## Results (this iteration)

Headed Tetris run, `CLICKY_GL_TEXGEN_VERBOSE=1`, 12s, exit 0.
Log: `/tmp/tetris_run_20260621_002942.log`.

- **0 fatals, 0 skips** (was 9 skipped; draw29 terminator skip resolved too).
- **draws 9-14 (scalar path)**: all 6 now `texgen=true`, each sampling a
  DISTINCT sub-region of the correct `f10x12text1_a8.pix` (980x24) font atlas
  (was the wrong `menuTetrisLogo_4444.pix` 132x91). 6 distinct chars consumed
  per frame: `'0'(0x30), ':'(0x3a), '.'(0x2e), '''(0x27), 'A'(0x41), 'M'(0x4d)`
  — clock-format chars.
- **draws 21-29 (UTF-16 path)**: still all `texgen=true`, all sampling
  distinct sub-regions of `f16x16menu1_a8.pix`. UVs now slightly different
  from before because the recorded char gives the true per-glyph index
  instead of the cursor-scan heuristic (more correct).
- First-frame fingerprint (`sig=[0x13,0xe,0x3,0x3,0x1b,...]`,
  `internal=0x9b1d4c80541b7f5e`) byte-identical to pre-fix → splash golden
  regression intact.
- 29 lib + eapp_gl_decode tests pass.

## Checklist
- [x] Extend write-watchpoint RE tool to w8/w16 + graceful drain (d0ce3fe).
- [x] Headed+verbose run to confirm both text paths' current behavior.
- [x] Watch state object `0x101aa170` → per-glyph UVs NOT written to +0x48.
- [x] Watch UV array `0x101aa0a8` → written once, full-atlas.
- [x] Watch position array `0x101aa218` → written once, unit-cell.
- [x] Disassemble `0x1801616c` + scalar formatter `0x18008480..0x1800857c`.
- [x] Root-cause: scalar formatter passes chars in r1 at `0x1801616c`; never
      stored in a UTF-16 buffer.
- [x] Implement general fix: PC hook records `(text_obj, char)`; decoder
      consumes recorded char per draw.
- [x] Headed Tetris regression: 0 fatals, 0 skips, draws 9-14 distinct glyphs
      on correct font atlas.
- [ ] **Ask user to confirm visual result of the menu text.**
- [ ] Cross-game check: does the recorded-char path help PAC-MAN / Ms.
      PAC-MAN / Texas Hold'em zero-UV skips? (They may not share the
      `0x1801616c` callsite; verify before claiming.)
- [ ] Cleanup: consider deprecating `live_find_texgen_text_cursor` cursor scan
      if the recorded path fully supersedes it (keep as fallback for now).

## Verification
- Headed verbose log: `/tmp/tetris_run_20260621_002942.log`
- Watch logs (RE evidence):
  - `/tmp/tet_watch_scalar.log` (355 hits, text_obj `0x101aa140`)
  - `/tmp/tet_watch_uv.log` (8 hits, UV array, all PC 0x1801e8a4, full-atlas)
  - `/tmp/tet_watch_pos.log` (16 hits, position array)
  - `/tmp/tet_watch_state.log` (53 hits, no per-glyph UV writes)
  - `/tmp/tet_watch_buf.log` (181 hits, sibling text_obj pool)
- Disassembly via `arm-none-eabi-objdump` on
  `~/Downloads/16-ipod-games/Games_RO/66666/Executables/Tetris_1_1_2563292.bin`
  (image VMA `0x18000000`; code at file offset = vma - 0x18000000).
- Cycle calibration: 60M cycles → frame 960 (~62.5k cyc/frame); menu at ~11M.
- Golden regression baseline (pre-fix): ~4808 rasterized, ~9 skipped, 0 fatals.

## Notes
- The `eapp-matrix-hardening` ralph loop (completed) established the broader
  UV/upload-matching context; this loop narrows in on the text-accuracy gap.
- Always run headed for visual confirmation unless doing a quick RE watch.
  Headed command:
  `CLICKY_GL_TEXGEN_VERBOSE=1 ./scripts/tetris.sh --no-build --timeout 12`
- Keep changes general: the PC-hook + recorded-char-seq design is a clean
  runtime ABI model, not a per-title patch. The `0x1801616c` PC is Tetris's
  binary address for a shared-runtime helper; sibling engines that link the
  same runtime may hit the same convention (to verify per-game).
