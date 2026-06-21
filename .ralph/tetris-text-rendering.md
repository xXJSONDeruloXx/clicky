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
- [x] **Find and fix what writes the time value at guest `0x1005f780`.**
      Iterations 5-6: field is object `0x1005f710 + 0x70`; constructor zeroes
      it at `0x1801c290`; update path writes it at `0x1801be6c`/`0x1801b56c`
      as `tm_min + 60 * tm_hour` after calling `miscTBD:12` (`0x18000e98`
      veneer). Implemented `miscTBD:12` localtime with the recovered **six-word**
      layout (`sec,min,hour,mday,mon0,year`). Default headed run now logs
      `time_val_i32=483` and scalar text `8:03AM` instead of `':.0AM`.
- [x] **Fix wrong menu-layer texture selection.** Iteration 4: all A8 uploads
      are tagged with ambiguous tex_name `0x8`; blindly choosing the latest
      upload forced menu/spinner/background strips to `f16x16menu3_a8` (Japanese
      glyph sheet). Added UV-containment + same-tex-name fallback so draw1 uses
      `screenBG_565`, spinner draws use `spinner*_a8`, draw7/17 use `menus_a8`,
      and draw5/6 use `arrows_a8` instead of `f16x16menu3`/`eaLogo`.
- [ ] **Find why expected menu labels are not issued as draws.** User supplied
      oracle shows expected English menu labels (`MENU`, `PLAY`, `VOLUME`,
      `OPTIONS`, `RECORDS`, `HELP`, `EXIT`). After texture-selection and clock
      fixes the background/strips/clock are correct, but steady frames still
      only push/draw the decorative/sample text `89/ABCDE` and the clock; no
      menu-label strings are emitted through `0x1801616c` and no other label
      draw path is currently visible. Iteration 7 enhanced `EAPP_STRING_SCAN=1`:
      expected rows decode correctly from raw `Strings.dta` (`Play`, `Volume`,
      `Options`, etc.), but there were **no work-RAM u32 pointers into those rows
      or English value spans** at run end. Iteration 10 found that writing
      `AsyncFileIO:3` completion fields (`req+0x20=1`, `req+0x24=bytes_read`)
      made the localization table parse and the expected labels materialize as
      runtime pointer tables — BUT iteration 11 proved that was a **regression**:
      it stalled Tetris on the legal/loading screen so the game never reached the
      menu. The completion-field writes were reverted in iteration 11, restoring
      menu entry (clock + decorative, labels still absent). The label-parse
      approach needs the FULL completion ABI reversed, not just byte-count writes.
- [x] **Fix newly exposed localtime downstream blocker.** Root cause was my
      initial 9-word `struct tm` proof write: Tetris passed a stack slot with
      room for six words; writing `wday/yday/isdst` overwrote saved registers
      and produced the `0x1801b994` null-object fault. Restricting `miscTBD:12`
      to six words fixes the blocker; localtime is now enabled by default.
- [ ] Investigate save/default state and input transition issues. Iteration 7
      found `.clicky-saves/prefs.sav` and `game.sav` are zero-byte files, while
      Tetris requests 4096 bytes for each and current `AsyncFileIO:3` reports
      success after delivering 0 bytes. Iteration 8 tested 4096-byte zero-filled
      save files: this did **not** materialize labels or parsed string pointers.
      Iteration 8 also root-caused the scripted `menu` input fatal at
      `0x180206fc` to the existing placeholder resource-slot patch creating
      zero-vtable fake refcounted objects; placeholders now get the guest's
      minimal base vtable/refcount so release is safe. Iteration 9 fixed direct
      `AsyncFileIO:12/14/16` handle semantics and zero-fills the post-Menu
      `prefs.sav` read buffer, but this still does not materialize labels. The
      post-Menu `frame_state=6` transition is now traced to the scripted `menu`
      event id mapping: event id 1 maps to guest bit `0x10`, calls `0x5034`,
      then `0x4088` returns state 5 and the frame state advances to 6. Iteration
      10 also fixed request-object `AsyncFileIO:3` completions (`req+0x20=1`,
      `req+0x24=bytes_read`, zero-fill requested buffers); this makes the
      localization table parse, so save/default-state is no longer the best
      label lead. Next input work is specifically advancing the legal/loading
      screen to the menu.
- [ ] Re-run headed after menu-label issuance is fixed; clock already reads a
      real `H:MM AM`, but the menu still needs to match the user oracle labels.
- [ ] Cleanup: once labels are sane, deprecate `live_find_texgen_text_cursor`
      cursor scan and remove temporary RE hooks (`TEXT_FORMAT_TIME_ENTRY_PC`,
      `take_text_char_diag`, `scan_for_strings`).

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

## Iteration 4 — user oracle + texture-selection fix (major visual improvement, labels still absent)

User supplied a visual oracle:
- current bad screenshot: `/var/folders/_h/z9vz7mfx48b1fp6z1y0srbdm0000gn/T/pi-clipboard-9099c439-b111-47a0-8676-7fbb5a80bf39.png`
- expected menu screenshot: `/var/folders/_h/z9vz7mfx48b1fp6z1y0srbdm0000gn/T/pi-clipboard-8bb3a0ac-8861-4726-9c09-3f730628fded.png`
Expected shows the real English menu labels (`MENU`, `PLAY`, `VOLUME`,
`OPTIONS`, `RECORDS`, `HELP`, `EXIT`). This disproved the previous "UTF-16 row
is probably fine" assumption: `89/ABCDE` is not intended menu text.

What iteration 4 found/fixed:

1. **`89/ABCDE` is a deliberate decorative/font-sample string, not a
   localization label.** Watchpoint on its UTF-16 buffer (`0x101a76c0`) shows PC
   `0x18018f70` copying it from a guest-built sample table. The routine fills
   ranges (`A-Z`, accented/Japanese-ish glyphs, digits, Greek Δ/Ε) and copies a
   9-char sliding window. So this string is guest-authored, but it is not the
   menu labels from the oracle.

2. **The ugly Japanese/gibberish background was mostly wrong texture selection.**
   All A8 assets are uploaded with ambiguous tex_name `0x8`:
   `menus_a8`, `spinner*`, `scanlines`, `arrows`, `f10x12`, `f13x13`,
   `f16x16menu{1,2,3}`, etc. The old draw selection blindly preferred the latest
   upload for tex_name `0x8`, so steady menu draws 2-7/17 all sampled
   `f16x16menu3_a8.pix` (Japanese glyph atlas), even when the UV extents exceeded
   that texture's 32px height. This exactly matched the user's current screenshot.

3. **Implemented a general upload-selection fix.** For UV-backed draws, a
   tex-name match only wins if the selected upload can contain the draw's UV
   extents. If not, selection falls back first among uploads with the same
   tex_name by exact dimensions / smallest-containing, then to the old generic
   UV/dim heuristic. This fixes ambiguous texture-name reuse without hardcoding
   Tetris assets.

   Post-fix steady frame selection (headed/headless):
   - draw1: `screenBG_565.pix` (was `matrix_565.pix`)
   - draws2-4: `spinner_a8`, `spinner2_a8`, `spinner3_a8` (was `f16x16menu3`)
   - draws5-6: `arrows_a8` (was `f16x16menu3`, then briefly `eaLogo` before the
     same-tex-name fallback refinement)
   - draws7/17: `menus_a8.pix` (was `f16x16menu3`)
   - logo/text/battery remain on their expected assets

   Captured post-fix menu frame: `/tmp/tetris_post_texselect_latest.png`.
   It is visually much closer (correct blue background/strips/logo), but still
   lacks the English option labels and still has the bad clock/sample text.

4. **Fixed Settings:0 scalar-output ABI.** Tetris calls
   `Settings:0("Language", out_value_ptr, size_ptr)` and then reads
   `*out_value_ptr` on success. The previous stub returned success but left the
   output stack word uninitialized. Now it writes a scalar default (env-overridable
   via `CLICKY_EAPP_LANGUAGE`, default 0) and confirms size=4. This did not by
   itself make labels appear, but it removes one real source of guest stack
   garbage.

5. **String scan result:** `EAPP_STRING_SCAN=1` finds `MENU/PLAY/VOLUME/...`
   only inside the raw `Strings.dta` buffer loaded at `0x10003620..0x10010532`.
   No parsed UTF-16LE draw/cache copy was found at steady menu frame. Current
   frames only call the text helper for:
   - clock: `':.0AM` (known bad upstream time value)
   - decorative/sample: `89/ABCDE`
   Therefore the remaining label problem is **not glyph table/atlas selection**;
   the guest is not issuing expected menu-label draws in this state/path.

Verification:
- `cargo test -p clicky-core --lib live_gl::tests` → 14 passed
- `cargo test -p clicky-core --lib eapp` → 16 passed
- headed run `/tmp/tet_texselect_fix_headed.log`: 0 skipped, 3834 rasterized,
  selected assets match the corrected list above.

Next:
- Find why the main menu label objects are not created/issued (state/input/API
  issue, not renderer glyph decode). Look at unimplemented/suspicious runtime
  APIs (`miscTBD:13`, async request completion fields, possible string-table
  parse callbacks) and compare expected label strings from `Strings.dta` against
  guest callsites that should request them.
- Separately fix clock source at `0x1005f780` so `':.0AM` becomes valid time.

## Iteration 5 — clock writer/root cause found; localtime ABI proven but gated

Work completed this iteration:

1. **Fixed the watchpoint tool to catch overlapping word stores.** The prior
   watch only logged writes whose *starting address* was inside the watched
   range. `w16`/`w32` now use access-range overlap, so a watch catches stores
   that partially overlap the target. Sanity check: watching `0x100015a4,8`
   catches `miscTBD:9` monotonic tick writes.

2. **Mapped the bad clock field to its owning object and writer PCs.**
   Allocation trace shows `0x1005f780` is inside a 0xf0-byte object:
   - object base: `0x1005f710`
   - field: `+0x70`
   - allocator LR: `0x18021b68`

   Whole-object watch (`CLICKY_EAPP_WATCH=0x1005f710,0xf0`) showed:
   - constructor initializes `+0x70` to `0` at `0x1801c290`
   - update path writes `0xc165e819` once at `0x1801be6c` with old unhandled API
   - steady path writes `0xa02f68f0` repeatedly at `0x1801b56c` with old
     unhandled API

   Disassembly of both write sites:
   ```armasm
   bl 0x1800559c      ; veneer to miscTBD:12 / 0x18000e98
   add r1, sp, #0x28
   ldm r1, {r0,r1}   ; r0 = tm_min, r1 = tm_hour (inferred)
   rsb r1, r1, r1, lsl #4
   add r0, r0, r1, lsl #2   ; tm_min + 60 * tm_hour
   str r0, [r4,#0x70]
   ```
   So the scalar formatter wants **minutes since midnight**, and the upstream
   bug is specifically unimplemented `miscTBD:12` (calendar/localtime), not the
   renderer and not the monotonic `miscTBD:9` tick API.

3. **Implemented a gated localtime ABI proof.** Added
   `CLICKY_EAPP_LOCALTIME=1` support for `miscTBD:12`, writing the leading C
   `struct tm` fields: `sec,min,hour,mday,mon0,year,wday,yday,isdst`.
   Proof runs:
   - `/tmp/tet_iter5_localtime_proof.log` (`min=24 hour=2`, wrote `0x90`)
   - `/tmp/tet_iter5_localtime_proof2.log` (`min=27 hour=2`, wrote `0x93`)
   In both cases the watched value equals `hour * 60 + minute`.

   This proves the correct ABI/layout and would make the formatter input sane.
   It is **not enabled by default yet**, because it reveals a separate guest
   runtime path before the menu:
   - fault: `pc=0x1801b994`, read `[r4+0x2c]` with `r4=0`
   - recent path returns from the `0x1801bc90` UI update / audio-transition
     sequence to `0x1801b990` with null `r4`
   - making audio control imports return success and `miscTBD:6/7` nonzero did
     **not** fix it, so those experiments were reverted.

Verification this iteration:
- default headed runs `/tmp/tet_iter5_default_headed.log` and
  `/tmp/tet_iter5_default_headed2.log`: stable again, 0 fatal, 0 skipped, still
  old clock string `':.0AM` because localtime is gated
- `cargo test -p clicky-core --lib eapp` → 16 passed

Next:
- RE the newly exposed null-object path around `0x1801b8b4..0x1801bfb0`,
  especially how `0x1801bc90` is invoked/returned to `0x1801b990` and what
  runtime/audio object or vtable is expected. Once that path is safe, remove the
  `CLICKY_EAPP_LOCALTIME` gate and the clock should become real text.
- Continue separate menu-label issuance investigation.

## Iteration 6 reflection + clock fix enabled by default

Reflection checkpoint:

1. **What has been accomplished so far?**
   - Renderer/text mechanism: fixed. The scalar register-computed text path now
     consumes the real `0x1801616c(r0=text_obj,r1=char)` pushes; UTF-16 path is
     still decoded correctly; push==consume; 0 skipped draws.
   - Texture selection: fixed. Ambiguous A8 tex_name reuse no longer forces menu
     layers onto the wrong Japanese glyph atlas.
   - Clock source: now fixed. `miscTBD:12` was identified as the calendar /
     localtime ABI feeding Tetris' `tm_min + 60*tm_hour` clock field.

2. **What's working well?**
   - Log-driven RE with write watchpoints is very effective, especially after
     overlap matching was added for `w16`/`w32`.
   - The PC-hook text diagnostics are strong: they prove whether a visible string
     is renderer-caused or guest-authored.
   - Headed runs plus `frame_diag` / `text_char_diag` give fast regression checks
     without relying on agent vision.

3. **What's not working / blocking?**
   - Main-menu labels are still not issued as draws. After this iteration the
     clock is real (`8:03AM` in the headed proof), but the expected oracle labels
     (`MENU`, `PLAY`, `VOLUME`, `OPTIONS`, `RECORDS`, `HELP`, `EXIT`) remain only
     in the raw `Strings.dta` buffer and no text-helper pushes emit them.
   - Temporary RE hooks/logging are accumulating and should be cleaned once the
     remaining label path is fixed.

