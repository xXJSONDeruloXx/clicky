# iPod Game Compatibility Report — Clicky Emulator

**Date:** 2026-06-26 (updated)  
**Test Method:** Headless 5s + headed smoke tests, experimental GL HLE enabled  
**Build:** `clickwheel-games` branch, commit `eea5af2`

## Summary

| Category | Count | Games |
|----------|-------|-------|
| ✅ **Working** | 10 | Tetris, Cubis 2, Texas Hold'em, Ms. Pac-Man, Pac-Man, Mahjong, Mini Golf, Sims Bowling, Sims Pool, Sudoku |
| ⚠️ **Partial** | 2 | Solitaire (93% content, gem sprites visible), Bejeweled (gem sprites on black bg) |
| ⚠️ **Vortex** | 1 | Vortex (title graphic renders, VBO path missing) |
| 🔴 **Not Working** | 3 | Zuma (DMA stall), TWA/iQuiz (AsyncFileIO:7), Lost (div-by-zero) |

## Detailed Results (5s headless)

| Bundle | Game | Draws | Status | Notes |
|--------|------|-------|--------|-------|
| 66666 | Tetris | 4,116 | ✅ | Reference game |
| 99999 | Cubis 2 | 23,194 | ✅ | Highest draw count |
| 33333 | Hold'em | 19,186 | ✅ | Complex .ipd pipeline |
| 14004 | Ms. Pac-Man | 16,591 | ✅ | Procedural graphics |
| AAAAA | Pac-Man | 16,008 | ✅ | TGA textures |
| 77777 | Mahjong | 13,295 | ✅ | .rlb bundles |
| 88888 | Mini Golf | 7,137 | ✅ | Course data |
| 1500C | Sims Bowling | 1,521 | ✅ | Sims engine |
| 1500E | Sims Pool | 1,998 | ✅ | Sims engine |
| 50513 | Sudoku | 2 | ✅ | NDC engine, splash renders + centered |
| 50514 | Solitaire | 115 | ✅/close | 93% framebuffer, ~1 skip/frame |
| 12345 | Vortex | 59 | ⚠️ | Title graphic renders, handle 0x21 VBO fail |
| 55555 | Bejeweled | 180 | ⚠️ | Gem sprites visible (headed), DMA bg missing |
| 44444 | Zuma | 42 | 🔴 | DMA stall, "Abnormal termination" |
| 11002 | TWA/iQuiz | 0 | 🔴 | AsyncFileIO:7 not implemented |
| 1B200 | Lost | 0 | 🔴 | ARM divide-by-zero crash |

## Fixes Applied This Session (2026-06-26)

### 1. Auto-begin + 0-draw Frame Preservation (Sudoku fix)
Sudoku/Solitaire engine never calls ordinal-158 (begin frame). Per-frame loop: 159→149→157.
- Auto-begin frame on present when no active frame
- Preserve previous framebuffer on 0-draw idle frames

### 2. NDC-to-Pixel Viewport Scaling
Sudoku engine passes vertex positions in 0–1 NDC range instead of pixel coords.
- Detection: `max_coord < 2.0` triggers NDC mode
- Mapping: `(min_x..max_x, min_y..max_y)` → `(0..FB_WIDTH, 0..FB_HEIGHT)`
- Initial `x * 320` overshot (1.2 → 384px), centering was 30px off. Viewport mapping fixed it.

### 3. Auto-Vflip Suppression for NDC Frames
- Pixel-coord engines (Tetris) render bottom-to-top → need vflip
- NDC engines (Sudoku/Solitaire) render top-to-bottom → need no vflip
- Added `ndc_frame: bool` flag per-frame, suppresses vflip in `complete_frame()` and `present()`

### 4. DMA Framebuffer Storage + Overlay Path (PopCap engine)
- Added 153KB RGB565 DMA framebuffer in `EappBus` at `0x1402_0000`
- HW stub writes (r8/r16/r32/w8/w16/w32) store DMA pixel data
- HW stub reads return DMA-complete status (=1) to prevent spin-waits
- Added `overlay_dma_rgb565()` to `LiveGlState` with alpha blend compositing
- Wired overlay into `live_handle_candidate_present` before `complete_frame`
- **Result**: Bejeweled gem sprites visible on black background in headed mode
- **Remaining**: DMA background not visible — PopCap writes it after GL frames, and exits when DMA interrupt never fires

## Engine Classification

| Engine | Games | Coordinate System | Vflip | Frame Begin | Background |
|--------|-------|--------------------|-------|-------------|-----------|
| **Tetris Runtime** | 9 games | Pixel (0–320) | Yes | ordinal-158 | GL texture |
| **Hold'em Runtime** | Texas Hold'em | Pixel | Yes | ordinal-158 | GL texture |
| **Sudoku/SS Engine** | Sudoku, Solitaire | **NDC (0–1)** | **No** | **None** | GL texture |
| **PopCap Engine** | Zuma, Bejeweled | Pixel | Yes | ordinal-158 | **DMA buffer** |
| **iQuiz Engine** | TWA | Pixel | Yes | ordinal-158 | .ipd via AsyncFileIO:7 |
| **Lost Engine** | Lost | Pixel | Yes | ordinal-158 | rserver.bin |

## Remaining Issues

### Solitaire UV Mismatch (minor)
One draw per session fails: `no live upload matched triangle-strip UV span Some((24, 40))`.
The game is rendering a 24×40 sub-rect of a 577×40 atlas. `select_smallest_containing_upload`
should match it but sometimes doesn't. Negligible impact — 93% content coverage.

### Vortex VBO Path (ordinal-175/125)
Handle 0x21 always fails: `position array unusable`. The engine sets up VBO-style
indirection via ordinal-175/125, which corrupts the next ordinal-137 array definition
(`comps=268602880` = `0x10040000` = a pointer value, not a component count).
Need to implement VBO pointer-to-struct dereferencing for ordinal-137.

### PopCap DMA Background (Bejeweled/Zuma)
PopCap renders its background via software rasterization into a DMA buffer
at `0x1402_0000`, then expects DMA hardware to transfer it to the display.
The DMA writes happen *after* GL frame presents, and the game watchdog-exits
("Abnormal termination") when the DMA interrupt never fires.
- **Partial workaround**: DMA reads return "complete" (=1) to prevent register spin-waits
- **Missing**: DMA interrupt delivery (game callbacks), and the DMA overlay isn't
  visible because data arrives between GL frames

## Environment

```bash
CLICKY_EXPERIMENTAL_GL_HLE=1
CLICKY_GL_GATE_B=1
CLICKY_GL_LIVE_CONTINUOUS=1
CLICKY_GL_PRESENT_VFLIP=1    # auto-suppressed for NDC frames
```
