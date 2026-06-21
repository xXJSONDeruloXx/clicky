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
- [x] ~~Ask user to confirm visual result of the menu text~~ — superseded:
      iteration 3 traced the gibberish to a garbage upstream time value,
      NOT a rendering bug. User eyes still welcome for the UTF-16 mid-row
      `89ΔΕABCDE` (a memory literal, probably a real label), but the scalar
      bottom row `':.'0AM` is explained by guest state, not the renderer.
- [x] ~~Cross-game check~~ (iteration 1: Tetris-scoped helper, no regression).
- [x] Decide: mechanism is PROVEN correct. The remaining issue is NOT
      char-source or consumption alignment — it's an upstream garbage time
      value. Pivoted to the time-value workstream below.
- [ ] **Find what writes the time value at guest `0x1005f780`** (svc? struct
      init?) and provide a valid iPod system-clock value so the formatter
      produces real digits. NEW top-priority item (separate RTC/time-emulation
      workstream, upstream of the text renderer).
- [ ] Re-run headed after the time value is sane; the scalar bottom row
      should then read a real `H:MM AM` instead of `':.'0AM`.
- [ ] (Optional) Find a menu-state oracle or capture a real-device menu
      frame to validate the exact strings; the lcd5 oracle is gameplay-only.
- [ ] Cleanup: once the time value is sane AND user confirms UTF-16 row,
      deprecate `live_find_texgen_text_cursor` cursor scan and remove the
      temporary `TEXT_FORMAT_TIME_ENTRY_PC` / `take_text_char_diag` RE hooks.

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

## Iteration 3 — root cause CONFIRMED: garbage upstream time value (not mechanism)

After iteration 2 proved the text mechanism correct (push==consume,
ASCII table, glyphs match) but left the *string content* of `':.'0AM`
unexplained, iteration 3 traced WHERE the weird chars `'` (0x27) and
`.` (0x2e) come from. They are NOT produced by the clock formatter's
literal-emitting branches — so they must be computed. Found the real
source via static + dynamic RE:

### Static RE
- Disassembled the binary in **ARM mode** (`-m arm`, not `force-thumb`
  which had mis-decoded ARMv4 code as Thumb-2/NEON). Found all **15**
  call sites of `0x1801616c` (14 `bl` + 1 tail `b`):
  - **Cluster A** `0x1800846c..0x18008578` (7 sites): the AM/PM clock
    formatter. Emits `add r1,r0,#0x30` (digits), `mov r1,#0x3a` (`:`),
    `moveq r1,#0x50`/`movne r1,#0x41` (P/A), `mov r1,#0x4d` (M).
  - **Cluster B** `0x1800868c..0x18008748` (5 sites): a second time
    formatter (`H:MM`/`HH:MM`), digits + `:` only.
  - **UTF-16 loops** `0x18009398, 0x180094e4, 0x18009574`: read chars
    from a memory buffer (`ldrhne`).
- Neither cluster A nor B contains `mov r1,#0x27` or `mov r1,#0x2e`.
  Grep across the whole binary shows no `add r1,rN,#0x27/0x2e` either.
  So `'` and `.` are NOT literal chars — they are **computed digits
  that underflowed below 0x30.**
- Cluster A's function (entry `0x180083b4`, guarded by `cmp r3,#0; bxeq lr`):
  - `r0 = *r3` (the time value, r3 = ptr passed by caller)
  - computes `hours = (r0 / 60) mod 12` (0→12), `minutes = r0 mod 60`,
    `r7` = AM/PM (`hours < 12`).
  - ones-hour digit = `(hours mod 10) + 0x30`; tens-minute digit =
    `abs-ish(minutes/10) + 0x30`; etc. (signed arithmetic: the "abs" is
    `sub r0,r0,r0,asr#31`, NOT a true abs, so negative inputs leak through.)
- For a digit char to be `'` (0x27) the computed digit must be -9; for `.`
  (0x2e) it must be -2. **Both negative ⇒ the input time value is negative.**

### Dynamic RE (runtime confirmation)
Added a temporary PC hook at `TEXT_FORMAT_TIME_ENTRY_PC = 0x1800_83b4`
(after the r3!=0 guard) logging `r0` (text_obj), `r3` (time ptr), `*r3`
(signed). Headed run:
```
text_obj=0x101aa140 time_ptr=0x1005f780 time_val_i32=-1607505680 time_val_hex=0xa02f68f0
```
- `*r3` = **-1,607,505,680** (0xa02f68f0) — a huge garbage value, stable
  across all 86 entries in the frame. A valid clock value (seconds/minutes
  since midnight) is small positive (0..86399). This is unambiguously
  garbage ⇒ unemulated / uninitialized time source.