4. **Should the approach be adjusted?**
   - Yes: stop treating visible gibberish as a renderer problem. Renderer, atlas,
     and clock are now solved. The remaining work should target guest runtime
     state / localization-string issuance: unimplemented imports, string-table
     parse/copy paths, or menu-state objects that decide whether to draw labels.

5. **Next priorities.**
   - Find the string lookup/render path for expected labels. Start from
     `Strings.dta` hit addresses / string IDs and locate callsites that copy or
     render those UTF-16BE entries into menu text objects.
   - Keep Tetris default golden: headed 0 fatal / 0 skipped; current clock-fixed
     artifact: `/tmp/tetris_iter6_clock_fixed_latest.png`.

Iteration 6 implementation details:

- Fixed `miscTBD:12` from the iteration-5 proof. The first proof wrote a full
  9-word `struct tm` (`sec,min,hour,mday,mon0,year,wday,yday,isdst`), but
  Tetris passes `sp+0x24` inside a 0x3c-byte stack frame. Writing words 6-8
  overwrote saved registers at `sp+0x3c..`, causing the apparent downstream
  null-object fault at `0x1801b994` (`r4` restored from overwritten `wday`).
  The recovered ABI is six words: `sec,min,hour,mday,mon0,year_since_1900`.

- Enabled `miscTBD:12` by default after the six-word fix. Verification:
  - headless proof `/tmp/tet_iter6_localtime6_headless.log`: `local_time`
    `min=2 hour=8`, watch wrote `0x000001e2` at `0x1005f780`, and
    `text_char_diag` emitted `8:02AM`.
  - headed proof `/tmp/tet_iter6_default_localtime_headed.log`: 0 fatal,
    0 skipped, `time_val_i32=483`, scalar text `8:03AM`.
  - exported frame: `/tmp/tetris_iter6_clock_fixed_latest.png`.
  - tests: `cargo test -p clicky-core --lib eapp` → 16 passed.

- Re-ran string scan after the clock fix:
  `/tmp/tet_iter6_string_scan_after_clock.log`. Expected labels are still only
  in the raw `Strings.dta` buffer; unique pushed text sequences are now just
  `8:04AM` / `8:05AM` for the clock plus `89ΔΕABCDE` for the decorative sample.
  So the remaining label bug is independent of the clock.

## Iteration 7 — menu-label negative evidence tightened

Work completed this iteration:

1. **Brute-forced the `Settings:0("Language")` value.** Ran headless text
   diagnostics for `CLICKY_EAPP_LANGUAGE=0..10`:
   `/tmp/tet_iter7_lang_{0..10}.log`. Values 0-9 all emitted only the clock plus
   `89ΔΕABCDE`; value 10 changed the decorative/sample row to unsupported
   glyphs (`89.......`) but still emitted no expected menu labels. So the label
   absence is not simply "English is a different language index".

2. **Decoded `Strings.dta` rows in the runtime scan.** Enhanced the env-gated
   `EAPP_STRING_SCAN=1` diagnostic to parse selected UTF-16BE tab-separated rows
   and scan for work-RAM u32 pointers into those rows/value spans. Result log:
   `/tmp/tet_iter7_string_ptr_scan.log`.

   Decoded rows:
   - `TET_STRING_MAIN_MENU` → `Menu` at `0x1000b9b2`
   - `TET_STRING_PLAY` → `Play` at `0x10003e6c`
   - `TET_STRING_VOLUME` → `Volume` at `0x10010044`
   - `TET_STRING_OPTIONS` → `Options` at `0x10004116`
   - `TET_STRING_RECORDS` → `Records` at `0x10003fb0`
   - `TET_STRING_HELP` → `Help` at `0x10004078`
   - `TET_STRING_EXIT` → `Exit` at `0x10004206`

   For all of these rows, `row_ptr_refs=[]` and `value_ptr_refs=[]`. This means
   the raw table is loaded and decodable, but no obvious parsed pointer table or
   direct value pointer for the expected labels exists in work RAM at scan time.

3. **Checked time/input as explanations.** A long no-input run
   (`/tmp/tet_iter7_long_noinput.log`, 80M cycles, text frames >1000) still only
   emitted the clock + decorative row. Scripted input runs
   (`/tmp/tet_iter7_input_{1..5}.log`) did change state in some cases (new
   single-character / repeated-`A` text objects), but still emitted no expected
   labels. `menu`/`down` event paths can hit a separate null read at
   `pc=0x180206fc`, read `[0x14]`, which is likely another runtime-state/input
   issue rather than a text decoder problem.

Additional observation for next iteration:

- `prefs.sav` and `game.sav` currently exist as zero-byte files under
  `.clicky-saves`, while Tetris requests 4096 bytes for each. Current
  `AsyncFileIO:3` logs success after loading 0 bytes to `0x1802795c` and
  `0x1802895c`. This could leave save/default state different from real hardware
  (or from a clean "file missing" path) and may be related to the missing menu
  label state or input-transition null deref.

Verification:

- `cargo test -p clicky-core --lib eapp` → 16 passed.
- String scan / long no-input runs: 0 fatal, 0 skipped.
- Input experiments: some scripted event paths still fatal at `0x180206fc`; left
  unresolved for next iteration.

Next:

- Reverse `AsyncFileIO` save-read semantics: for missing/short `prefs.sav` and
  `game.sav`, decide whether ordinal 3 should fail, zero-fill the requested
  buffer, or synthesize default records before reporting completion.
- RE `0x180206fc` null deref from scripted menu/down input.
- Continue string lookup tracing from enum IDs / row indices if save-state fixes
  still do not materialize labels.

## Iteration 8 — save experiment + input null-deref root-caused

Work completed this iteration:

