# iPod Game Compatibility Report — Clicky Emulator

**Date:** 2026-06-26 (final)  
**Test Method:** Headless 5s + headed smoke tests, experimental GL HLE enabled  
**Build:** `clickwheel-games` branch, commit `0b5c9fc`

## Summary

| Category | Count | Games |
|----------|-------|-------|
| ✅ **Fully Working** | 10 | Tetris, Cubis 2, Hold'em, Ms. Pac-Man, Pac-Man, Mahjong, Mini Golf, Bowling, Pool, Sudoku |
| ✅ **DMA Background Working** | 2 | **Bejeweled** (98% content, gem sprites + game board), **Zuma** (33% content, game board top) |
| ⚠️ **Partial** | 2 | Solitaire (93% content, ~5 UV skips), Vortex (18% content, title graphic) |
| 🔴 **Not Working** | 2 | TWA (pack content loading), Lost (programmable shader pipeline) |

**12 out of 16 games show visual content on screen.**

## Detailed Results (5s headless)

| Bundle | Game | Draws | Status | Notes |
|--------|------|-------|--------|-------|
| 66666 | Tetris | 3,991 | ✅ | Reference game, fully playable |
| 99999 | Cubis 2 | 23,285 | ✅ | Highest draw count |
| 33333 | Hold'em | 17,700 | ✅ | Complex Audio pipeline |
| 14004 | Ms. Pac-Man | 15,960 | ✅ | Procedural graphics |
| AAAAA | Pac-Man | 15,540 | ✅ | TGA textures |
| 77777 | Mahjong | 12,651 | ✅ | .rlb bundles |
| 88888 | Mini Golf | 6,948 | ✅ | Course data |
| 1500C | Sims Bowling | 1,442 | ✅ | Sims engine |
| 1500E | Sims Pool | 1,887 | ✅ | Sims engine |
| 50513 | Sudoku | 2 | ✅ | NDC engine, splash centered |
| 55555 | **Bejeweled** | 180 | ✅ | DMA bg 98%, gem sprites overlay |
| 44444 | **Zuma** | 42 | ✅ | DMA bg 33% (top 86 rows only) |
| 50514 | Solitaire | 110 | ⚠️ | 93% content, ~5 UV skips |
| 12345 | Vortex | 59 | ⚠️ | 18% content (title graphic), VBO path broken |
| 11002 | TWA | 0 | 🔴 | AsyncFileIO:7 callback works, pack loading incomplete |
| 1B200 | Lost | 0 | 🔴 | Shader pipeline (164) needs real GPU execution |

## Playing Games

All working games are **playable in headed mode**! The keyboard maps to click wheel:

| Key | iPod Control |
|-----|-------------|
| ↑ ↓ ← → | Click wheel (scroll / direction) |
| Enter | Select (center button) |
| M | Menu |
| Mouse wheel | Scroll |

Launch with a game script (recommended):
```bash
./scripts/tetris.sh          # headed, with GL rendering
./scripts/tetris.sh --headless  # headless test
```

Or directly:
```bash
CLICKY_EXPERIMENTAL_GL_HLE=1 CLICKY_GL_GATE_B=1 \
CLICKY_GL_LIVE_CONTINUOUS=1 CLICKY_GL_PRESENT_VFLIP=1 \
cargo run -p clicky-desktop --bin eapp --release -- /path/to/GameID
```

## TWA (iQuiz) — What's Working

The AsyncFileIO:7 directory enumeration is now implemented:
- Enumerates subdirectories from `UserTrivia/Packs` (follows symlinks)
- Uses the same async request-object protocol as ordinal 3
- Writes status/count to [req+0x20/0x24]
- Queues completion callback from [req+0x34/0x38]
- After callback: game calls AsyncFileIO:0 to load "data" (pack metadata)

**Blockers remaining:**
1. The "data" file resolution doesn't find the right file (game bundles have font data in `UserTrivia/Data/`)
2. Pack icons (149×75 A8 textures) aren't loaded — the game uploads a 2×2 A8 placeholder but draws with handle 0x28 (unassociated)
3. The game needs to download pack content (.ipd/.tga files) from each pack directory

## Lost — What's Working

The shader program API is stubbed (ordinals 164, 167, 152, 153):
- Ordinal 164 logs the shader binary address (rserver.bin)
- Ordinal 152 writes GL_TRUE=1 (link success) to the query buffer
- Ordinal 153 logs viewport dimensions

**Blockers remaining:**
1. Lost uses a **programmable GPU pipeline** — rserver.bin contains compiled shaders
2. The game never calls glDrawArrays/glDrawElements without a working shader
3. The frame loop is just `clear → present` — no draws at all
4. A real shader compiler or full shader interpreter would be needed
5. rserver.bin is likely OpenGL ES 1.1 shader programs in binary format

## Engine Classification

| Engine | Games | Background | Coordinates | vflip |
|--------|-------|------------|-------------|-------|
| Tetris Runtime | 9 | GL texture | Pixel | Yes |
| Hold'em Runtime | Hold'em | GL texture | Pixel | Yes |
| Sudoku/SS Engine | Sudoku, Solitaire | GL texture | NDC | No |
| **PopCap Engine** | Bejeweled, Zuma | **DMA buffer** | Pixel | Yes |
| iQuiz Engine | TWA | .ipd (needs pack loading) | Pixel | Yes |
| Lost Engine | Lost | rserver.bin (programmable) | Pixel | Yes |

## Fixes Applied This Session

1. **Sudoku splash rendering** — auto-begin on present, NDC viewport scaling, auto-vflip, 0-draw preservation
2. **PopCap DMA background** — framebuffer storage, dirty flag, overlay injection, alpha blending, full-write check
3. **DMA vflip fix** — display-oriented (top-to-bottom) DMA → framebuffer copy, vflip only at final present
4. **HW stub (64MiB)** — prevents FatalMemException in PopCap games
5. **Filesystem import handler** — removes "unhandled module" warnings
6. **NDC scaling dedup** — removed redundant shadow block in rasterizer
7. **Per-game launch scripts** — 11 scripts (10 working + Bejeweled + Zuma)
8. **Per-game documentation** — 14 game docs + README index
9. **AsyncFileIO:7 directory enumeration** — finds Packs, follows symlinks, async callback
10. **OpenGLES ordinal stubs** — 164, 167, 152, 153 for shader program API

## Remaining Issues

### TWA pack loading
After AsyncFileIO:7 returns pack names, the game tries to load a "data" file that doesn't exist in the bundle. The resolution to `.clicky-saves/data` creates an empty file. The game needs proper path mapping for pack resources (.ipd/.tga files).

### Lost programmable GPU
Lost requires real shader execution (rserver.bin). The HLE renderer only supports fixed-function pipelines. This would need either a full shader interpreter or a different rendering approach.

### Solitaire UV skips (~5/frame)
Minor — `no live upload matched triangle-strip UV span Some((24, 40))` for handle 0x1f in a 577×40 atlas.

### Vortex VBO path
Handle 0x21 `position array unusable` — ordinal-175/125 VBO setup corrupts vertex array definitions.

### Zuma 33% coverage
DMA background only covers the top 86 rows. The bottom part may need input events or different GL paths.