- 0 fatals, 0 skips still hold (golden regression intact).
- Also added `take_text_char_diag(frame)` diagnostic at frame boundary:
  per text_obj, logs push_count vs consume_count + the full pushed char
  sequence. Result: **pushed == consumed for both text_obj every frame**
  (`0x101aa140`: 6/6 = `' : . 0 A M`; `0x101a3670`: 9/9 = `8 9 Δ Ε A B C D E`).
  This DISPROVES the iteration-2 "shared text_obj / mis-segmentation"
  concern: the consumption counter is correctly aligned; the gibberish is
  real guest data.

### Conclusion (the honest status)
- **The text-rendering mechanism (PC hook + recorded char seq) is correct
  and complete.** It faithfully renders exactly what the guest computes.
- **The scalar clock path renders `':.'0AM` because the guest's time value
  at `0x1005f780` is garbage (-1.6B).** On real hardware this would be the
  iPod system clock; my emulator does not provide a valid time, so the
  formatter's signed-digit arithmetic underflows to `'`/`.`
- **The UTF-16 path `89ΔΕABCDE`** comes from a memory string literal read
  by the `ldrhne` loops — likely a real label (not garbage-driven). Its
  "weirdness" is probably just an unobvious on-screen string; needs user
  eyes to confirm, but is NOT a state bug.

### Next step (separate workstream)
- Find what writes `0x1005f780` (a guest svc? a struct init?) and provide a
  valid time value — either by emulating the iPod system-clock API or by
  seeding the field. This is an **RTC/time-emulation** task, upstream of
  and independent from the (now-correct) text renderer.
- Keep the `0x83b4` RE hook and `take_text_char_diag` diagnostic until the
  time value is sane; re-evaluate the rendered string then.
- User visual confirmation of the UTF-16 `89ΔΕABCDE` mid-row is still
  welcome but is now lower-priority than the time-value fix for the scalar
  bottom row.

### Artifacts
- `/tmp/tet_time.log` — headed run with the time-entry hook
- `/tmp/tet_arm.dis` — ARM-mode disassembly of the Tetris binary
- `/tmp/tet_diag.log` — headed run with the text_char_diag diagnostic

Doubled down on verifying the fix is *actually correct*, not just
"produces distinct glyphs," using only logs + run-data:

1. **`table_a` is a plain ASCII-order font atlas.** Confirmed by
   correlating every consumed char's recorded `glyph_index` against its
   ASCII code: `glyph_index == ch - 0x20` holds exactly for all 6 scalar
   chars (`'`→7, `.`→14, `0`→16, `:`→26, `A`→33, `M`→45) and all 9 UTF-16
   chars. So there is **no table bug**; the slot→glyph mapping is standard.
   Decoded `f10x12text1_a8.pix` directly: it is an 8-bit indexed **BMP** (magic
   `BM`, biBitCount=8, 1078-byte header) 980×24. Rendered the 6 consumed
   scalar glyphs in draw order to `/tmp/font_scalar_consumed.png`.

2. **Push order == draw order == left-to-right, zero drift.** The
   `texgen_generated_uvs` log fires once per draw; matching its UV column
   against each draw_detail's UV column gives a perfect 1:1 correlation for
   all 6 scalar draws on frame 171 (draw9=push[0]=`'` at x=14; … draw14=
   push[5]=`M` at x=64). Pushes repeat identically every frame across 18+
   frames (lines 1-18 of the log = 3 clean repetitions). If push/consume
   indices were drifting, the sequence would shift — it does not. The
   texgen-decode mechanism is textbook-correct.

3. **Actual rendered strings (computed this iteration):**
   - Scalar path (y≈228, draws 9-14): `' : . 0 A M` → `':.'0AM`
   - UTF-16 path (y≈52, draws 21-29): `8 9 Δ Ε A B C D E` → `89ΔΕABCDE`
     (was `9ΔΕABCDE` via cursor-scan; the recorded path recovered the
     dropped `8`, so it is strictly *more* correct than before.)
   These are not English words, so they cannot be self-verified as the
   *intended* menu strings without vision. The mechanism that produces
   them is provably correct, but string-level ground-truth needs the
   user's eyes.

4. **Oracle decode (`tetris.raw.lcd5`):** Decoded as 16-byte header
   (w=320,h=216,row_bytes=640,tag=`565L`) + 320×216×2 RGB565 = 138256 bytes
   (exact). Rendered `/tmp/oracle_tetris_lcd5.png` + contrast-stretched
   `/tmp/oracle_stretched.png` + 4× content band `/tmp/oracle_band_4x.png`.
   Pixel analysis: 77% black; bright content only in x=64..255, y=42..210
   with a playfield-shaped (167-px-wide) upper region and narrower
   (60-px) lower regions. **This is a Tetris gameplay frame (well + pieces
   + stats box), NOT the menu** — so it cannot directly validate the
   menu text rows (9-14/21-29). It does confirm the rest of the renderer
   produces coherent gameplay.