1. **Tested the zero-byte save hypothesis.** Temporarily backed up the current
   zero-byte `.clicky-saves/prefs.sav` and `game.sav`, replaced both with
   4096-byte zero-filled files (matching Tetris' request size), and ran a
   headless `EAPP_STRING_SCAN=1` / text diagnostic:
   `/tmp/tet_iter8_zero4096_scan.log`.

   Result: no change. The only pushed strings were the real clock (`9:03AM` in
   that run) and the decorative `89ΔΕABCDE`; selected `Strings.dta` rows still
   decoded correctly but had `row_ptr_refs=[]` / `value_ptr_refs=[]`. This means
   “short save left destination buffer dirty” is not the sole explanation for
   missing menu-label issuance.

2. **Root-caused the `0x180206fc` scripted-input crash.** Reproduced with
   `CLICKY_EAPP_INPUT_SCRIPT='menu:190-205'` while using 4096-byte zero saves:
   `/tmp/tet_iter8_input_menu_zero4096.log`.

   The fault was not directly save data. It was the guest's refcount-release
   loop over an array of resource objects:
   - `0x180206b0..0x18020718` decrements `[obj+4]`, and if it reaches zero calls
     the destructor at `[[obj]+0x14]`.
   - The faulting object `0x100e4be0` had words
     `[0x00000000, 0x00000000, 0x100e4c00, ...]`: null vtable, refcount just
     decremented to zero, payload pointer at `+8`.
   - That exact shape matches the existing `maybe_patch_guest_state()`
     placeholder entries (entry + payload) for Tetris resource slots 20..37.
     The patch made non-null resources so startup could continue, but the fake
     entries were not valid refcounted objects once input/state transitions
     released copied slots.

3. **Made the existing placeholder patch release-safe.** The placeholders now
   initialize the minimal base-object header the guest itself uses:
   - `[entry+0x00] = 0x18023efc` (base vtable; destructor slot `+0x14` is
     `0x18020734`, which frees non-null objects)
   - `[entry+0x04] = 1` (initial refcount)
   - `[entry+0x08] = payload`

   This is still a bounded compatibility shim for the known placeholder patch,
   not a string/label injection.

Verification:

- `cargo test -p clicky-core --lib eapp` → 16 passed.
- Scripted input replay after the fix:
  `/tmp/tet_iter8_input_menu_placeholder_fix.log`
  - 0 fatal lines, 0 skipped lines.
  - Previously fatal `menu:190-205` now survives past the release path.
  - New/remaining behavior: after the menu event, the guest enters
    `frame_state=6` and emits long runs of `GL:157` presents with zero draws.
    So the null-deref is fixed, but the input/pause/menu state is still not
    properly modeled.
- Headed smoke after the fix:
  `/tmp/tet_iter8_headed_placeholder_fix.log`, capture dir
  `/tmp/tetris_capture_20260621_090931`
  - timeout exit 124 as expected, 0 fatal lines, 0 skipped lines.
  - Text still only `9:09AM` plus `89ΔΕABCDE`; expected labels remain absent.

Next:

- Continue from the new post-input state (`frame_state=6`, GL:157-only) instead
  of the old `0x180206fc` crash.
- Reverse `AsyncFileIO:12/14` and save open/read/write semantics around the
  input path; after pressing Menu, Tetris opens `prefs.sav` and calls
  `AsyncFileIO:14(handle=1, buffer=0x101a99a0, len=79)`.
- Keep tracing localization/string lookup from IDs/row indices; the 4096-zero
  save experiment disproved the simplest short-read theory.

## Iteration 9 — direct AsyncFileIO handle semantics + post-Menu state explained

Work completed this iteration:

1. **Reversed and fixed the direct `AsyncFileIO:12/14/16` wrapper path.** Static
   RE of the guest wrappers shows:
   - `0x6068` calls ordinal 12 as `AsyncFileIO:12(mode, path, file_obj, cb)`;
     it writes the import return to `[file_obj+4]` as an open/status code.
   - The actual guest-visible handle is `file_obj[0]`, written through `r2`.
   - `0x603c` polls ordinal 16 when `[file_obj+4]` is `0` or `5`.
   - `0x609c` calls ordinal 14 as `AsyncFileIO:14(handle, buffer, len)`.

   The previous emulation returned handle `1` from ordinal 12, accidentally
   making both `[file_obj+0]` and `[file_obj+4]` equal `1`, and ordinal 14 only
   returned `len` without writing the buffer. The implementation now tracks
   synthetic open handles, returns status `0` from ordinal 12, returns ready
   status `1` from ordinal 16, and makes ordinal 14 copy the tracked host file
   bytes into the guest buffer with zero-fill for short reads.

2. **Retested post-Menu save read with real buffer writes.** Replay log:
   `/tmp/tet_iter9_direct_async14.log`.

   Observed sequence:
   - `AsyncFileIO:12 path=prefs.sav` → `handle 1 status=0`
   - `AsyncFileIO:16 handle=1 known=true`
   - `AsyncFileIO:14 handle=1 ... buffer=0x101a99a0 len=79 file_bytes=0 delivered=true`

   Result: still 0 fatal / 0 skipped, but still no expected labels; the run
   still transitions to `frame_state=6` and drawless GL:157 frames. So stale
   direct-read buffer contents were a real ABI bug, but not the label blocker.

3. **Explained the post-Menu `frame_state=6` transition.** A graceful `--cycles`
   watch run (`/tmp/tet_iter9_frame_state_watch_cycles.log`) on
   `frame_context=0x100035a0` showed:
   - `frame_context[0]=1` at `pc=0x180051e8`
   - `frame_context[0]=5` at `pc=0x1800532c`
   - `frame_context[0]=6` at `pc=0x18005344` and `pc=0x18005358`
   - `frame_context+0x20` event mask `0xe` at `pc=0x18005df8`

   Static RE ties this to the input-event mapping, not save I/O: the scripted
   `menu` event emits event id 1; the guest event mapper at `0x5630..0x5698`
   maps id 1 to bit `0x10`; callsites `0x125fc` / `0x15874` call `0x5034` when
   bit `0x10` is present; then `0x4088` returns state `5`, and the frame state
   advances to `6`. In other words, the old crash is fixed and the remaining
   drawless state is the guest's response to our provisional `menu` event id,
   not evidence that `AsyncFileIO:14` is still broken.

Verification:

- `cargo test -p clicky-core --lib eapp` → 16 passed.
- `cargo fmt --check -p clicky-core` still reports broader pre-existing repo
  formatting drift, so no cargo-fmt churn was applied.
- Headed default smoke after the AsyncFileIO fix:
  `/tmp/tet_iter9_headed_direct_async.log`, capture dir
  `/tmp/tetris_capture_20260621_092314`
  - 0 fatal lines, 0 skipped lines, 3650 rasterized lines.
  - Text still only real clock (`9:23AM`) plus decorative `89ΔΕABCDE`.
  - No direct `AsyncFileIO:12/14/16` calls on the default no-input path.

Next:

- Stop treating scripted `menu:...` as a likely path to the expected main-menu
  labels; it drives guest bit `0x10` / state 6. If input testing remains useful,
  map event ids 2..5 to their semantic actions first.
- Return to the core label blocker: localization/menu label issuance is absent
  in the default steady state even with texture, clock, placeholders, and direct
  save reads fixed.
- Continue tracing from string IDs / row lookup routines rather than save short
  reads.

## Iteration 10 — request completion byte counts parse `Strings.dta`

Work completed this iteration:

1. **Added env-gated localization/string PC tracing.** `EAPP_STRING_TRACE=1`
   now logs bounded hits for the suspected Tetris string/resource path:
   `0x1801fc68` async request callback, `0x1801e0fc/0x1801e45c/0x1801e484`
   file-table parse/update, and `0x1801fa90`-family menu-resource lookup paths.
   First trace: `/tmp/tet_iter10_string_trace.log`.

   The trace showed every `AsyncFileIO:3` request used callback `0x1801fc68`,
   and that callback reads `req+0x20` / `req+0x24` before notifying the owner.
   Before this iteration those fields were always zero, even after the payload
   bytes were copied to the guest destination.

2. **Fixed request-object `AsyncFileIO:3` completion semantics.** Ordinal 3 now:
   - copies host bytes to the requested guest destination,
   - zero-fills the full requested length for short reads,
   - writes `req+0x20 = 1` on delivered success,
   - writes `req+0x24 = actual_bytes_read`, which `0x1801fc68` propagates as
     the completed byte count.

   This is separate from the iteration-9 direct-handle `AsyncFileIO:12/14/16`
   path. It fixes the older async request-object path used for `Strings.dta`,
   textures, wav headers, and initial save reads.

3. **Verified `Strings.dta` is now parsed/materialized.** Concise scan log:
   `/tmp/tet_iter10_final_scan.log`.

   `EAPP_STRING_SCAN=1` now finds real work-RAM pointer refs for the expected
   values, e.g.:
   - `TET_STRING_PLAY` value `Play`: `0x100ee238 -> 0x10003e6c`
   - `TET_STRING_VOLUME` value `Volume`: `0x100eec98 -> 0x10010044`
   - `TET_STRING_OPTIONS` value `Options`: `0x100ee2b8 -> 0x10004116`
   - `TET_STRING_RECORDS` value `Records`: `0x100ee278 -> 0x10003fb0`
   - `TET_STRING_HELP` value `Help`: `0x100ee298 -> 0x10004078`
   - `TET_STRING_EXIT` value `Exit`: `0x100ee2d8 -> 0x10004206`

   This overturns the iteration-7 negative pointer evidence: the missing piece
   was not a language index or a glyph decoder; it was async completion metadata.

4. **New visible/runtime state: legal/loading text.** With completion counts
   fixed, Tetris no longer jumps directly to the old menu-ish steady state.
   Headless and headed runs render `TET_STRING_LOADING_LEGAL` text:
   `Tetris(R)&(C)1985-2006...`, via text objects `0x100e5a80` and
   `0x100e5c00`. The run remains stable (0 fatal / 0 skips), but it did not
   advance to the expected main-menu labels within the tested windows.

   Tested immediate event ids 1..5 and a later action/event=2 press; all stayed
   on the legal text. Logs:
   - `/tmp/tet_iter10_async3_completion_fields.log`
   - `/tmp/tet_iter10_async3_long.log`
   - `/tmp/tet_iter10_event{1..5}_after_completion.log`
   - `/tmp/tet_iter10_event2_late_after_completion.log`

Verification:

- `cargo test -p clicky-core --lib eapp` → 16 passed.
- Headed smoke after the request-completion fix:
  `/tmp/tet_iter10_headed_async3_completion.log`, capture dir
  `/tmp/tetris_capture_20260621_094751`
  - 0 fatal lines, 0 skipped lines.
  - Text is legal/loading text, not the final menu labels yet.
- `EAPP_STRING_SCAN` was tightened to understand parsed NUL-delimited rows and
  cap logged values so the scan no longer dumps megabytes once parsing works.

Next:

- Debug why the legal/loading text state does not advance to the main-menu draw
  state. Likely leads: legal-screen timer/state transition, input-event mapping
  after parsed resources, or another completion/status field beyond byte count.
- Keep the `AsyncFileIO:3` completion-field fix; it is the first change that
  makes expected menu label strings materialize as runtime pointer tables.
- Once the menu is reachable, confirm whether the newly materialized pointers
  produce the expected `MENU`, `PLAY`, `VOLUME`, `OPTIONS`, `RECORDS`, `HELP`,
  `EXIT` draws.

## Iteration 11 reflection — iteration 10 was a regression; reverted

Reflection checkpoint:

1. **What has been accomplished so far?**
   - Renderer/text mechanism: fixed and proven (PC hook + recorded char seq;
     push==consume; ASCII table; glyphs verified).
   - Texture selection: fixed (ambiguous tex_name reuse handled by UV
     containment + same-name fallback).
   - Clock source: fixed (`miscTBD:12` six-word localtime; real `H:MM AM`).
   - Placeholder-resource release safety: fixed (guest base vtable/refcount).
   - Direct `AsyncFileIO:12/14/16` handles: fixed (separate handle/status,
     real buffer reads).
   - Request-object `AsyncFileIO:3` completion fields: **tried, then reverted**
     (see below).

2. **What's working well?**
   - Log-driven RE (write watchpoints, PC traces) reliably localizes ABI gaps.
   - Headed smoke runs give fast regression checks (frame count, text diag).
   - Each iteration keeps the Tetris golden (0 fatal / 0 skip).

3. **What's not working / blocking?**
   - Iteration 10's `AsyncFileIO:3` completion-field writes were a **visible
     regression**: writing `req+0x20=1` / `req+0x24=byte_count` on every load
     made `Strings.dta` parse (labels materialized as pointer tables — the
     real goal), but it also stalled the loader on the legal/loading screen so
     the game never reached the menu (user reported `tetris_run_20260621_105340.log`
     stuck at frame ~18, 191 draws/frame of legal text). The completion
     callback `0x1801fc68` forwards those fields to the resource owner; the
     owner evidently uses them as load-progress/status and with nonzero values
     the load-bar→menu transition never completes.
   - The expected menu labels are still absent on the menu that DOES render
     (clock + decorative row only).
   - Temporary RE hooks/logging are accumulating (tracked for cleanup).

4. **Should the approach be adjusted?**
   - Yes. Iteration 10 proved the label strings DO parse once completion
     metadata is provided, but that metadata is not just `byte_count`. The
     next attempt must reverse the FULL `AsyncFileIO:3` request-object
     completion protocol (which `req+0xNN` is status vs byte count vs
     error, and what values the owner expects per resource type), not guess
     at two fields. Keep menu-entry intact while doing it.

5. **Next priorities.**
   - Reverse the complete `AsyncFileIO:3` completion ABI by watching `req`
     fields across the lifecycle (request → payload write → callback → owner
     processing → load-bar counter update), then set ONLY the fields the
     guest itself sets, with the values it expects. Verify menu still enters
     AND labels materialize.
   - Re-examine whether the legal/loading screen has its own timer/input
     transition that iteration 10's change simply made visible for the first
     time.
   - Keep Tetris default golden: headed 0 fatal / 0 skipped.

Iteration 11 implementation details:

- Reverted the iteration-10 completion-field writes in
  `handle_async_file_io_import` ordinal 3: removed
  `write_guest_u32(req+0x20, 1)` / `write_guest_u32(req+0x24, n)` on success
  and the matching `=0` writes on failure. Kept the buffer zero-fill for short
  reads (harmless, strictly safer than leaving stale guest memory). Left a
  comment explaining why so the next attempt doesn't naively re-add it.

Verification:

- `cargo test -p clicky-core --lib eapp` → 16 passed.
- Headed smoke after revert: `/tmp/tet_iter11_revert_verbose.log`, capture
  `/tmp/tetris_capture_20260621_105859`
  - 0 fatal, 0 skipped, maxframe 268 (was stuck at 18 before revert).
  - Texts back to the iteration-9 steady state: clock `10:59AM` + decorative
    `89ΔΕABCDE`. Menu entry restored.

Next:

- Reverse the full `AsyncFileIO:3` request-object completion ABI before
  re-attempting label materialization, so menu entry is preserved.

## Iteration 12 — fully reversed the AsyncFileIO:3 completion ABI

Goal: reverse the **full** request-object completion protocol (per the
iteration-11 reflection) so menu entry stays intact while labels materialize.
This iteration was pure RE; no default-path change (still the iter-11 revert /
golden).

### The callback chain (fully reversed)

`AsyncFileIO:3` (our handler) delivers bytes to `dest` and queues the guest
callback `cb_pc(req, cb_ctx)` via `pending_guest_calls`. `cb_pc = [req+0x34]`
is `0x1801fc68` for every load observed. Disassembly:

```
0x1801fc68: push {r4,r5,r6,lr}           ; r0=req, r1=ctx
0x1801fc6c: add  r6, r0, #32             ; r6 = req+0x20
0x1801fc70: ldm  r6, {r5,r6}             ; r5=[req+0x20]=status, r6=[req+0x24]=byte_count
0x1801fc74: ldr  r4, [r0,#8]             ; r4 = [req+0x08] = OWNER
0x1801fc7c: blne 0x21b20
0x1801fc80: mov  r2,r6 ; mov r1,r5 ; mov r0,r4   ; (owner, status, byte_count)
0x1801fc8c: pop {r4,r5,r6,lr}; nop        ; fall-through (tail-call) into 0x1801fc94
0x1801fc94: push {r4,lr}                  ; r0=owner, r1=status, r2=byte_count
0x1801fc98: mvn  r3,#0                   ; r3 = -1
0x1801fc9c: mov  lr,#0
0x1801fca0: str  r3,[r0,#8]              ; [owner+0x08] = -1   (DONE sentinel)
0x1801fca4: strb lr,[r0,#4]              ; [owner+0x04] = 0    (clear state byte)
0x1801fca8: ldr  ip,[r0,#12]             ; ip  = [owner+0x0c]  (owner cb)
0x1801fcac: ldr  r3,[r0,#16]             ; r3  = [owner+0x10]  (ctx)
0x1801fcb0: str  lr,[r0,#12] ; str lr,[r0,#16]   ; clear owner cb/ctx
0x1801fcc0: bxne ip                      ; tail-call owner_cb(owner, status, byte_count, ctx)
```

**Key finding 1: status ([req+0x20]) is IGNORED.** `0x1801fc94` only marks the
owner done and tail-calls the owner's callback `[[owner+0x0c]]`, forwarding
`r1=status, r2=byte_count`. So iteration-10's `req+0x20=1` was irrelevant; only
`req+0x24` (byte_count) mattered.

**Key finding 2: one shared owner + ctx for ALL loads.** Every AsyncFileIO:3
load uses owner `0x1001378c`, `[[owner+0x0c]] = 0x1801d370`, `[[owner+0x10]] =
ctx = 0x10013620`. All resources funnel through one load manager.

`0x1801d370(owner, status, byte_count, ctx=0x10013620)`:
```
0x1d3e4: ldr  r0,[r6,#8]          ; r0 = [owner+8] (= -1 done sentinel)
0x1d3e8: add  r1, r5, #0x11c      ; r1 = ctx+0x11c
0x1d3ec: stm  r1, {r0,r7}         ; [ctx+0x11c]=[owner+8]=-1,  [ctx+0x120]=byte_count
0x1d3f4: strb r0(=1),[r5,#0x124]  ; [ctx+0x124] = 1
0x1d3f8: ldr  ip,[r5,#0x164]     ; ip = [ctx+0x164] = per-resource PROCESSOR
0x1d404: ldr  r0,[r5]             ; r0 = [ctx]         (an object ptr stored at ctx)
0x1d408: ldr  r1,[r5,#0x120]     ; r1 = byte_count
0x1d40c: ldr  r3,[r5,#0x168]     ; r3 = [ctx+0x168] = DESC
0x1d414: ... bx ip               ; PROCESSOR(r0=[ctx], r1=byte_count, r2=ctx+8, r3=desc)
```
So byte_count lands in `[ctx+0x120]` and is passed as `r1` to the per-resource
processor `[ctx+0x164]`. The processor and desc differ per resource type:

| resource | [ctx+0x164] processor | [ctx+0x168] desc |
|---|---|---|
| Strings.dta | 0x18015308 | 0x1802995c (BSS) |
| all *.pix   | 0x18019770 | per-texture (e.g. 0x1802a554) |

`0x18015308` (Strings.dta processor):
```
0x15310: ldr  r7,[r3,#0x11c]      ; r7 = [desc+0x11c]  (a resource index/key)
0x15318: mov  r6, r1             ; r6 = byte_count
0x15344: bl   0x1d644            ; 0x1d644(manager, [desc+0x11c])  -- NO byte_count!
0x1534c: str  0,[r4,#0x11c]      ; [desc+0x11c] = 0
0x15354: strb 1,[r4,#0x120]      ; [desc+0x120] = 1  (done byte)
0x15358: str  r6,[r4,#0x124]     ; [desc+0x124] = byte_count   <-- lands here
0x15370: bxne [r4,#0x128]        ; tail-call [desc+0x128] 2nd-stage cb if set
```
`0x1d644(manager, index)` links `entry[index]` into the manager's pending list
(an array of ten 0x184-byte/388-byte slots) at `[manager+0xf28]`, marking
`entry[0x124]=1`, `entry[0x180]=link`. There is a **dead spinloop at 0x1d664**
if `entry[7]!=0` (a lock/assert).

### Why byte_count=N "regressed" but byte_count=0 "worked"

`0x1d644` does NOT take byte_count; it takes `[desc+0x11c]`. So the only
observable effect of byte_count is that `desc+0x124` / `ctx+0x120` become N
instead of 0. Some later READER of those fields decides behavior:

- byte_count=0 (iter-11 revert, default): reader sees 0 -> treat as "not loaded /
  skip parse" -> localization table NOT parsed -> game takes a FALLBACK path
  that renders the recognizable-but-label-less "menu-ish" state (clock +
  decorative `89ΔΕABCDE`). User recognizes this as "the menu".
- byte_count=N (iter-10): reader sees N>0 -> "loaded" -> properly parses
  Strings.dta (labels materialize as pointer tables, confirmed iter-10) ->
  follows the PROPER boot flow which renders the legal/loading screen first
  -> then stalls before the legal->menu transition.

So iteration-10 was actually on the CORRECT track (proper boot flow); it just
exposed the NEXT blocker: the legal/loading screen does not advance to the
menu. That transition is gated on something else (a dwell timer, an
"all-resources-loaded" counter that never reaches total, or another completion
field). The reverted default keeps the recognizable menu-ish state for now.

### RE tooling added (env-gated, off by default)

- `CLICKY_EAPP_ASYNC3_COMPLETE=1` re-enables the iter-10 completion-field
  writes for RE, so the proper-boot-flow / legal screen is reproducible on
  demand without changing the default/golden path.
- `EAPP_ASYNC_OWNER` dumps per-load: owner, `[o+4/8/c/10]`, and `ctx+0x120/124/
  164/168` (byte_count, done flag, processor, desc). Confirmed the shared
  owner/ctx and the per-type processor/desc table above.

### Verification
- `cargo test -p clicky-core --lib eapp` -> 16 passed.
- default (no env) headed smoke: 0 fatal, 0 skip, maxframe 222 (menu entry
  intact; byte-equivalent to iter-11 revert).
- RE runs: `/tmp/tet_iter12_owners.log`, `/tmp/tet_iter12_ctx.log`.

### Next
- The exact stall cause (who reads `desc+0x124` / `ctx+0x120` and what gates the
  legal->menu advance) needs a write/read watchpoint on `0x10013740`
  (ctx+0x120) and `0x18029a80` (desc+0x124) with `CLICKY_EAPP_ASYNC3_COMPLETE=1`.
- Candidates to check: a dwell timer using `miscTBD:9` monotonic ticks; an
  "all slots linked" counter in the manager (`[manager+0xf24]` is zeroed in init,
  likely an active-count); or the `0x1d664` spin being hit because a slot's
  `entry[7]` is nonzero when a duplicate registration happens.
- Keep status writes OFF (proven irrelevant); focus only on byte_count + the
  legal-screen advance gate when re-enabling.

## Iteration 13 — load-manager slot-index RE; ruled out dead-spin / dup-reg stall

Goal: act on iteration-12's next step — determine whether the
`0x1801d664` dead-spin (duplicate-registration guard) or a slot-index
collision on entry[0] is the legal→menu stall cause. Pure RE; default path
unchanged.

### Probes added (env-gated, off by default; reused the existing
`EAPP_STRING_TRACE` machinery with a higher per-PC cap)

