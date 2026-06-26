# iPod Game Compatibility Report — Clicky Emulator

**Date:** 2026-06-26 (updated)  
**Test Method:** Headless 5s + headed smoke tests, experimental GL HLE enabled  
**Build:** `clickwheel-games` branch, commit `c400405`

## Summary

| Category | Count | Games |
|----------|-------|-------|
| ✅ **Working** | 10 | Tetris, Cubis 2, Hold'em, Ms. Pac-Man, Pac-Man, Mahjong, Mini Golf, Bowling, Pool, Sudoku |
| ⚠️ **Partial (DMA bg visible)** | 2 | Bejeweled (98% content), Zuma (49% content) |
| ⚠️ **Partial (GL only)** | 2 | Solitaire (93% content), Vortex (title renders) |
| 🔴 **Not Working** | 2 | TWA/iQuiz (AsyncFileIO:7), Lost (div-by-zero) |

## Detailed Results (5s headless)

| Bundle | Game | Draws | Status | Notes |
|--------|------|-------|--------|-------|
| 66666 | Tetris | 4,140 | ✅ | Reference game |
| 99999 | Cubis 2 | 22,848 | ✅ | Highest draw count |
| 33333 | Hold'em | 19,196 | ✅ | Complex .ipd pipeline |
| 14004 | Ms. Pac-Man | 16,610 | ✅ | Procedural graphics |
| AAAAA | Pac-Man | 15,948 | ✅ | TGA textures |
| 77777 | Mahjong | 13,347 | ✅ | .rlb bundles |
| 88888 | Mini Golf | 7,083 | ✅ | Course data |
| 1500C | Sims Bowling | 1,519 | ✅ | Sims engine |
| 1500E | Sims Pool | 2,007 | ✅ | Sims engine |
| 50513 | Sudoku | 2 | ✅ | NDC engine, splash centered |
| 55555 | **Bejeweled** | 180 | **✅/DMA** | **98% content** — DMA bg at frame 6, gem sprites overlay |
| 44444 | **Zuma** | 42 | **✅/DMA** | **49% content** — DMA bg at frame 7, no more crash |
| 50514 | Solitaire | 118 | ⚠️ | ~93% content, 1 UV skip/frame |
| 12345 | Vortex | 59 | ⚠️ | Title graphic, handle 0x21 VBO fail |
| 11002 | TWA/iQuiz | 0 | 🔴 | AsyncFileIO:7 not implemented |
| 1B200 | Lost | 0 | 🔴 | ARM divide-by-zero crash |

## PopCap Engine DMA Background (New!)

Bejeweled and Zuma use a hybrid rendering pipeline:
1. **Software rasterizer** writes RGB565 pixels into a DMA framebuffer at `0x1402_0000`
2. **OpenGL ES** overlays gem sprites / ball paths on top
3. The game writes the entire 320×240 background **exactly once** (~100K cycles)
4. No GL ordinal-157/158 calls during DMA phase — background is off-screen

### DMA Rendering Pipeline
```
Game init → GL loading screens (4-6 frames)
         → Software rasterize background → 0x1402_0000 (153,600 bytes)
         → maybe_present_dma_frame() detects dirty buffer
         → overlay_dma_rgb565() composites: DMA bg + GL sprites
         → Present to window
```

### Key Implementation Details
- **Dirty flag** (`hw_dma_dirty`): set on any DMA write, checked every 10K steps
- **has_dma_overlay**: causes `complete_frame()` to use framebuffer even with 0 GL draws
- **Alpha blending**: DMA RGB565 as base, GL RGBA8 composited on top with alpha
- **Auto-vflip**: DMA frames use same vflip logic as pixel-coord engines (inverted)
- **Guard**: only injects DMA frames when no GL frame is active (`!frame_active`)

## Fixes Applied This Session

### 1. DMA Framebuffer Storage + HW Stub (already committed)
- 153KB RGB565 DMA buffer at `0x1402_0000`
- HW reads return DMA-complete status (=1) to prevent spin-waits
- All access sizes (r8/r16/r32/w8/w16/w32) handle DMA range

### 2. DMA Injection + Overlay (this commit)
- `maybe_present_dma_frame()`: detects dirty DMA buffer and injects frame
- `has_dma_overlay` flag: allows 0-draw frames to use DMA content
- `overlay_dma_rgb565()`: alpha-blend DMA background under GL sprites
- Throttling: every 10K steps, only when no GL frame active

## Engine Classification

| Engine | Games | Background | vflip |
|--------|-------|------------|-------|
| Tetris Runtime | 9 games | GL texture | Yes |
| Hold'em Runtime | Texas Hold'em | GL texture | Yes |
| Sudoku/SS Engine | Sudoku, Solitaire | GL texture | No (NDC) |
| **PopCap Engine** | **Zuma, Bejeweled** | **DMA buffer** | Yes |
| iQuiz Engine | TWA | .ipd via AsyncFileIO:7 | Yes |
| Lost Engine | Lost | rserver.bin | Yes |

## Remaining Issues

### Zuma 49% coverage (vs Bejeweled 98%)
Zuma's DMA buffer writes may be partially overlapping, or the game renders
only part of the screen via DMA (center game board, with borders/UI via GL).
Needs investigation.

### Solitaire UV Mismatch (minor)
One draw per session fails: `no live upload matched triangle-strip UV span Some((24, 40))`.

### Vortex VBO Path (ordinal-175/125)
Handle 0x21: `position array unusable`. VBO pointer-to-struct dereferencing needed.

### TWA/iQuiz (AsyncFileIO:7)
Needs directory enumeration for .ipd trivia packs.

### Lost (div-by-zero)
ARM exception handling needed for UDIV instruction.

## Environment

```bash
CLICKY_EXPERIMENTAL_GL_HLE=1
CLICKY_GL_GATE_B=1
CLICKY_GL_LIVE_CONTINUOUS=1
CLICKY_GL_PRESENT_VFLIP=1
```