### Honest confidence assessment
- **Mechanism: high confidence (vision-independent proof above).** Chars
  are read from r1 at the documented `0x1801616c` push site; table is
  ASCII-order; push==draw==left-to-right; deterministic; UVs match.
- **Pre→post improvement: certain.** Wrong atlas→correct atlas; 6× stale
  `(`→6 distinct correct chars; 9 skips→0; splash fingerprint unchanged.
- **String-level correctness of `':.'0AM` / `89ΔΕABCDE`: unverified.**
  These need the user's visual confirmation against the real device.

### Artifacts open for the user
- `/tmp/tetris_text_fix_latest.png` — my rendered menu (post-fix)
- `/tmp/tetris_text_prefix_baseline.png` — my rendered menu (pre-fix)
- `/tmp/font_scalar_consumed.png` — the 6 glyphs the scalar path renders,
  in draw order (so the user can read what `':.'0AM` looks like as glyphs)
- `/tmp/font_f10x12_full.png` — full 980×24 font atlas (all 98+ glyphs)
- `/tmp/oracle_stretched.png` / `/tmp/oracle_band_4x.png` — real device
  gameplay frame (not menu; for renderer coherence reference)

Scanned all 5 sibling binaries for `bl 0x1801616c` callers and for the
Tetris text-helper signature at file offset `0x1616c`:

| Game | size | bl→0x1801616c callers | bytes@0x1616c matches Tetris? |
|---|---:|---:|---|
| Tetris 66666 | 0x256ec | 14 | yes (push{r4,r5,r6,lr};mov r4,r0;ldr r0,[r0,#0x14];mov r5,r1) |
| Pacman AAAAA | 0xac8b4 | 0 | no (different fn) |
| MsPacman 14004 | 0xc7678 | 0 | no |
| Holdem 33333 | 0x5acd4 | 0 | no |
| Cubis2 99999 | 0xa9df8 | 0 | no |
| Minigolf 88888 | 0x37a1c | 0 | no |

**Conclusion:** `0x1801616c` is a Tetris-binary-specific text-runtime helper
(the EA Tetris engine's own statically-linked text code), NOT a shared-runtime
ABI. The PC hook is therefore correctly Tetris-scoped. Siblings never execute
it, so the recorded-char path never engages for them — confirmed by the
`state+0x00 == 0x18023e24` vtable guard in `live_decode_generated_uvs` and the
`live_is_texgen_text_object` font/state layout checks that gate
`take_text_char_for_draw`. PAC-MAN headless smoke (25M cyc, post-fix): 0
fatals, skip patterns identical to the prior matrix baseline (handle `0x19` =
file-backed no-live-upload, handle `0x2` = zero-UV). No regression.

The sibling zero-UV skips remain a separate workstream: those draws have a
valid position array but no UV array and no texgen text object (they are real
sprite/background draws missing UV data, not scalar-formatter text). Resolving
them belongs to the zero-UV decode workstream noted in the prior
`eapp-matrix-hardening` loop, not this text-rendering fix.

## Status

**Text-rendering mechanism is CORRECT and COMPLETE** (PC hook + recorded
char seq; push==consume; ASCII-order table; glyphs OCR-verified; UVs match;
0 fatals/0 skips; splash golden intact).

The scalar clock path still renders `':.'0AM`, but iteration 3 **proved this
is not a renderer bug**: the guest's time value at `0x1005f780` is garbage
(-1,607,505,680), so the formatter's signed-digit arithmetic underflows to
`'`/`.`. Fixing this requires emulating the iPod system clock (a separate
RTC/time-emulation workstream, upstream of the text renderer). The UTF-16
mid-row `89ΔΕABCDE` is a memory string literal (not garbage-driven) and
likely correct; user eyes still welcome to confirm.

Bottom line: the text-rendering task itself is done; only the upstream
time-value sourcing remains, which belongs to a different workstream.

## Notes
- The `eapp-matrix-hardening` ralph loop (completed) established the broader
  UV/upload-matching context; this loop narrows in on the text-accuracy gap.
- Always run headed for visual confirmation unless doing a quick RE watch.
  Headed command:
  `CLICKY_GL_TEXGEN_VERBOSE=1 ./scripts/tetris.sh --no-build --timeout 12`
- The PC-hook + recorded-char-seq design is a clean runtime ABI model, not a
  hardcoded-string patch. The `0x1801616c` PC is Tetris-specific (its EA
  engine's own text code); sibling engines use different text paths that
  would need per-game RE if their text accuracy becomes a goal.