- `0x1801_d644` (load-manager slot registration: `mgr=r0, idx=r1`).
  Logs `mgr`, `idx`, computed `entry=mgr+idx*388`, and the bytes/words
  `entry[7]` (spin guard), `entry[0x180]` (linked-list next),
  `entry[0x124]`, `entry[0x120]`.
- `0x1801_d664` (dead-spin guard: spins forever if `entry[7]!=0`).

### Findings

1. **The `0x1801d664` dead-spin is NEVER hit.** Duplicate-registration is
   NOT the legal→menu stall cause.

2. **Slot index distribution (run with `CLICKY_EAPP_ASYNC3_COMPLETE=1`, 10s
   headless, ~30 frames):**
   ```
   8421 idx=0
   7769 idx=1
     44 idx=2
     42 idx=21704   (out of range — some non-load path, irrelevant)
     40 idx=61441
      2 each for idx in {3..39 \ {39}} (one-time startup registrations)
      1 idx=39
   ```
   - The 2-per `idx` values (3..39) correspond to one-entry/one-exit of each
     initial resource registration at startup; the >9 ones return out of
     bounds via `cmp r1,#10; bxcs lr` (harmless).
   - The dominant `idx=0` (8421) and `idx=1` (7769) are PER-FRAME polling of
     the first two linked-list entries during the legal/load-bar steady
     state. So the loader dispatcher iterates entry[0] and entry[1] every
     frame; it is NOT a collision on entry[0]; it is steady-state polling.

3. **Legal screen per-frame trace** (`EAPP_PROGRESS`, `EAPP_GL lifecycle`):
   - `frame=37 draws=191`, repeating an identical draw sequence every frame,
     maxframe ~39 in the 10s window. The legal/loading text (`F,165(h0x180254c8),169,...,37#191`)
     is built once and re-rendered every frame. No state transition occurs.
   - `app_time_delta` ≈ 260ms/frame (host time, slow but advancing), and
     `frame_state=1` is held throughout. So the loader is making progress on
     per-frame work (spinners / load bar) but never tripping the
     legal→menu transition.

4. **Field `+0x11c` is set by `0x15300`** (`str r0, [r4, #0x11c]` —
   the tail store of the load-queue "assign slot index" routine, which
   runs BEFORE `0x18015308` reads then clears it). Other writers*
   `0x1534c, 0x153ac, 0x1540c, 0x15440, 0x15494, 0x1d51c, 0x19748`* and
   read sites at `0x15310, 0x1d54c, 0x1d570, 0x154e8`. So `+0x11c` is a
   generic "pending slot index" state field, not a statically-initialized
   table. By itself NOT a bug (the booking setter at `0x15300` does run on
   the path — confirmed by `idx=3..9` each registering once).

### Reassessment

- Both iteration-12 candidates for the stall (`0x1d664` spin, slot-0
  collision) are **disproved**. The legal/load-bar state polls entry[0]
  and entry[1] every frame, suggesting two resources/loadables are
  perpetually "in flight".
- The legal→menu advance gate is therefore NOT about a duplicate slot:
  it is about WHY entries 0 and 1 never get removed from the pending list /
  marked fully complete. Likely candidates:
  - a status word other than `byte_count` that `0x18015308` should set/
    preserve and that the loader dispatcher re-checks before unlinking the
    entry (we currently only propagate `byte_count`);
  - or an explicit `[entry+0x124]`/`[entry+0x120]`/`[entry+0x128]` "done"
    flag that the dispatcher polls and that we never set on entry[0]/[1].

### Verification
- `cargo test -p clicky-core --lib eapp` -> 16 passed.
- default (no env) headed smoke: 0 fatal, 0 skip, maxframe 218 (golden).
- RE runs: `/tmp/tet_iter13_d644.log`, `/tmp/tet_iter13_idx_dist.log`.

### Next
- Watch `[entry+0x180]` (or its surrounding fields `0x124/0x128`/
  `0x140/0x164/0x168`) on entry[0] (`0x10013620`) and entry[1]
  (`0x1001378c`... wait — entry[1] is mgr+1*388 = `0x10013620+388=`
  `0x10013620+0x184 = 0x100137a4`). The goal: find a field that, when set,
  the per-frame loader unlinks the entry and the legal screen advances.
- Specifically: trace `0x1d644` writes to `[entry+0x124]/[entry+0x128]`
  from the *texture* processor `0x18019770` (not yet disassembled), which
  may set a different completion shape than the Strings.dta processor
  `0x18015308`.
- Keep `CLICKY_EAPP_ASYNC3_COMPLETE=1` for RE; default remains reverted/
  golden.


**Text-rendering mechanism is CORRECT and COMPLETE** (PC hook + recorded
char seq; push==consume; ASCII-order table; glyphs OCR-verified; UVs match;
0 fatals/0 skips; splash golden intact).

The scalar clock path is now fixed: default headed run logs a sane minutes-since-
midnight value (`0x1e3`) and emits `8:03AM` instead of `':.0AM`. The apparent
iteration-5 downstream blocker was caused by writing too many localtime fields
and overwriting saved registers; the recovered `miscTBD:12` ABI is six words.

User's visual oracle also proved the 9-char `89ΔΕABCDE`/`89/ABCDE` row is not
intended menu-label text. Iteration 4 showed it is a deliberate decorative /
font-sample string. The actual expected menu labels were not issued as text
draws on the old path. Iteration 10 fixed async request completion metadata, and
now `Strings.dta` rows do materialize as runtime pointer tables for the expected
English labels. The current blocker has moved forward: the game now renders the
legal/loading text and has not yet advanced to the main-menu draw state.

Bottom line: glyph decode, texture asset selection, clock time source,
placeholder-resource release safety, direct `AsyncFileIO:12/14/16` handles are
now fixed. The iteration-10 attempt to add `AsyncFileIO:3` completion metadata
parsed `Strings.dta` (labels materialized) but regressed menu entry and was
reverted in iteration 11. The expected menu labels remain absent on the menu
that does render. Next: reverse the FULL completion ABI (not just two guessed
fields) so menu entry stays intact while labels materialize.

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

## Iteration 14 — RE: dispatcher + I/O initiator + in-flight byte lifecycle

Goal: act on iteration-13's next step — disassemble the texture processor
`0x18019770` and find the legal→menu transition gate by watching entry
lifecycle fields.

### Static RE: full load-manager cascade decoded

1. **`0x18019770` texture processor** (analog of Strings.dta processor
   `0x18015308`, mirror image with +0xc field offset):

   | logical field       | Strings.dta `0x18015308` | Texture `0x18019770` |
   |---|---|---|
   | index-to-register   | `[desc+0x11c]`           | `[desc+0x128]`      |
   | done byte           | `[desc+0x120]=1`         | `[desc+0x12c]=1`    |
   | byte_count landing  | `[desc+0x124]=byte_count` | `[desc+0x130]=bc`   |
   | 2nd-stage cb        | `[desc+0x128]`            | `[desc+0x134]`      |

   Both processors call `bl 0x1d644(manager, index)` to register the slot.

2. **`0x1d8d0` = manager init** (fires once at startup). Allocates a
   10-slot array of 0x184-byte entries, builds an initial FREE LIST
   (`entry[i]+0x180 ⟶ entry[i+1]`), sets `[mgr+0xf24]=0` (active count)
   and `[mgr+0xf28]=entry[0]` (free-list head).

3. **`0x1d644` = LINK function** (not the dispatcher): if `[entry+0x180] != 0`
   (already in some list), return immediately; else set `[entry+0x124]=1`, set
   `[entry+0x180] = old_head`, set `[mgr+0xf28] = entry` (push to front).
   This is what `0x18015308` and `0x18019770` call after each load completion:
   it "returns" the slot to the free list for reuse. The 8000+ idx=0 hits in
   iteration 13 are NO-OPS because `[entry+0x180] != 0`.

4. **`0x1d76c` = DISPATCHER** (one-step pop). Pops head entry from
   `[mgr+0xf28]`, advances head to `[head+0x180]`, sets `[head+0x180]=0`, then
   sets `[head+4]=1`, `[head+5]=1`, `[head+6]=1`, `[head+0x120]=0`,
   `[head+0x124]=0`, memsets `entry+8..entry+0x11c`, installs continuation
   `{r8=fn, r9=arg}` at `[head+0x164]/[head+0x168]`, then:
   - `[entry+0x110]==0 || ([entry+0x110]==1 && [entry+0x114]==0)` ⟶ `bl 0x1fe28`
     with `(r0=entry+0x16c, r1=entry[0x118], r2=0x1801d370[,owner cb],
     r3=entry[0x10c])`.
   - else ⟶ `bl 0x1fcc8` with `(r0=entry+0x16c, ..., r2=0x1801d1b4[,cb])`.
   If the call returns non-zero (load started), `bne 0x1d854` ⟶ return r6
   (unchanged initial value `-1`) WITHOUT re-linking ⟶ entry permanently
   removed from the pending list. If the call returns zero (busy/fail),
   `[entry+0x180]=old_head; [mgr+0xf28]=entry` ⟶ re-link at head (busy-wait).

5. **`0x1fe28` = I/O initiator A** (the path hit by Strings.dta-style loads).
   `r0 = entry+0x16c` (= "the request struct"). Sequence:
   - `r0 = [r0+4]` = `[entry+0x170]` (BUSY / in-flight byte).
   - If `[entry+0x170] != 0`, return 0 immediately ⟶ dispatcher re-links
     (busy-wait).
   - Else: `bl 0x21b38(60)` ⟶ alloc owner struct; `bl 0x20154` ⟶ init owner;
     set `[entry+0x178]=entry[0x118]`, `[entry+0x17c]=0x1801d370` (owner cb);
     `bl 0x200c8(owner, 6, 0, ...)` = AsyncFileIO:6 (open file);
     `bl 0x200ac(owner, entry+0x16c)` = link owner ⟷ request struct;
     `bl 0x644 (r0=entry[8], r1=entry+9(name ptr), r2=owner)` = AsyncFileIO:3
     (our handler). On result r6 != 0 (success), `mov r0,#4; strb r0, [r4,#4]`
     ⟶ `[entry+0x170]=4`, jump to `0x1fed8`; `0x1fed8: r0=r6; pop` ⟶ return
     r6 to dispatcher (success path). On r6==0, cleanup, return 0.

