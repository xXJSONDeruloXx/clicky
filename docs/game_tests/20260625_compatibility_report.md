# iPod Game Compatibility Report — Clicky Emulator

**Date:** 2026-06-26 (updated)  
**Test Method:** Headless 8s runs + headed 3s smoke tests, experimental GL HLE enabled  
**Build:** `clickwheel-games` branch, commit `a74079c`

## Summary

| Category | Count | Games |
|----------|-------|-------|
| ✅ **Working** | 10 | Tetris, Cubis 2, Texas Hold'em, Ms. Pac-Man, Pac-Man, Mahjong, Mini Golf, Sims Bowling, Sims Pool, **Sudoku** |
| ⚠️ **Partial** | 3 | Vortex, Solitaire, Bejeweled |
| 🔴 **Not Working** | 3 | Zuma, TWA/iQuiz, Lost |

## Detailed Results (8s headless)

| Bundle | Game | Draws | Presented | Discarded | Skipped | Status | Notes |
|--------|------|-------|-----------|-----------|---------|--------|-------|
| 66666 | Tetris | 10,651 | 120 | 0 | 120 | ✅ | Reference game |
| 99999 | Cubis 2 | 44,208 | 120 | 0 | 120 | ✅ | Highest draw count |
| 33333 | Hold'em | 33,340 | 120 | 0 | 120 | ✅ | Complex .ipd pipeline |
| 14004 | Ms. Pac-Man | 26,540 | 120 | 0 | 121 | ✅ | Procedural graphics |
| AAAAA | Pac-Man | 26,299 | 120 | 0 | 123 | ✅ | TGA textures |
| 77777 | Mahjong | 21,643 | 120 | 0 | 121 | ✅ | .rlb bundles |
| 88888 | Mini Golf | 11,379 | 120 | 0 | 122 | ✅ | Course data |
| 1500C | Sims Bowling | 2,533 | 120 | 0 | 120 | ✅ | Sims engine |
| 1500E | Sims Pool | 3,360 | 120 | 0 | 120 | ✅ | Sims engine |
| 50513 | **Sudoku** | **2** | **120** | **0** | 120 | ✅ | NDC engine — splash renders, waiting for input |
| 50514 | Solitaire | 340 | 120 | 0 | 125 | ⚠️ | Partial — some textures missing |
| 55555 | Bejeweled | 180 | 6 | 0 | 7 | ⚠️ | DMA stall after loading screen |
| 12345 | Vortex | 99 | 75 | 0 | 150 | ⚠️ | Limited texture uploads |
| 44444 | Zuma | 42 | 7 | 0 | 8 | 🔴 | DMA stall, PopCap engine |
| 11002 | TWA/iQuiz | 0 | 120 | 0 | — | 🔴 | AsyncFileIO:7 not implemented |
| 1B200 | Lost | 0 | 120 | 0 | — | 🔴 | ARM divide-by-zero crash |

## Fixes Applied This Session

### 1. HW Register Stub (`0x14000000..0x17FFFFFF`)
Read-zero, write-discard for 64MiB hardware register region. Prevents `FatalMemException` in Zuma and Bejeweled.

### 2. `Filesytem` Import Handler
Returns 1 for ordinal 0 (filesystem init). Used by TWA/iQuiz. Doesn't fully fix TWA (needs AsyncFileIO:7).

### 3. Auto-begin on Present + 0-draw Frame Preservation
Sudoku/Solitaire engine never calls ordinal-158 (begin frame). Its per-frame loop is 159→149→157 (bind→setup→present). The fix:
- Auto-begin a frame when ordinal 157 (present) arrives with no active frame
- When `complete_frame` returns 0 draws, preserve the previous framebuffer instead of overwriting with black
- Set `should_present = true` for all completed frames

### 4. NDC-to-Pixel Position Scaling
Sudoku engine passes vertex positions in normalized device coordinates (0–1 range) while the Tetris engine uses pixel-space (0–320, 0–240). The rasterizer expected pixel coordinates and only wrote 1 pixel of coverage.
- **Detection:** `max_coord < 2.0` triggers NDC mode
- **Mapping:** Viewport-style transform — scale `(min_x..max_x, min_y..max_y)` to `(0..FB_WIDTH, 0..FB_HEIGHT)`
- Applied in both `rasterize_draw()` and `rasterize_triangle_strip_record()`

### 5. Auto-Vflip Suppression for NDC Frames
Pixel-coord engines (Tetris) render bottom-to-top and need `present_vflip=1`. NDC engines (Sudoku/Solitaire) render top-to-bottom and must NOT be flipped. The `ndc_frame` flag per-frame disables vflip automatically.

## Engine Classification

| Engine | Games | Coordinate System | Vflip | Frame Begin | Asset Format |
|--------|-------|--------------------|-------|-------------|-------------|
| **Tetris Runtime** | Tetris, Cubis 2, Mini Golf, Mahjong, Ms. Pac-Man, Pac-Man, Sims Bowling, Sims Pool | Pixel (0–320) | Yes | ordinal-158 | .pix via AsyncFileIO:3 |
| **Hold'em Runtime** | Texas Hold'em | Pixel (0–320) | Yes | ordinal-158 | .ipd/.blob via AsyncFileIO:3 |
| **Sudoku/SS Engine** | Sudoku, (Solitaire) | **NDC (0–1)** | **No** | **None** | Minimal |
| **PopCap Engine** | Zuma, Bejeweled | Pixel | Yes | ordinal-158 | DMA + .ipd |
| **iQuiz Engine** | TWA | Pixel | Yes | ordinal-158 | .ipd via AsyncFileIO:7 |
| **Lost Engine** | Lost | Pixel | Yes | ordinal-158 | rserver.bin |

## Environment

```bash
CLICKY_EXPERIMENTAL_GL_HLE=1
CLICKY_GL_GATE_B=1
CLICKY_GL_LIVE_CONTINUOUS=1
CLICKY_GL_PRESENT_VFLIP=1    # auto-suppressed for NDC frames
```

## Test Commands

```bash
# Headless 8s test
CLICKY_EXPERIMENTAL_GL_HLE=1 CLICKY_GL_GATE_B=1 CLICKY_GL_LIVE_CONTINUOUS=1 \
CLICKY_GL_PRESENT_VFLIP=1 RUST_LOG=EAPP_GL=info,EAPP_IMPORT=warn \
timeout 8 ./target/release/eapp <bundle_dir> --headless

# Headed run
./scripts/tetris.sh          # or cubis2.sh, holdem.sh, etc.
```
