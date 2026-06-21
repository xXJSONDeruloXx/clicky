# Eapp Matrix Hardening

Document the 2026-06-20 headed matrix and iteratively harden the general clickwheel eapp runtime using cross-game evidence.

## Goals
- Preserve a clear, cited matrix of current per-game status, artifacts, logs, visual observations, performance, and blockers.
- Prefer general ABI/renderer/runtime fixes over title-specific hacks.
- After each fix, rerun targeted headed/headless smoke tests and record evidence.
- Keep Tetris as the golden regression while improving sibling games.

## Checklist
- [x] Add headed matrix documentation with artifact root and per-game log/capture citations.
- [x] Implement/decode `src_fmt=0x190a pix_type=0x1401` upload path and validate against Cubis 2 / LOST.
- [x] Investigate and fix shared Bejeweled/Zuma unmapped write at `0x1080000c`.
- [x] Implement `OpenGLES:37 mode=5` or accurately identify/guard it for Texas Hold'em.
- [ ] Improve UV/upload matching for Mahjong / PAC-MAN / Ms. PAC-MAN / Mini Golf / Tetris.
  - [x] Capture and document Mahjong ordinal-45 descriptor/upload evidence.
  - [x] Decode Mahjong ordinal-45 descriptor layout enough to build guarded real resource uploads and recover pre-bind UV arrays without hardcoded texture/zero-UV fallbacks.
  - [x] Fix ordinal-37 `mode=5` (Texas Hold'em) UV decode: vertex arrays persist across material binds, so use the epoch-agnostic UV decode (matches ordinal-38 path and GL semantics). `no live upload matched` count dropped `164 -> 1`; rasterized strips `-> 165`. Tetris golden regression unaffected.
- [ ] Investigate lifecycle/timer/settings/resource blockers for Sims Bowling, Sims Pool, Sudoku, Royal Solitaire, iQuiz, Vortex.
  - [x] Classify no-capture titles from matrix logs.
  - [x] Implement/identify ordinal `38` draw path for Sims/Sudoku/Solitaire family as `DrawElements(GL_TRIANGLE_STRIP, count, GL_UNSIGNED_SHORT, indices)`.
  - [x] Identify ordinal `149` draw-adjacent state semantics; classified as a safe no-op per-draw bind of a fixed runtime state object (constant args `r0=0 r1=1 r2=0 r3=<BSS ptr>`, between material bind `159` and `DrawElements` `38`). Sims runs 0 fatals without it; full decode is low-priority.
  - [x] Investigate iQuiz/Vortex early fatal object-layout writes; root-caused both to a null destination buffer/object pointer (Vortex struct-fill reads `[object+4]`=null at `pc=0x18014d58`; iQuiz memcpy dest null at `pc=0x18001b08`). Fix needs targeted per-game RE of the specific HLE call that should populate the buffer (Vortex: likely GL surface bind; iQuiz: likely a `Metadata` object provider). Added general register-dump-on-fault diagnostics. Not patched (no guessing pointer-backed buffers).
- [ ] Rerun a full matrix and update docs.

## Verification
- Matrix artifact root: `/tmp/clicky_headed_matrix_unique_20260620_201555`
- Contact sheet: `/tmp/clicky_headed_matrix_unique_20260620_201555/contact_sheet.png`
- Build before matrix: `cargo build -p clicky-desktop --bin eapp` passed with existing warnings.
- Added full matrix docs to `docs/IPOD_GAMES_BRINGUP.md` with per-game logs/captures, performance estimates, and ranked blockers.
- Implemented `TextureFormat::LuminanceAlpha88` for GL ES `GL_LUMINANCE_ALPHA` (`0x190a`) + `GL_UNSIGNED_BYTE` (`0x1401`).
- `cargo test -p clicky-core --test eapp_gl_decode` passed.
- Targeted headed validation root: `/tmp/clicky_la_validate_20260620_202557`.
- Cubis 2 after fix: unsupported `0x190a/0x1401` count 0, skipped draws down from 18 to 2; latest screenshot `/tmp/clicky_la_validate_20260620_202557/99999_latest.png`.
- LOST after fix: unsupported `0x190a/0x1401` count 0 and early uploads decode as `Some(LuminanceAlpha88)`; still no completed GL frames in 7s.
- PopCap crash root cause: work RAM aperture too small. 8 MiB crashed at `0x1080000c`; 32 MiB moved Zuma to `0x1200000c`; 64 MiB survives 7s headed windows for Bejeweled and Zuma.
- 64 MiB validation root: `/tmp/clicky_ram64_validate_20260620_203011` with logs/screens for `55555`, `44444`, and `66666` Tetris regression.
- Implemented `OpenGLES:37 mode=5` as standard GL ES triangle strip. Validation root: `/tmp/clicky_mode5_validate_20260620_203634`; unsupported mode=5 count is now 0 for Texas Hold'em, remaining blocker is triangle-strip UV/upload matching for handle `0x23`.
- Mahjong UV/upload investigation: trace root `/tmp/clicky_mahjong_trace_20260620_203851`; JSON capture `/tmp/clicky_mahjong_capture2_20260620_204015/mahjong_gl.json`. Evidence suggested alternate `OpenGLES:45` descriptor/resource upload semantics; documented in `docs/IPOD_GAMES_BRINGUP.md`. Do not hardcode zero-UV fallbacks before decoding descriptor layout.
- Implemented guarded Mahjong-style ordinal-45 resource uploads. Decoded layout: `r1+0x04` points to texture object; object word 2 is material handle (`0x19`/`0x12`); word 4 is packed dimensions (`height << 16 | width`); word 9 is pixel pointer; word 10 is format-ish token (`0x8808`/`0x0801`, currently A8). Relaxed ordinal-37 UV material-epoch guard only for handles backed by ordinal-45 resource uploads, because Mahjong defines arrays before `159` material bind. Headless validation root `/tmp/clicky_ord45_validate_20260620_210929`: Mahjong logged 2 ordinal-45 uploads, 33,095 rasterized draws, 1 skip, 0 fatals; Tetris short regression logged 0 ordinal-45 uploads, 3,253 rasterized draws, 0 fatals. Headed validation root `/tmp/clicky_ord45_headed_20260620_211224`: Mahjong produced 33 PPM captures, 2 ordinal-45 uploads, 1,664 rasterized draw lines, 1 skip, 132 frame diagnostics, 0 fatals.
- Lifecycle/no-capture classification added to `docs/IPOD_GAMES_BRINGUP.md`: Sims Bowling/Pool, Sudoku, and Royal Solitaire use uploads/arrays followed by `159(...),149,38` rather than ordinal-37 draws; LOST now idles after LA fix with `159(h0xe)`; iQuiz/Vortex remain early fatal object-layout writes.
- Implemented ordinal `38` indexed draw-elements path. Evidence root `/tmp/clicky_ord38_capture2_20260620_204826`: `r0=0x5`, `r1=count`, `r2=0x1403`, `r3=index_ptr`. Headless validation root `/tmp/clicky_draw_elements_validate2_20260620_205657`: Sims Bowling logged 950 indexed draw rasterizations and 132 frame diagnostics with 0 fatals; Sudoku logged 1 indexed draw and 2 frame diagnostics; Tetris regression had 0 fatals. Headed validation root `/tmp/clicky_draw_elements_headed_20260620_205755`: Sims Bowling 5s headed run logged 234 indexed draw rasterizations and 9 capture files; Tetris 5s headed regression logged 20 capture files and 0 fatals.
- Added general register-dump-on-fault diagnostics (`fault regs pc=... fault_addr=... kind=... r0..r12 sp lr`) to the eapp memory-fault path; helps any future fatal, not just iQuiz/Vortex.
- iQuiz/Vortex fatal investigation: Vortex logs `/tmp/clicky_vortex_invest_20260620_211533/vortex{,_debug}.log`; iQuiz log `/tmp/clicky_iquiz_invest_20260620_211533/iquiz_debug.log`. Disassembly confirms Vortex faults in a struct-fill (`0x14d38`: `ldr r0,[r0,#4]` then `stmia r0!`, with `mov fp,#65536` field) and iQuiz faults in a 32-byte memcpy loop (`0x1af4`). Both dereference a null destination buffer. Register dumps: Vortex `r0=0x8 lr=0x00010000`, iQuiz `r0=0x10 lr=0x0`. Fix needs per-game RE of which HLE call populates the buffer (Vortex: likely GL surface bind; iQuiz: likely `Metadata` object provider); not patched to avoid guessing.
- Ordinal `149` classification: evidence `/tmp/clicky_ord149_invest_20260620_211533/sims.log` (2941 calls, constant args `r0=0 r1=1 r2=0 r3=0x1807cbc8`, between `159` and `38`). `r3` is beyond the file-backed image (BSS/runtime token) and the guest never derefs it, so the no-op is provably safe; full decode low-priority.
- Ordinal-37 `mode=5` UV epoch fix (Texas Hold'em): evidence `/tmp/clicky_uv_evidence_20260620_213000/holdem.log` (binds of GL texture-name handles `0x6`/`0x23` after array defs caused strict material-epoch UV guard to reject valid UVs). Fix uses `live_decode_uvs_range_any_epoch` in `live_handle_triangle_strip_draw`, matching the ordinal-38 path and GL semantics (vertex arrays persist across texture binds). Validation root `/tmp/clicky_uv_fix_20260620_213500`: Texas Hold'em `no live upload matched` `164 -> 1`, rasterized strips `-> 165`; Tetris regression 0 fatals/1005 rasterized/2 skipped; Sims Bowling 0 fatals/763 rasterized/0 skipped; 15/15 tests pass. Next accuracy step: associate ordinal-99 uploads with their GL texture names (need the `glBindTexture` ordinal).

## Notes
- Matrix used headed 7s runs via `scripts/tetris.sh --no-build --timeout 7 --bundle <bundle>` with live GL enabled and captures/logs rooted per-game.
- Obvious high-value fixes, in order: 0x190a/0x1401 texture decode, 0x1080000c PopCap write, mode=5, UV/upload matching, lifecycle/idling titles.
