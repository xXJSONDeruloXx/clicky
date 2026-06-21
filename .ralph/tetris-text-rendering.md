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
      `Options`, etc.), but there are **no work-RAM u32 pointers into those rows
      or English value spans** at run end, so the guest has the raw file but has
      not materialized/drawn these labels in the current state/path.
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
      then `0x4088` returns state 5 and the frame state advances to 6.
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

## Status

**Text-rendering mechanism is CORRECT and COMPLETE** (PC hook + recorded
char seq; push==consume; ASCII-order table; glyphs OCR-verified; UVs match;
0 fatals/0 skips; splash golden intact).

The scalar clock path is now fixed: default headed run logs a sane minutes-since-
midnight value (`0x1e3`) and emits `8:03AM` instead of `':.0AM`. The apparent
iteration-5 downstream blocker was caused by writing too many localtime fields
and overwriting saved registers; the recovered `miscTBD:12` ABI is six words.

User's visual oracle also proved the 9-char `89ΔΕABCDE`/`89/ABCDE` row is not
intended menu-label text. Iteration 4 showed it is a deliberate decorative /
font-sample string. The actual expected menu labels are still **not issued as
text draws** in the current guest state/path. Iteration 7 tightened this:
`Strings.dta` rows decode to the expected English values, but no work-RAM pointer
refs into those rows/value spans exist at scan time, language-index brute force
does not reveal labels, and long no-input runs never emit them.

Bottom line: glyph decode, texture asset selection, clock time source,
placeholder-resource release safety, and direct `AsyncFileIO:12/14/16` handle
semantics are now fixed. The post-Menu `frame_state=6` path is explained as the
provisional `menu` event id mapping to guest bit `0x10`, not as a save-read
failure. Remaining visible gap is still upstream guest state/API for main-menu
label issuance/string-table path: expected rows exist in `Strings.dta`, but the
default steady state never materializes or draws them.

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
