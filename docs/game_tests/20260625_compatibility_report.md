# iPod Game Compatibility Report - Clicky Emulator

**Date:** 2026-06-25  
**Test Method:** Headless 8-10s runs with experimental GL HLE enabled

## Current Status

| Bundle | Game | Status | Draws | Root Cause |
|--------|------|--------|-------|------------|
| 66666 | Tetris | ✅ WORKS | 10,840 | — |
| 99999 | Cubis 2 | ✅ WORKS | 42,990 | — |
| 33333 | Texas Hold'em | ✅ WORKS | 34,088 | — |
| 14004 | Ms. Pac-Man | ✅ WORKS | 26,405 | — |
| AAAAA | Pac-Man | ✅ WORKS | 24,511 | — |
| 77777 | Mahjong | ✅ WORKS | 21,235 | — |
| 88888 | Mini Golf | ✅ WORKS | 11,166 | — |
| 1500C | Sims Bowling | ✅ WORKS | 3,296 | — |
| 1500E | Sims Pool | ✅ WORKS | 3,397 | — |
| 50514 | Solitaire | ⚠️ PARTIAL | 346 | Limited texture uploads |
| 12345 | Vortex | ⚠️ PARTIAL | 99 | Limited texture uploads |
| 44444 | Zuma | ⚠️ DMA WAIT | 42 | DMA registers at 0x1402_000c not emulated |
| 55555 | Bejeweled | ⚠️ DMA WAIT | 180 | DMA registers at 0x1402_000c not emulated |
| 50513 | Sudoku | 👀 MINIMAL | 2 | Not enough texture uploads |
| 11002 | TWA/iQuiz | ❌ NO GFX | 0 | Needs AsyncFileIO:7 dir enumeration for .ipd |
| 1B200 | Lost | ❌ CRASH | 0 | ARM divide-by-zero in audio init |

## Fixes Applied

1. **HW Register Stub** (0x14000000..0x17FFFFFF): Read-zero/write-discard
   - Zuma & Bejeweled no longer crash with FatalMemException
   - They still hang waiting for DMA completion status

2. **Filesytem Import Handler**: Returns 1 for ordinal 0
   - TWA no longer logs "unhandled module" warning
   - Game still can't load .ipd textures (needs AsyncFileIO:7)

## Next Steps (Priority Order)

1. **DMA completion emulation** → Fix Zuma & Bejeweled (highest impact)
2. **AsyncFileIO:7 directory enumeration** → Fix TWA/iQuiz
3. **ARM UDIV exception handling** → Fix Lost
4. **More texture loading paths** → Fix Solitaire, Vortex, Sudoku

## Game Engine Classification

| Engine | Games | Asset Format | Status |
|--------|-------|-------------|--------|
| Tetris Runtime | Tetris, Cubis2, MiniGolf, Mahjong, MsPacman, Pacman, SimsBowling, SimsPool | `.pix` | ✅ Works |
| Hold'em Runtime | Texas Hold'em | `.ipd` + `.blob` via AsyncFileIO:3 | ✅ Works |
| iQuiz Runtime | TWA/iQuiz | `.ipd` via AsyncFileIO:7 | ❌ Needs dir enum |
| PopCap Runtime | Zuma, Bejeweled | `.ipd` + DMA hardware | ⚠️ DMA wait |
| Lost Runtime | Lost | `rserver.bin` | ❌ CPU crash |
| Sudoku Engine | Sudoku, Solitaire | Mixed | ⚠️ Partial |

## Environment

```bash
CLICKY_EXPERIMENTAL_GL_HLE=1
CLICKY_GL_GATE_B=1
CLICKY_GL_LIVE_CONTINUOUS=1
CLICKY_GL_PRESENT_VFLIP=1
RUST_LOG=EAPP_GL=info
```

**Platform:** Apple Silicon macOS | **Build:** `target/release/eapp`
