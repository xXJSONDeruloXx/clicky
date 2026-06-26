# iPod Clickwheel Games — Clicky Emulator

Game-by-game compatibility and launch documentation.

## Quick Reference

| Game | Bundle | Script | Status | Docs |
|------|---------|--------|--------|------|
| Tetris | 66666 | `./scripts/tetris.sh` | ✅ WORKS | [→](66666_tetris.md) |
| Cubis 2 | 99999 | `./scripts/cubis2.sh` | ✅ WORKS | [→](99999_cubis2.md) |
| Texas Hold'em | 33333 | `./scripts/holdem.sh` | ✅ WORKS | [→](33333_holdem.md) |
| Ms. Pac-Man | 14004 | `./scripts/mspacman.sh` | ✅ WORKS | [→](14004_mspacman.md) |
| Pac-Man | AAAAA | `./scripts/pacman.sh` | ✅ WORKS | [→](AAAAA_pacman.md) |
| Mahjong | 77777 | `./scripts/mahjong.sh` | ✅ WORKS | [→](77777_mahjong.md) |
| Mini Golf | 88888 | `./scripts/minigolf.sh` | ✅ WORKS | [→](88888_minigolf.md) |
| Sims Bowling | 1500C | `./scripts/simsbowling.sh` | ✅ WORKS | [→](1500C_simsbowling.md) |
| Sims Pool | 1500E | `./scripts/simspool.sh` | ✅ WORKS | [→](1500E_simspool.md) |
| Sudoku | 50513 | — | ✅ WORKS | [→](50513_sudoku.md) |
| Solitaire | 50514 | — | ✅/close | — |
| Vortex | 12345 | — | ⚠️ VBO | — |
| Bejeweled | 55555 | — | ⚠️ DMA | [→](55555_bejeweled.md) |
| Zuma | 44444 | — | 🔴 DMA STALL | [→](44444_zuma.md) |
| TWA/iQuiz | 11002 | — | 🔌 NO GFX | [→](11002_twa.md) |
| Lost | 1B200 | — | ❌ CRASH | [→](1B200_lost.md) |

**Summary:** 10 working, 2 close/partial, 1 VBO issue, 3 not working

## Running Games

### Working Games (10)

Each working game has a launch script in `scripts/`:

```bash
./scripts/tetris.sh                # most tested
./scripts/cubis2.sh                # highest draw count
./scripts/holdem.sh                # complex poker game
./scripts/mspacman.sh              # classic arcade
./scripts/pacman.sh                # classic arcade
./scripts/mahjong.sh               # tile matching
./scripts/minigolf.sh              # golf game
./scripts/simsbowling.sh           # bowling sim
./scripts/simspool.sh              # pool sim
```

Sudoku runs directly (no script yet):
```bash
./target/release/eapp /path/to/Games_RO/50513
```

Common script options:
```bash
./scripts/<game>.sh --timeout 15    # auto-terminate after 15s
./scripts/<game>.sh --headless      # no window (CI / testing)
./scripts/<game>.sh --verbose       # debug-level logging
./scripts/<game>.sh --dump 30       # dump first 30 frames as PPM
./scripts/<game>.sh --no-build      # skip cargo build
./scripts/<game>.sh --no-capture    # skip PPM frame captures
```

### Required Environment

All games require the experimental GL HLE renderer:

```bash
export CLICKY_EXPERIMENTAL_GL_HLE=1
export CLICKY_GL_GATE_B=1
export CLICKY_GL_LIVE_CONTINUOUS=1
export CLICKY_GL_PRESENT_VFLIP=1
```

Vflip is **auto-suppressed** for NDC-coordinate engines (Sudoku/Solitaire).
Launch scripts set these automatically.

### Bundle Directory

Games live in the preservation dump at:
```
~/Downloads/16-ipod-games/Games_RO/<bundle_id>/
```

Override with environment variables:
```bash
TETRIS_BUNDLE=/path/to/66666 ./scripts/tetris.sh
```

## Engine Classification

| Engine | Games | Coords | Vflip | Frame Begin | Assets |
|--------|-------|--------|-------|-------------|--------|
| Tetris Runtime | Tetris, Cubis 2, Mini Golf, Mahjong, Ms. Pac-Man, Pac-Man, Sims Bowling, Sims Pool | Pixel | Yes | ordinal-158 | .pix |
| Hold'em Runtime | Texas Hold'em | Pixel | Yes | ordinal-158 | .ipd/.blob |
| Sudoku/SS Engine | Sudoku, (Solitaire) | **NDC** | **No** | **None** | Minimal |
| PopCap Engine | Zuma, Bejeweled | Pixel | Yes | ordinal-158 | DMA + .ipd |
| iQuiz Engine | TWA/iQuiz | Pixel | Yes | ordinal-158 | .ipd (AsyncFileIO:7) |
| Lost Engine | Lost | Pixel | Yes | ordinal-158 | rserver.bin |

## Recent Changes

### 2026-06-26: Sudoku works, PopCap DMA foundation, 10/16 games rendering
- **Sudoku fix**: Three bugs found and fixed (auto-begin, NDC scaling, auto-vflip)
- **DMA framebuffer**: Foundation for PopCap engine (Bejeweled, Zuma) — pixel
  storage, overlay path, completion stubs. Gem sprites now visible in Bejeweled.
- **Solitaire**: Actually works at ~93% content, upgraded from partial
- DMA background not yet visible (needs interrupt/async mechanism)

### 2026-06-25: Initial compatibility report
- 9/16 games working, HW stub for 0x14000000 region, Filesytem handler added

## See Also

- [Compatibility Report](../game_tests/20260625_compatibility_report.md) — full metrics
- [Debug Analysis](../game_tests/debug_analysis.md) — root cause analysis
- [EAPP Format Specification](../EAPP_FORMAT_SPECIFICATION.md)
- [Emulator Architecture](../EMULATOR_ARCHITECTURE.md)