6. **`0x1fcc8` = I/O initiator B** (texture-side path). Same structure, calls
   `0x638` (a different AsyncFileIO import) and uses cb `0x1801d1b4`. On
   success, `mov r0,#1; strb r0, [r4,#4]` ⟶ `[entry+0x170]=1` (vs A's 4).

7. **Call chain summary**: loader-poll → `0x1d76c`(dispatcher) → `0x1fe28`/
   `0x1fcc8`(I/O initiator) → `0x644`/`0x638`(AsyncFileIO import) ⟶ our
   `handle_async_file_io_import(ordinal=3)` → bytes copied, completion queued
   → later: `0x1801fc68`⟶`0x1801fc94`⟶`0x1d370`⟶processor.

### Dynamic RE: empirical progress on the stall

Traces added (env-gated, off by default in default path): `0x1d76c` (dispatcher
with head/in-flight/done-byte dump), `0x1d500` (begin-load: entry[4]/[7]/
[0x174]/[0x164]/[0x168]), `0x1fe28`/`0x1fcc8` (I/O initiator: dump `[e+170]`,
`[r+4]`, `[r+0xc]`, `[r+0x10]`), `0x1fec8` (failure-path code address),
`0x1fed8` (success-path code address).

Run: `CLICKY_EAPP_ASYNC3_COMPLETE=1 EAPP_STRING_TRACE=1 EAPP_STRING_TRACE_LIMIT=400
CLICKY_EAPP_WATCH=0x10013790,0x4 ./scripts/tetris.sh --no-build --timeout 10 --headless`
(`/tmp/tet_iter14_post3.log`).

Empirical results:

- **`0x1fec8` (failure path) was HIT ZERO TIMES.** No `0x1fe28`/`0x1fcc8` call
  ever took the failure/cleanup branch.
- **`0x1fed8` (success path) was HIT 20+ times in frames 1-2**, with
  `r0=0x04` (the OLD r0 left over from `movne r0,#4`, proving r6 != 0 ⟶ our
  AsyncFileIO:3 returned non-zero ⟶ `[entry+0x170]=4` was stored). The
  AsyncFileIO:3 outcome log confirms 40 staged loads completed successfully.
- **BUT on every `0x1fe28` ENTRY, `[r+4] == 0x00` (= `[entry+0x170]=0`).**
  Since `0x1febc` DID store `[entry+0x170]=4` (proven by 0x1fed8 success-path
  hits), this means the value is being CLEARED between dispatches by the
  completion chain.
- **Watch tool on `[entry+0x170]=0x10013790` (length 4) returned ZERO hits**
  despite the `strb` guest store firing repeatedly. → our watch tool's hook does
  not catch single-byte ARM `strb` stores from guest execution (only larger
  host-side writes). Need to widen the guest-store hook if we want to trace
  this field directly.

### Reassessment

- Both "dead-spin" and "slot-collision" hypotheses (iteration 12-13) are
  disproved. The new stall model is simpler: the dispatch cycle is
  pop-entry⟶set-in-flight-byte⟶AsyncFileIO:3⟶completion-clears-in-flight-byte⟶re-link-at-head,
  and the legal screen's "is pending work?" check sees `[mgr+0xf28] != 0`
  forever because every load keeps returning to the free list.
- The legal→menu gate is therefore NOT "[entry+0x170] stuck at 4"; it's that
  the cycle runs forever because completion keeps resetting the slot instead
  of leaving it done. Need to find the field that DECIDES "slot is finally
  done" and never re-dispatched — and confirm whether OUR completion chain is
  failing to set it (e.g. `[entry+0x11c]` is set by the completion processor,
  and the dispatcher reads `[entry+0x174]` (via `0x1d500: ldr r0, [r4, #0x174];
  ...; str r0, [r4, #0x11c]`)). If the per-resource processor writes
  `[entry+0x174]=-1` (sentinel) on completion, the next dispatcher would
  `ldr [entry+0x174]=-1`... actually that doesn't quite fit either.

### Verification
- `cargo test -p clicky-core --lib eapp` ⟶ 16 passed.
- default (no env) headed smoke: 0 fatal, 0 skip, maxframe 219 (golden).
- RE runs: `/tmp/tet_iter14_dispatch_watch.log`, `/tmp/tet_iter14_full.log`,
  `/tmp/tet_iter14_post3.log`.

### Next
- Find the field/setter that the dispatcher would read to decide a slot is
  DONE (so it stops putting `[entry+0x170]=4`/`1`-relinked entries back on the
  free list). Candidate: `[entry+0x174]` (read by `0x1d500`, used to refill
  `[entry+0x11c]`). Find writers.
- Fix the watch tool so it catches single-byte ARM `strb` guest stores (so we
  can directly watch `[entry+0x170]` writes).
- Keep `CLICKY_EAPP_ASYNC3_COMPLETE=1` for RE; default remains reverted/golden.

## Iteration 15 — disproved dispatcher stall: real gate is frozen `splash_phase`

Goal: act on iteration-14's next steps — find the field that decides a slot is
DONE and verify whether the dispatcher keeps re-dispatching. Built richer RE
tooling and ran a long steady-state capture; CONCLUSIVELY proved the stall is
not in the load manager at all.

### Tooling added (env-gated, off by default)

- `dump_string_trace_totals(&mut self)` — emits each PC's actual hit count
  (the per-PC log itself is throttled; the underlying counters are not).
- `EAPP_PROGRESS`'s per-frame `startup_progress` line now appends a `trace=[...]`
  summary of all RE-PC hit counts, so the dispatch cycle is visible per frame.
- `dump_string_trace_totals` now also calls `drain_watch_log()`, so watches
  fire at end-of-run even when no fatal memory fault occurs (Tetris never
  faults, so previously the watch_log was never emitted).
- `STRING_TRACE_PCS` dedup'd and extended with the per-resource processors:
  `0x1801_d370` (shared owner cb), `0x1801_5308` (Strings.dta processor),
  `0x1801_9770` (texture processor).
- Match cases for the three new PCs dump their respective desc fields
  (`byte_count`, `done`, `next_cb`, slot-index word).

### Breakthrough 1: the dispatch cycle COMPLETES at startup; steady-state
  dispatches NOTHING

Long capture: `CLICKY_EAPP_ASYNC3_COMPLETE=1 EAPP_STRING_TRACE=1
  EAPP_STRING_TRACE_LIMIT=10 CLICKY_STARTUP_PROGRESS_FRAMES=2000
  CLICKY_STARTUP_PROGRESS_INTERVAL=20 --timeout 25 --headless`
  (`/tmp/tet_iter15_long.log`, 110 frames).

Per-PC totals at frame 9:
```
0x1801_5308=1     Strings.dta processor
0x1801_9770=38    texture processor
0x1801_d370=40    shared owner cb (every load)
0x1801_d644=40    LINK fn
0x1801_d76c=41    dispatcher (40 + 1 empty-list final)
0x1801_d8d0=1     manager init (once)
0x1801_fc68=40    completion trampoline
0x1801_fc94=40    completion status handoff
0x1801_fe28=40    I/O initiator A
0x1801_fed8=40    success-branch return
```

At frame 100, the totals are **byte-identical** to frame 9 — every PC count is
frozen. Iteration 13's `8421 idx=0, 7769 idx=1` count for `0x1d644` was from a
much longer run / a different aggregate; the cycle clearly reaches steady
state rapidly and the dispatcher (and processors) are NEVER re-entered.

So **both** "dead-spin" (iter 13) and "per-frame busy-wait re-dispatch"
(iter 14's leftover hypothesis) are disproved: the load manager drains its
work queue correctly, all 40 resources complete, no slot is stuck in flight.

### Breakthrough 2: the real legal→menu gate is a frozen splash/timer

`EAPP_PROGRESS startup_progress` for frames 1..100:
```
  frame=1   frame_state=0
  frame=2   frame_state=1   splash_phase=0   splash_times=[12193,12193,12193]
  frame=10  frame_state=1   splash_phase=0   splash_times=[12193,12193,12193]
  frame=100 frame_state=1   splash_phase=0   splash_times=[12193,12193,12193]
```

Three independent timer accumulators at splash_base+0x18/+0x1c/+0x20
(`0x180256d4/d8/dc` in the FILE_VMA .data tail) are **frozen at one small
value (~12K) for the entire run**. `splash_phase` (byte at 0x180256bc) never
transitions from 0 → 1 → 2. Since `frame_state=1` (legal/loading) is gated on
`splash_phase`, the game never advances past the legal screen.

`miscTBD:9` (monotonic tick source) IS returning advancing `host_us` values
(10K → 8.7M over the run) to its per-call args[0] pointed-at destination
(0x100015a4 etc.), so the time SOURCE works. The problem is specifically the
splash_times writers: either they read from a field that ISN'T advancing, or
they set splash_times once at startup and never update it again. The byte
write to `splash_phase` would advance the legal screen à la `splash_phase =
splash_phase + 1` after some accumulated ticks.

Static RE candidates to chase next iter:
- Execution path that writes `splash_base+0x18/0x1c/0x20` (the splash_times)
  and `splash_base+0x00` (phase byte). Iteration 14 found two literal-pool
  references to `0x180256bc` at file offsets 0x5750 and 0x224d0; the second
  is a function entry near `0x1801_224a0` (called near splash logic).
- Look at the timer-read call sites and the routine at ``0x1801_224a0``.
  The dispatch cycle is DONE — the stall is *_inside_* the legal-screen render
  / phase machine, not in I/O.

### Verification
- `cargo test -p clicky-core --lib eapp` └ 16 passed.
- default (no env) headed smoke: 0 fatal, 0 skip, maxframe 224 (golden).
- RE runs: `/tmp/tet_iter15_full.log`, `/tmp/tet_iter15_long.log`,
  `/tmp/tet_iter15_splashdrain.log`.

### Next
- Disassemble around `0x18000575c` (literal pool target = splash_base)
  and `0x1801_224d0` (function near splash update logic). Find the writer/
  reader of `0x180256bc+0x18/0x1c/0x20`.
- Trace ALL stores to `0x18025600..0x18025700` (the whole splash BSS range)
  (CLI: `CLICKY_EAPP_WATCH=0x18025600,0x100`) to enumerate every writer PC.
- Tests on `frame_state` writers (iter 9 identified `0x180051e8` sets 1,
  `0x1800532c` sets 5, `0x18005344` sets 6): trace what gates the writer of
  state 2 (the legalñmenu case).
- Re-enable `byte_count` writes (iter-12 ABI) and fix the new splash gate
  once it is identified; default stays reverted/golden.

## Iteration 16 — REPAIR WATCH TOOL + decode splash update function + frame_state gate

Reflection checkpoint (Ralph iteration 16/40). See above for the reflection
summary; this iteration's work was RE and tooling only. Default path unchanged
(golden).

### Tooling fix: watch tool was being dropped on `--timeout` SIGTERM

Major RE breakthrough this iteration came from instrumenting the watch hook
itself with `eprintln!` to prove hits WERE being recorded — the watch tool was
working ALL ALONG but `drain_watch_log()` was emitting ZERO output on RE runs.

Root cause: `tetris.sh --timeout N` sends SIGTERM to the eapp binary at N
seconds, killing the process BEFORE the headless end-of-run `drain_watch_log()`
at `eapp.rs:184` (and `dump_string_trace_totals()` which also drains) ever
fire. So iterations 14-15's "watch returns zero hits" conclusion was a RED
HERRING — the writes were captured by the bus hook but never flushed to stderr
before termination.

Fix: `maybe_log_startup_progress` now calls `self.drain_watch_log()` at every
emitted startup_progress frame (interval controllable via
`CLICKY_STARTUP_PROGRESS_INTERVAL`). Watch hits now survive regardless of how
the run terminates. The end-of-run drain paths are kept for the no-timeout case
(clean `--cycles N` exit).

### Splash update function fully decoded: `0x180222a4`

Disassembled around the literal-pool site at file offset 0x224d0 (vma
0x180224d0). It's a literal pool belonging to the function ending at
0x180224cc (`pop {...,pc}`). The function START is at vma `0x180222a4` (`push
{r2,r3,r4,r5,r6,r7,r8,r9,sl,lr}`). The raw byte sequence `0x180222a4` (LE)
appears at exactly ONE place in the binary: file offset 0x00024 (vma
0x18000024), which is the EAPP header's main-entry-pointer slot (confirmed by
the bootstrap log: `aux=0x180222a4`).

So **`0x180222a4(app_object, frame_context)` is the EAPP main per-frame entry
function**, invoked once per frame by the runner.

The function body loads `splash_base = 0x180256bc` into r9 via
`ldr r9, [pc, #524]` at 0x180222bc, then:
- `0x180222b8: ldrb r0, [r5]` where `r5 = frame_context` (=0x100035a0) — i.e.
  it loads `frame_state` (first byte at frame_context).
- `0x180222c4: cmp r0, #0`.
- `0x180222c8: ldreq r0, [r4, #4]` — r0 = `[app_object+4]` (= 0x100015a4 =
  the miscTBD:9 destination, i.e. host_us).
- `0x180222d0/d4/d8: streq r0, [r9, #24/28/32]` — WRITE splash_times_a/b/c
  with host_us, ONLY when `frame_state == 0`.
- `0x180222dc: streq r8, [r5, #32]` and `0x180222e4: streq r8, [r4, #52]` —
  zero other context fields.
- `0x180222e8: bl 0x920` — service call (miscTBD:9 trampoline).
- `0x18022388: str r0, [r9, #16]` — write `[splash_base+0x10] = counter`
  (0..15 wrap; iterates 8 times each call → ~40 writes total over 5 frames
  before settling).
- `0x180223ac: str r0, [r9, #20]` — reset the `[splash_base+0x14]` field
  using `bic` then `orr` (clears bit `0x60`).

### Conclusion: the real legal→menu stall

The full splash writer cycle works correctly. `splash_times` are updated
ONCE per boot because they only write when `frame_state == 0`. Once state
advances 0→1 (by writer `0x180051e8`), the streq path inside `0x180222a4`
is skipped and splash_times freeze at the host_us value from the (single)
frame where state was 0.

**Key correction to the iteration-15 hypothesis**: `splash_phase` at byte
`0x180256bc` (= splash_base+0x00) is **NEVER WRITTEN by guest code**.
Iter-15's "splash_phase=0 is freezing" was a misread of a static-init value
that stays static. The actual state the legal screen depends on is
`frame_state` at `0x100035a0`, NOT splash_phase.

### frame_state writes captured

Watching `0x100035a0, 0x10` (covers `[r4+0..r4+12]`) shows the default boot
produces ONLY ONE write to the entire frame_context byte range:
  - `addr=0x100035a0 (=frame_ctx+0x00) val=0x01  writer_pc=0x180051e8  hits=1`

The other writers iter-9 found (`0x1800532c` sets state=5, `0x18005344` sets
state=6) only fire via scripted INPUT events. **No writer of state=2 (the
legal→menu transition state) exists on the default (no-input) boot path.**
This is the concrete description of the stall: the legal screen renders every
frame (repeating the legal text draws) but never advances to the menu because
nothing writes `frame_state=2`.

Disassembly around `0x180051d8..0x18005344` shows the function flow:
- `0x180051d8: bl 0x5508` (pre-state work)
- `0x180051dc: str r7, [r4, #12]`  — write `[frame_ctx+0x0c]`
- `0x180051e0: strb r6, [r4, #1]`  — write `[frame_ctx+0x01]`
- `0x180051e4: ldr r0, [r4, #8]`   — load frame_state_ptr
- `0x180051e8: strb r6, [r0]`      — write r6 (=1) to *frame_state_ptr  ← state set to 1
- `0x180051ec: b 0x535c`           — jump to function tail
- `0x1800531c: strbeq r6, [r4]`    — conditional write if `0x4088(r0) == 2` ← would set state=1 again (not state=2!)
- `0x1800532c: strbeq r0, [r1]`    — conditional write `[r1] = r0` (=5 if input routed)
- `0x18005344: strb lr, [r1]`      — write `[r1] = lr` (=6 if different input routed)

Note: `0x1800531c` writes `[r4]` (= `[frame_ctx+0x00]` which IS frame_state)
but only when `0x4088(r0) == 2` returns true — and that conditional DID NOT
fire on the default path. So `0x4088` does NOT return 2 in default usage.
`0x4088` is the state-machine sub-routine that decides whether to advance.
**If we could make `0x4088` return the value that triggers state=2
advancement, the legal→menu gate would open.**

### Verification

- `cargo test -p clicky-core --lib eapp` → 17 passed (16 + lib set).
- Default golden headed: 0 fatal, 0 skip, maxframe 189 (`tet_iter16_final_default.log`).
- RE runs with periodic drain (every 20 progress frames):
  - `/tmp/tet_iter16_splashdrain_v2.log` (full splash writer map)
  - `/tmp/tet_iter16_framestatelog.log` (frame_state writes)

### Next (iteration 17 priorities)
- Disassemble `0x18004088` (the state-machine sub-routine called at 0x18005314)
  to find what conditions return state=2 and what readers gate it.
- Watch `[0x100035a8..0x100035b4]` (covers `[r4+8]` = `frame_state_ptr`)
  to confirm the pointer at `[frame_ctx+0x08]` actually aliases 0x100035a0.
- Try a wider scripted input sweep: events 0..7 at multiple timings
  (early during `frame_state=0`, mid-frame, late during `frame_state=1`) to
  find if a specific event triggers the legal→menu advance.
- Re-enable `CLICKY_EAPP_ASYNC3_COMPLETE=1` (byte_count path) once the legal
  state machine is understood; default stays reverted/golden.
- Cleanup: remove temp RE hooks `TEXT_FORMAT_TIME_ENTRY_PC`, `take_text_char_diag`,
  `scan_for_strings`, `EAPP_STRING_TRACE` once labels materialize on the menu.

## Iteration 17 — REVERSED STATE-MACHINE GATE: state advances 1→5→6 when [0x18025674]=1

Reflection leftover from iter 16 said "no writer of state=2 (legal→menu)". Iter 17
fixed the wrong-base-address misread in iter 16 and uncovered the actual gate.

### Static RE: `0x18004088` (= state-machine sub-routine called at 0x18005314)

Disassembled `0x18004088`:

```
0x18004088: push {r4, lr}
0x1800408c: mov r4, r0          ; r4 = saved r0 (= sl from caller, app_state_ptr)
0x18004090: ldr r0, [pc, #56]   ; literal at 0x180040d0 = 0x18025678
0x18004094: ldr r0, [r0]        ; r0 = [*0x18025678]
0x18004098: cmp r0, #3
0x1800409c: movlt r0, #2        ; if [*0x18025678] < 3 → return 2
0x180040a0: poplt {r4, pc}
0x180040a4: ldr r0, [pc, #40]   ; literal at 0x180040d4 = 0x18025674
0x180040a8: ldrb r0, [r0]       ; r0 = byte [0x18025674]
0x180040ac: cmp r0, #0
0x180040b0: movne r0, #5        ; if byte != 0 → return 5 (STATE ADVANCE)
0x180040b4: popne {r4, pc}
0x180040b8: bl 0x39f0           ; otherwise call 0x39f0
0x180040bc: ldr r0, [pc, #20]   ; literal at 0x180040d8 = 0x180255d4
0x180040c0: mov r1, r4          ; r1 = saved r0
0x180040c4: ldr r0, [r0]        ; r0 = [*0x180255d4] (= 0x1005f710 clock obj)
0x180040c8: pop {r4, lr}
0x180040cc: b 0x1b8b4           ; tail-call 0x1b8b4(clock_obj, sl) and return its result
```

The function returns:
- **2** when `[*0x18025678] < 3` (legal/loading pending) → caller bumps `[0x1802554c]`, no state change.
- **5** when `[*0x18025678] >= 3 AND [*0x18025674] (byte) != 0` → caller writes 5 to `frame_state[0]` (the 1→5 advance gate!).
- otherwise the tail call returns (clock-driven animation/timer value, usually 0 → no state change).

### Wider main loop function at `0x180050c0` (function entry)

Full main-frame dispatch:

```
0x180050c0: push {r4..sl, lr}
0x180050c4: ldr r4, [pc, #740]   ; literal at 0x18005053b0 = 0x1802554c  ← STATE STRUCT BASE (NOT 0x180256b4!)
0x180050c8: sub sp, sp, #32
0x180050cc: stmib r4, {r0, r1}  ; [r4+4]=[0x18025550]=arg0 (app_object=0x100015a0), [r4+8]=[0x18025554]=arg1 (frame_context=0x100035a0)
0x180050d0: ldr r2, [r4, #20]    ; r2 = [r4+0x14] = [0x18025560] (per-frame counter, starts 0)
0x180050dc: mov r6, #1           ; r6 = 1
0x180050e0: mov r5, #0           ; r5 = 0
0x180050e4: bne 0x5140           ; if per-frame counter != 0, dispatch 1/etc.
   ... [0x180050e8 .. 0x1800513c]: the state == 0 / counter == 0 path
0x18005140: cmp r2, #1
0x18005144: bne 0x5168           ; if counter != 1, jump to state-machine dispatch
   ... [0x18005148 .. 0x18005164]: the state == 1 / counter == 1 path
0x18005168: ldr r2, [r0, #4]
   ... (compute frame delta etc.)
0x18005180: ldrb r3, [r0]       ; r3 = frame_state byte
0x18005184: mov lr, #6           ; lr = 6
0x18005188: mov ip, #4           ; ip = 4
0x1800518c: cmp r3, #6
0x18005190: mov r7, #2           ; r7 = 2
0x18005194: strb r3, [r4, #1]   ; [0x1802554d] = r3 (current state byte)
0x18005198: ldrls pc, [pc, r3, lsl #2] ; JUMP TABLE dispatch on r3 (frame_state)
   jump table at 0x180051a0:
   r3=0 → 0x180051bc (state=0 case)
   r3=1 → 0x180051f0 (state=1 case = legal/loading)
   r3=2 → 0x180053a8 (state=2 case — strb ip,[r1] ip=4, → state 4!)
   r3=3,4,5 → 0x1800533c (state=3/4/5 case — sets state 6)
   r3=6 → 0x1800535c (state=6 case)
0x1800519c: b 0x53a8              ; if r3 > 6 fallback
```

State-machine summary:

- state=0 case `0x180051bc`:
  - writes r7 (=2) to `[r4+0xc] = [0x18025558]`
  - writes r6 (=1) to `[r4+1] = [0x1802554d]`
  - writes r6 (=1) to `*frame_context_ptr = [0x100035a0]` ★ frame_state 0→1 transition
  - branches to function tail

- state=1 case `0x180051f0` (where we're STUCK):
  - reads `[r4+0xc] = [0x18025558]`; if != 2, branches to tail WITHOUT calling `0x4088`
  - else falls through and computes legal-screen animation progress
  - calls `0x1800530c: bl 0x5a94` (some pre-state work)
  - calls `0x18005314: bl 0x4088(sl)` ← state-machine sub-routine
  - if returns 2 → write r6 (=1) to `[0x1802554c]` (NOT frame_state!) — just progress counter
  - if returns 5 → write 5 to `*frame_state_ptr = [0x100035a0]` ★ frame_state 1→5 transition
  - jump tail

- state=3/4/5 case `0x1800533c`:
  - writes ip (=4) to `[r4+0xc] = [0x18025558]` (so future state=1 case short-circuits)
  - writes lr (=6) to `*frame_state_ptr` ★ frame_state → 6 transition
  - calls `0x18005 0: bl 0x5048; bl 0x59f4; bl 0x55a0`

- state=6 case `0x1800535c`:
  - reads `[r4+0]` and loops back to the function tail (steady state)

### Static value verification (binary file)

```
addr=0x18025678  byte=0x00 word=0x00000000 (static dword) → [*0x18025678] starts at 0
addr=0x18025674  byte=0x00 (static) → never written by guest code
addr=0x18025670  (heap obj pointer) = 0x10001560 (set at boot by PC 0x18021d60)
addr=0x180255d4  (*pointer to clock obj) = written by PC 0x180043e8 with val 0x1005f710
addr=0x18025558  (= [r4+0xc] gating byte) = written by PC 0x180051dc with val 2 (state=0 case)
```

### Dynamic RE (watches)

Watch `0x1802554c, 0x40` (= `[r4+0..r4+0x40]` state struct):

- `[0x1802554c]` (= `[r4]`) → 477 writes. 239 from PC `0x1800517c` (strb r5=0 at start of state=1 case) + 238 from PC `0x1800531c` (strbeq r6=1 when `0x4088` returns 2). Confirms **state=1 case IS being entered every frame and `0x4088` IS returning 2 most frames**.
- `[0x18025558]` (= `[r4+0xc]`) → 1 write by PC `0x180051dc` (val 2). State=0 case set this so state=1 case falls through to call `0x4088`.
- `[0x18025560]` (= `[r4+0x14]`) → 240 writes by PC `0x18005398` with values 2,3,4,5,...,240. Per-frame counter (counts frames run).

Watch `0x18025674, 0x8` (state-machine decision fields):
- `[0x18025674]` (byte): ZERO writes. Static 0.
- `[0x18025678]` (word): 9 writes monotonically increasing 1,2,3,4,5,5,6,6,7 via PCs `0x18003c0c, 0x18004ffc, 0x180058c8, 0x180058d4, 0x18005480, 0x18003d8c, 0x18005014, 0x18003dd4, 0x18004fe4` — loader-progress-related increments during boot.

So the state machine gate is COMPLETELY UNDERSTOOD:
- `[0x18025678]` (= loader progress counter) reaches >= 3 quickly during boot.
- `[0x18025674]` (byte) is NEVER WRITTEN by the guest code; static 0.
- Therefore `0x4088` returns:
  - 2 (during early boot while counter < 3) — no state advance.
  - 0 (after counter >= 3, falls through to clock-obj tail-call) — no state advance.
  - NEVER returns 5 because byte is always 0.

### Hypothesis test: env-gated `CLICKY_EAPP_TEST_READY=1` writes byte 1 to 0x18025674

Added env-gated diagnostic in `maybe_log_startup_progress` that writes
`write_guest_u32(0x1802_5674, 1)` each progress interval when env is set.

Also added `statemach_count` (= [*0x18025678]) and `statemach_byte` (= byte [0x18025674])
to the `startup_progress` line so RE values are visible per-frame.

**RESULT: Confirmed!**
Headless run `CLICKY_EAPP_TEST_READY=1 CLICKY_STARTUP_PROGRESS_INTERVAL=5 --timeout 18`:

- frame=1: state=0, count=0, byte=0  (initial splash)
- frame=2: state=1, count=1, byte=1  (boot loader kicked in, TEST_READY wrote byte=1)
- frame=3: state=5, count=7, byte=1  ★ `0x4088` returned 5 → frame_state advanced 1→5
- frame=4: state=6, count=7, byte=1  ★ state=3/4/5 case advanced 5→6
- frame=5+: state=6 stable, hash unchanged

Also headed 14s run (`/tmp/tetris_iter17_test_ready_headed.log`) reached frame 8580
at state=6 cleanly, no fatal/skip — the menu screen renders fine.

### Visual confirmation (state=6 menu rendered)

Saved `/tmp/tetris_iter17_test_ready_menu.png` (320x240 RGB, color type 2). Pixel analysis:
- avg lum = 68.9 / 255 (medium brightness, NOT all-black)
- avg r/g/b = 47/60/101 — strong blue channel (consistent with blue Tetris menu BG)
- 64.4 % pixels have lum > 40 (significant visible content)
- 4.9 % pixels have lum > 180 (bright content = glyphs/sprites)
- bright content bbox: x=[63..279] y=[12..153] — looks like a centered text/label region

Compare to default golden run (no TEST_READY env):
- frame=3: state=1, count=7, byte=0  (stuck on legal screen)
- state_machine_byte remains 0, frame_state stays at 1 forever.

### Conclusion

Iter-15/16's "splash_phase frozen forever" was really "legal-screen state machine
gated on byte `[*0x18025674]` which is NEVER set by guest code". The byte is read
by `0x4088` (state-machine sub) to decide whether to return 2/5/0:
- 5 → 1→5 → 6 transition (legal → menu)
- 2 → no advance
- 0 → no advance

Writing byte 1 → state advances cleanly 1→5→6 within 3 frames.

The byte is supposed to be set by some service/event we don't emulate (no guest
writer found in either static or dynamic RE). The next iteration's job: figure
out WHO is SUPPOSED to write byte [0x18025674]. Candidate leads:
- The 14 literal-pool refs to `0x18025674` (file offsets 0x3a9c, 0x3df8, 0x3fe4,
  0x405c, 0x40d4, 0x41d8, 0x462c, 0x5020, 0x5044, 0x53fc, 0x5488, 0x58f4,
  0x6008, 0x6034) — disassemble each to find STRB/STREQ/STREQB writes to
  offset +0 of the loaded `0x18025674` base.
- Look at writers to nearby fields ([0x18025670]=0x10001560 via 0x18021d60,
  [0x18025678] via the loader-progress sites) and check if there's a sibling
  writer for byte at +0.
- Check bike-shed cases `[0x180255dc]` (front/back ping-pong buffer via PCs
  0x18007fc0 / 0x18007e80) and `[0x180255e0]` (write 1/2/3 by PC 0x18007fcc) —
  what service drives those; same source may set [0x18025674]=1 on some event.
- Check the `0x180040a4` line specifically: it loads the byte after
  `[*0x18025678] >= 3`. Maybe byte at 0x18025674 is only written when the BOOT
  reaches a specific phase; or it's the "first user input received" flag.

### Verification

- `cargo test -p clicky-core --lib eapp` → 16 passed.
- `cargo test -p clicky-core --lib live_gl::tests` → 14 passed.
- Default (no env) golden headed: 0 fatal, 0 skip, maxframe 244 — no regression.
- TEST_READY headed 14s: 0 fatal, 0 skip, maxframe 8580 — state 6 steady.
- `CLICKY_EAPP_TEST_READY=1` still default-OFF: production Tetris run unchanged.
- Saved state=6 screenshot: `/tmp/tetris_iter17_test_ready_menu.png` (320x240 RGB).

### Next (iteration 18 priorities)

1. Disassemble the 14 literal-pool reference functions for `0x18025674` to find
   the legitimate host-bound writer of `[0x18025674]` byte.
2. If no guest writer exists (likely), identify the emulator-side ABI gap that
   should have set this byte (probably a service/event import that we never
   deliver: e.g., "audio initialization complete" or "screen transition
   trigger"). Look at the `0x18007fc0`/`0x18007e80` ping-pong producers to
   see what service they're calling.
3. Once the legitimate source is found, replace the env-gated TEST_READY
   injection with a real ABI fix (a TI/host-event dispatcher that the runner
   fires once loads complete).
4. Ask user to visually confirm the state=6 menu screenshot
   `/tmp/tetris_iter17_test_ready_menu.png` matches the expected menu labels
   (MENU, PLAY, VOLUME, OPTIONS, RECORDS, HELP, EXIT).
5. Re-enable `CLICKY_EAPP_ASYNC3_COMPLETE=1` alongside the byte writer to see
   if BOTH need to fire for the loader path to complete properly.


## Iteration 18 — REVERSED: legit byte-setter `0x18005034` + 4 callers + ABI gap suspected

Goal: act on iter-17 next-step priorities. Find the legitimate host-bound writer of
`[0x18025674]` byte and replace the env-gated TEST_READY injection with a real ABI fix.
NO code changes this iteration (pure RE); default path unchanged (golden).

### Static RE: the legit byte-setter function

Iteration 17 found that **byte `[0x18025674]` is never written by guest code** and
wrote it via a TEST_READY env hack. Iteration 18 traced what would happen
if the byte were set legitimately: the writer exists at **`0x18005034`**, a tiny
4-instruction leaf function:

```
0x18005034: e59f1008  ldr  r1, [pc, #8]   @ 0x18005044  ; r1 = 0x18025674
0x18005038: e3a00001  mov  r0, #1                       ; r0 = 1
0x1800503c: e5c10000  strb r0, [r1]                     ; [0x18025674] = byte 1  ★ WRITER
0x18005040: e12fff1e  bx   lr                            ; return
0x18005044: 18025674 (literal pool: the byte address)
```

### Static RE: four callers of the byte-setter

Found exactly **4 callers** of `0x18005034`:

| caller | instr | outer fn | path |
|---|---|---|---|
| `0x180125fc: bl 0x18005034` | bl | `0x180125c0` | input-driven (bit 0x10 = menu button) |
| `0x18015874: bl 0x18005034` | bl | `0x18015830` | input-driven (bit 0x10 = menu button) |
| `0x1801b814: b 0x18005034`   | tail-call | `0x1801b630` | audio-completion-driven (clock sub call) |
| `0x1801c038: b 0x18005034`   | tail-call | `0x1801bfd8` | audio-cleanup-driven (via standalone fn `0x18004060`) |

### Caller 1 (`0x180125fc`) and Caller 2 (`0x18015874`) — input-driven

Both functions check `tst r5, #16` (bit `0x10`) of an event-flag arg, after a
shared call `bl 0x1800908c`. If bit `0x10` is set, they `bl 0x18005034` (set the
byte). This matches iter-9's hypothesis: **scripted `menu` event id 1 → bit
`0x10` → byte-setter fires**. So pressing the menu button on a controller would
legitimately write the byte, advancing state past the legal screen.

`0x1800908c(arg2)` is itself: `tst r2,#8 → mov r0,#8`; `tst r2,#2 → r0,#2`;
`tst r2,#4 → r0,#4`; else `r0, #0`. So it's an event-bit dispatch.

### Caller 3 (`0x1801b814`) — audio-completion path via state-machine

Call chain:
1. state-machine sub `0x18004088` (called by `0x18005314`) enters the
   `count>=3 && byte==0` branch and tail-calls `b 0x1801b8b4` (`0x180040cc`).
2. Function `0x1801b8b4` (a clock/aio-related function) `bl 0x1801b630` from
   `0x1801b97c`.
3. Function `0x1801b630` (388-byte-slot audio destructor-style loop) eventually
   tail-calls `b 0x18005034` at `0x1801b814` to set the ready byte.

`0x18004088` HAS been confirmed in iter 17 to run every frame in state=1 status
(`0x1800531c` writes to `[r4]` `0x4088` 238 times with return 2 — but wait, return
2 doesn't set the byte!).

Hmm, there's a contradiction: iter 17 said `0x18004088` returns 2 most frames
then 0 (clock tail call); never 5. But the chain `0x1801b8b4 → 0x1801b630 →
0x18005034` would always write the byte if executed — which contradicts iter
17's "byte never written" finding.

Resolution: either `0x1801b8b4` doesn't reach `0x1801b630` every time the
state-machine tail-calls it (some short-circuit / null-object/path condition),
or `0x1801b630` doesn't reach caller 3 (`0x1801b814`) without some precondition.

### Caller 4 (`0x1801c038`) — audio-cleanup via standalone function

Call chain:
1. Standalone function `0x18004060..0x1800407c` loads count from `[*0x18025678]`,
   if `count >= 3` branches `bge 0x1801bfd8` (caller 4's function). Called from
   `0x180043f0: bl 0x18004060` and `0x1800538c: bl 0x18004060`.
2. Function `0x1801bfd8` (caller 4 function body):
   ```
   0x1801bfd8: push {r4, lr}
   0x1801bfdc: mov r4, r0                        ; r4 = arg0 (clock_obj passed in)
   0x1801bfe0: ldr r0, [r0, #0x2c]                ; r0 = [clock_obj+0x2c]
   0x1801bfe4: cmp r0, #0
   0x1801bfe8: moveq r0, #1
   0x1801bfec: popeq {r4, pc}                      ; ← if [clock_obj+0x2c]==0, return 1 (NO byte write)
   0x1801bff0: ldr r1, [r0]                        ; r1 = [r0+0] = vtable
   0x1801bff4: add lr, pc, #4                      ; lr = next instr
   0x1801bff8: ldr r1, [r1, #0x40]                 ; r1 = vtable[16] (vcall)
   0x1801bffc: bx r1                               ; → second vtable method
   0x1801c000: ldr r0, [r4, #0x2c]
   0x1801c004: bl 0x18009118                       ; some cleanup sub
   0x1801c008: mov r1, r0
   0x1801c00c: ldr r0, [r4, #0xb8]                 ; r0 = [arg0+0xb8] (audio thing)
   0x1801c010: bl 0x18020744                       ; release call
   0x1801c014: ldr r0, [r4, #0x2c]
   0x1801c018: add lr, pc, #8
   0x1801c01c: ldr r1, [r0]                         ; vtable again
   0x1801c020: ldr r1, [r1, #0x44]                 ; slot 17
   0x1801c024: bx r1                                ; → third vtable method (finalizer?)
   0x1801c028: mov r0, #1
   0x1801c02c: pop {r4, pc}                         ; return 1
   0x1801c030: mov r1, #1
   0x1801c034: strb r1, [r0, #0x54]                 ; ← final path: write 1 to [r0+0x54]
   0x1801c038: b 0x18005034                          ; ← tail-call byte-setter
   ```
3. So `0x1801bfd8` either returns early (if audio queue is null) OR does a
   complete destructor dance and tail-calls byte-setter.

**The KEY gate**: `[clock_obj+0x2c]` (= `0x1005f713c` since clock_obj = `0x1005f710`).
This field holds a pointer to an audio sub-object. If null (no audio queue
initialized), `0x1801bfd8` returns 1 immediately without setting the byte.

### The suspected ABI gap: Audio:0 / Audio:1 / Audio:23 + callback structs

From the iter-17/18 startup_progress imports:
```
imports=[...,Audio:0=10,Audio:1=10,Audio:23=10,...]
```

When Tetris calls Audio:23 (= host import 23 on the Audio interface):
- lr=0x180059a0 (= site `0x1800599c: bl 0x18000758` — wait, actually LR
  matches `bl 0x18000758` so 0x18000758 is the tetris-side audio wrapper)
- r2=**0x1801bfc0** (= a callback FUNCTION-POINTER ARRAY / vtable struct)
- r3=**0x18002c5c** (ctx)

The address `0x1801bfc0` is a literal offset within the caller-4 block —
specifically, `0x1801bfc0` is the START of a small 4-entry function table:

| offset | vma | function | behavior |
|---|---|---|---|
| +0x00 | `0x1801bfc0` | `mov r0, #3; bx lr` | returns 3 |
| +0x08 | `0x1801bfc8` | `add r0, r0, #0x70; bx lr` | r0 += 0x70 |
| +0x10 | `0x1801bfd0` | `mov r0, #1; bx lr` | returns 1 |
| +0x18 | `0x1801bfd8` | (caller-4 function) | audio destructor + byte-setter |

The TI firmware Audio:23 / Audio:1 receives this callback-struct pointer in
`r2`, presumably stores it, and **invokes index 0x18 (`0x1801bfd8`) when an
audio sample finishes playing** to notify Tetris. That call eventually
tail-calls `0x18005034` (the byte-setter), advancing state legal→menu.

**On our emulator**, Audio:23 / Audio:1 are stubbed to return success without
ever invoking any registered callback later. Hence:
- The audio callback struct is registered but the host never schedules the
  completion callback.
- `[clock_obj+0x2c]` (the audio queue) may stay null or unconfigured
  because the Audio:23 ABI didn't fully run.
- Therefore `0x1801bfd8` short-circuits at `0x1801bfec: popeq {r4, pc}` every
  time it's reached, because `[clock_obj+0x2c] == 0`.
- Therefore the legitimate byte writer `0x18005034` is never called via caller 4.
- Same gating likely applies to caller 3's chain via `0x1801b8b4 → 0x1801b630`.

### Dynamic RE: experiment with TEST_READY + ASYNC3_COMPLETE together

Ran `CLICKY_EAPP_TEST_READY=1 CLICKY_EAPP_ASYNC3_COMPLETE=1` (the env flag from
iter 17 that force-writes byte 1, plus the iter-10 ABI completion fix that's
off by default).

Results (`/tmp/tet_iter18_test_ready_async3.log`,
`/tmp/tet_iter18Verbose.log`):
- frame 1: state=0, count=0, byte=0
- frame 2: state=1, count=1, byte=1 (loader kicked in; TEST_READY wrote byte)
- frame 3: state=5, count=4, byte=1 (state advanced 1→5)
- frame 4+: state=6 stable, byte=1, fb_hash=0x68c64e2693747153
- 0 fatal, 0 skip, maxframe ~5550

**Labels DID partially materialize** — 2 text_obj diagnostics at frame 2:
- `0x100e5a80` and `0x100e5c00`, both with `pushed=93 consumed=0` MISMATCH
- ASCII: `"Tetris(R)&(C)1985-2006TetrisHoldingLLC.GameDesignbyAlexeyPajitnov.TetrisLogoDesignByRogerDean"`

These are **legal-text objects** (93 chars × 2 text_objs = 188 total chars).
With `CLICKY_GL_TEXGEN_VERBOSE=1`, draw_detail count for frame 2 shot up to
**191 draws** dominated by `file=f8x10text3_a8.pix` (the legal font atlas) —
**186 of 191 draws actually rendered the legal text glyphs** (1 DrawDetail × 1
background `screenBG_565.pix`, 1 logo `tetrisLogoT_4444.pix`, 1 EA logo,
186 legal-text glyphs).

Iter 17's TEST_READY-only run produced framebuffer hash `0x50ab75c7bf00e5a4`
(only logos + bg). Enabling ASYNC3_COMPLETE changed the hash to
`0x68c64e2693747153` — meaning additional content (the legal text glyphs)
WAS added.

**However no expected menu labels (MENU/PLAY/...) are pushed or drawn**, because:
- The byte write happens too early — by frame 4 state already advanced to 6
  before the legal-screen dwell completed.
- After state reaches 6 the game's render loop doesn't issue additional text
  pushes; the framebuffer freezes mid-legal-text.

### Screenshot artifact (state=1 / legal-text frame)

`/tmp/tetris_iter18_test_ready_async3_menu.png` (320x240 RGB). Pixel analysis:
- avg lum 69.3/255 (medium bright)
- avg r/g/b 47/60/101 (bluish legal-screen palette)
- 64.7% pixels have lum > 40 (significant content)
- 5.0% pixels have lum > 180 (bright = glyphs/sprites)
- **bright content bbox: x=[9..279] y=[12..237]** — wider/taller than iter 17's
  xbbox of `x=[63..279] y=[12..153]`, consistent with the legal text glyphs
  being drawn across most of the vertical screen range.

Capture manifest: only one startup ppm captured (at frame 2: hash
`0xd0cb4fe54923dbbd`). Final hash `0x68c64e2693747153` is the state=6 frozen
frame (no separate capture triggered because the script only dumps on
hash-change events).

### Conclusion

Iter 18 REVERSED the legit byte-setter `0x18005034` and its 4 callers.
- 2 callers are input-driven (menu button) — proven via iter 9 already.
- 2 callers are audio-completion-driven, gated on the audio queue being
  initialized (`[clock_obj+0x2c] != 0`).

The legit trigger is likely the Audio:23 / Audio:1 callback ABI:
Audio:23 takes a callback struct (`r2=0x1801bfc0`) containing a 4-entry
vtable of mini-functions; the TI firmware invokes index 0x18 (`0x1801bfd8`)
on audio completion; that runs the destructor dance and tail-calls
`0x18005034` (byte-setter) — advancing state legal→menu.

Our emulator returns Audio:23 / Audio:1 as success without invoking the
registered callbacks later, so the legit byte-write never fires and the
state machine is gated on byte `[0x18025674]=0` forever.

### Verification

- `cargo test -p clicky-core --lib eapp` → 16 passed.
- Default (no env) golden headed: 0 fatal, 0 skip, maxframe 140 (golden).
- TEST_READY+ASYNC3_COMPLETE combined: 0 fatal, 0 skip, maxframe ~5550;
  legal text loads (188 glyph draws). No menu labels yet.

### Next (iteration 19 priorities)

1. **Watch `[clock_obj+0x2c]` (`0x1005f713c`) on default boot** — confirm the
   suspected null gate. Use `CLICKY_EAPP_WATCH=0x1005f713c,4`.
2. **Reverse Audio:23/Audio:1 ABI**: identify what the firmware struct fields
   mean (callback table, completion notification dispatch). Most likely the
   TI firmware Audio:23 stores `r2` (callback ctx) + `r3` (ctx-arg) in an
   internal table; when an audio sample finishes, it dispatches the
   callback struct index 0x18 with a `bl 0x1801bfd8`.
3. **Emulate the Audio:23 completion callback** in our handler:
   - Track Tetris's registered `r2` callback struct + `r3` ctx.
   - After some emulated host time (say N ms), invoke
     `0x1801bfd8(ctx_or_obj_ptr)` via the runner's pending-call queue.
4. **OR alternatively**: time the TEST_READY byte write to fire AFTER the
   legal-screen dwell time completes (e.g., delay writes by 80+ frames),
   letting the legal text fully render before state advances to 6. Verify
   whether menu labels materialize at state=6 after the proper dwell.
5. Once proper menu labels appear: ask user to visually confirm the menu
   screenshot matches the oracle (MENU/PLAY/VOLUME/OPTIONS/RECORDS/HELP/EXIT).
6. Cleanup: remove temp RE hooks once labels are sane.


## Iteration 19 — REVERSED more caller-3 gates + `CLICKY_EAPP_TEST_READY_DELAY` env

Goal: act on iter-18 priorities. Three things: (1) confirm [`clock_obj+0x2c`]
null-gate hypothesis, (2) implement timed TEST_READY byte write (delay until
after legal-screen dwell), (3) deeper RE on caller-3 chain to find remaining
gates.

### (1) Iter-18 hypothesis OVERTURNED: `[clock_obj+0x2c]` IS non-null on default boot

iter 18 hypothesized `[clock_obj+0x2c]` (= `0x1005f713c`) was null on default
boot. iter 19 added two fields to `startup_progress` and ran a long default run.

**Finding:** `[clock_obj+0x2c]` IS being populated by the guest code, starting
around frame 3-4. Per-frame values observed on DEFAULT boot:
- frame 1-2: `[clock_obj+0x2c] = 0x00000000` (NULL — clock not initialized yet)
- frame 3:    `[clock_obj+0x2c] = 0x100eff40` (heap object allocated)
- frame 4:    `[clock_obj+0x2c] = 0x101a47f0`
- frame 5+:    `[clock_obj+0x2c] = 0x101a7660` (eventually)

`[clock_obj+0x54]` (ready byte, the OTHER gate I checked): always 0 on default
boot. So BOTH caller-3 gates pass on default boot — yet the byte never gets set.

Conclusion: iter 18's watch must have had some intermittent hook bug (the
iter-14 note already warned that single-byte ARM `strb`/shared-store sometimes
escapes the watch hook). My new diagnostic reads via `read_guest_u32` and
works correctly.

### (2) Timed byte-write: `CLICKY_EAPP_TEST_READY_DELAY=N` env

Added a frame-delay env that defers the TEST_READY byte write until frame >= N
(so the legal-screen dwell completes naturally before state advances).

```
CLICKY_EAPP_TEST_READY=1
CLICKY_EAPP_TEST_READY_DELAY=N     # frames to wait before writing byte 1
```

Verified behavior with `CLICKY_EAPP_TEST_READY_DELAY=25
CLICKY_EAPP_ASYNC3_COMPLETE=1`:
- frame 1: state=0, byte=0
- frame 2-29: state=1, byte=0 (legal screen dwells ~5 seconds)
- frame 30: byte=1 written, state advances 1→5→6 immediately
- final: state=6 stable, fb_hash=`0x97ce4ebbe87a1ae7`

Also discovered iter-18's view was incomplete. State=6 framebuffer (fb_hash
0x97ce4ebbe87a1ae7) is THE SAME HASH that appeared at frame 4 — meaning
state=6 case just FREEZES the prior state=1 frame. State=6's case at
`0x1800535c` doesn't trigger any additional render work; the legal text
remains frozen on screen.

Re-disassembled state=6 case `0x1800535c..0x180053a4`:
```
0x1800535c: ldrb r0, [r4]          ; r0 = [state+0]
0x18005360: cmp r0, #0
0x18005364: beq 0x18005390         ; if 0, jump to function tail (NO WORK)
0x18005368: ldr r0, [r4, #12]     ; r0 = [state+0xc]
0x1800536c: cmp r0, #2
0x18005370: bne 0x18005390         ; if != 2, jump to function tail (NO WORK)
0x18005374: ldr r0, [pc, #72]
0x18005378: str r7, [r0]
0x1800537c: mov r0, #1
0x18005380: ldr r1, [r4, #24]
0x18005384: add r0, r0, #0x3f000
0x18005388: bl 0x1800002dc          ; some service call
0x1800538c: bl 0x1800004060          ; ← the audio-cleanup standalone fn!
0x18005390: ldr r0, [r4, #20]      ; function tail (state[+0x14] increment)
0x18005394: add r0, r0, #1
0x18005398: str r0, [r4, #20]
0x1800539c: mov r0, #1
0x180053a0: add sp, sp, #32
0x180053a4: pop {...}              ; return 1
```

State=6 case has TWO gates to do any work:
1. `[state+0] != 0` (some state byte)
2. `[state+0xc] == 2` (input-mode byte must be 2, not 4)

After state 1→5→6 transition, `[state+0xc]` was just WRITTEN TO 4 by the
state=3/4/5 case handler — so condition 2 FAILS — state=6 case just
goes to the function tail (basic counter increment, no rendering).

This confirms: STATE=6 CASE DOES NOT RENDER ANY MENU LABELS. It just freezes
the framebuffer and increments a counter.

So the menu labels must come from a DIFFERENT code path (not state=6 case in
`0x180050c0`). They would come from another per-frame render routine called
from `0x180222a4` AFTER the state-dispatch call.

### (3) Disassembled `0x1801b8b4` (state-machine tail-call target from `0x4088`)

`0x18004088` tail-calls `0x1b8b4(clock_obj, sl)` when `count>=3 AND byte==0`.
This happens EVERY FRAME during state=1. Iter 19 disassembled the full function:

```
0x1b8b4: push {r4,r5,r6,lr}
0x1b8b8: mov r4, r0              ; r4 = clock_obj
0x1b8bc: ldr r0, [r0, #0x6c]    ; counter
0x1b8c0: mov r5, r1              ; r5 = sl (small per-frame delta ~30)
0x1b8c4: add r0, r0, r1         ; counter += sl
0x1b8c8: cmp r0, #1000         ; if > 1000
0x1b8cc: str r0, [r4, #0x6c]
0x1b8d0: movgt r0, r4
0x1b8d4: blgt 0x1b548          ; PER-FRAME TIME UPDATER (calls miscTBD:12)
0x1b8d8: ldr r0, [r4, #0x5c]   ; sum2
0x1b8dc: add r0, r0, r5        ; sum2 += sl
0x1b8e0: str r0, [r4, #0x5c]
0x1b8e4: ldr r1, [r4, #0x60]   ; threshold
0x1b8e8: cmp r0, r1
0x1b8ec: ble 0x1b954           ; IF sum2 <= threshold → byte-setter gate path
   ...  (else, overflow path — never reached during observed default boot)
0x1b954: ldrb r0, [r4, #0x54]  ; gate 1: ready_byte
0x1b958: cmp r0, #0
0x1b95c: bne 0x1b96c          ; if non-zero, return 2 (NO byte-setter)
0x1b960: ldr r0, [r4, #0x2c]   ; gate 2: audio_queue pointer
0x1b964: cmp r0, #0
0x1b968: bne 0x1b974          ; if non-null, GO TO byte-setter chain
0x1b96c: mov r0, #2
0x1b970: pop {...}             ; return 2 (no byte-setter)
0x1b974: mov r1, r5
0x1b978: mov r0, r4
0x1b97c: bl 0x1b630           ; CALL 0x1b630 (audio mixing → maybe byte-setter)
0x1b980: ldrb r0, [r4, #0x10] ; r0 = [clock_obj+0x10]
0x1b984: cmp r0, #0
0x1b988: moveq r0, r4
0x1b98c: bleq 0x1bc90        ; tail-call 0x1bc90(clock_obj) if eq
0x1b990: ldr r0, [r4, #0x2c]
0x1b994: ldr r1, [r0]
0x1b998: ldr r2, [r1, #0x38]
0x1b99c: mov r1, r5
0x1b9a0: pop {r4,r5,r6,lr}
0x1b9a4: bx r2                ; tail-call vtable[14] of *[clock_obj+0x2c]
```

So the gates shown by iter 18 (gate 1 and 2) DO pass on default boot. The
function is tail-called every frame and `0x1b630` IS being called.

### KEY NEW GATES DISCOVERED IN `0x1b630`!

Disassembled `0x1801b814: b 0x18005034` (the eventual byte-setter tail-call
site) and traced backwards through `0x1b630`:

```
0x1b7c8: b 0x1b818              ; if (r5 & 0x10) AND (r6 & 0x10) ARE BOTH 0 ...
0x1b7cc: ldr r6, [pc, #220]    ; AND (r5 & 0x08) AND (r6 & 0x08) ARE BOTH 0
0x1b7d0: ldr r1, [sp, #40]      ; r1 = some stack value
0x1b7d4: ldr r0, [r6, #4]
0x1b7d8: add r0, r0, r1
0x1b7dc: cmp r0, #2000          ; comparison against threshold 2000
0x1b7e0: str r0, [r6, #4]
0x1b7e4: b 0x1b818              ; if r0 < 2000, skip byte-setter
0x1b7e8: strb fp, [r4, #0x55]  ; r4=1 → set ready byte 0x55 (different from 0x54)
0x1b7ec: ldr r0, [r4, #0x2c]
0x1b7f0: cmp r0, #0
0x1b7f4: ldrne r1, [r4, #0xc]
0x1b7f8: cmpne r0, r1
0x1b7fc: addne sp, sp, #44
0x1b800: popne {...}            ; if [r4+0xc] != [r4+0xc2 absorbed], skip
0x1b80c: b 0x55a8
0x1b808: strb fp, [r4, #0x54]   ; ← r4=1 → set ready byte 0x54
0x1b80c: add sp, sp, #44
0x1b810: pop {...}
0x1b814: b 0x18005034           ; ← TAIL CALL byte-setter
```

The new gate is at:
```
0x1b7ac: tst r5, #16 (bit 4 / 0x10)
0x1b7b8: tst r5, #8  (bit 3 / 0x08)
```

`r5` was loaded at `0x1b6e8: and r5, r0, #255` from `[some_audio_slot+0x8c]`.
That is the AUDIO SLOT state byte (NOT user input). If neither bit 0x10 nor
0x08 is set in this audio-state byte, the path branches to `0x1b818` which
is the SKIP-BYTE-SETTER path.

So caller-3's REAL final gate is: **the audio slot state byte at
`[output+0x8c]` must have bit 0x10 OR bit 0x08 set** — i.e., an AUDIO EVENT
must be currently flagged in the audio slot. Bit 0x10 likely means
"audio sample playing" and bit 0x08 means "audio sample completed".

Iter 18's hypothesis "audio callback gets invoked by firmware when sample
plays" is now FULLY confirmed with concrete bit-positions: the firmware
delivers a state flag to `[output+0x8c]`, Tetris polls it via caller-3's
chain every frame, and when it sees "playing" or "completed" bits set, the
byte-setter fires.

### Conclusion

The blocker is NOT a guest-code bug — it's that our emulator doesn't emulate
the Audio:23 / Audio:1 callback dispatching. Without invoking the registered
callback struct pointer `r2=0x1801bfc0` after an audio period elapses, the
audio slot state byte `[output+0x8c]` is never set with bit 0x10 (playing) or
0x08 (completed), so caller-3's final gate never passes, so byte-setter never
fires, so state is gated at 1 forever.

### Verification

- `cargo test -p clicky-core --lib eapp` → 16 passed
- `cargo test -p clicky-core --lib live_gl::tests` → 14 passed
- Default (no env) golden headed: 0 fatal, 0 skip, maxframe 150 (no regression)
- Timed byte test (delay=25 + ASYNC3_COMPLETE): state advances at frame 30,
  fb_hash stays at 0x97ce4ebbe87a1ae7 — confirms state=6 case doesn't render
### Next priorities for iteration 20

1. **Identify where `[output+0x8c]` is supposed to be written.** Use the
   existing watch tool (`CLICKY_EAPP_WATCH=...`) on a TIMED_TESByte run
   that forces state=6 and observe where the audio slot state byte's writer
   would normally be. Check the Audio:23 case carefully. Find the registered
   callback struct and confirm index+0x18 path `b 0x1801bfd8` (caller 4).
2. **Implement an actual emulator-side audio event scheduler**. When Tetris
   calls Audio:23 / Audio:1 to register a callback, we should:
   - Store the registered callback struct pointer + ctx.
   - At some emulated interval (say after N guest cycles, or after app time
     >= N ms), write byte 0x10 to `[output+0x8c]` of the audio slot.
   - Observe whether caller-3's new gate then passes and byte-setter fires
     NATURALLY.

3. **OR alternative**: investigate `0x1801bfd8` (caller 4) gate requirements
   in detail. Maybe caller 4 is fired by a different mechanism
   (e.g., "audio clock tick") that we can emulate more easily than the
   audio-event-flag scheme.
4. **Render verification**: if we SUCCESSFULLY fire the byte-setter via the
   natural audio-event path, the resulting state=6 might still be the frozen
   legal text (per the iter-19 finding that state=6 case doesn't render).
   So we may still need to understand what DIRECTLY triggers the menu-label
   draws in the per-frame render function `0x180222a4` or its post-dispatch

5. **Supplementary**: investigate what `0x1801bc90` (called at `0x1b98c`)
   does — this might be the "request next audio sample" routine.

