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
| Bejeweled | 55555 | — | ⚠️ DMA WAIT | [→](55555_bejeweled.md) |
| Zuma | 44444 | — | ⚠️ DMA WAIT | [→](44444_zuma.md) |
| Solitaire | 50514 | — | ⚠️ PARTIAL | — |
| Vortex | 12345 | — | ⚠️ PARTIAL | — |
| Sudoku | 50513 | — | 👀 MINIMAL | — |
| TWA/iQuiz | 11002 | — | ❌ NO GFX | [→](11002_twa.md) |
| Lost | 1B200 | — | ❌ CRASH | [→](1B200_lost.md) |

## Running Games

### Working Games (9)

Each working game has a launch script in `scripts/`:

```bash
# Tetris (most tested)
./scripts/tetris.sh

# Any other working game
./scripts/cubis2.sh
./scripts/holdem.sh
./scripts/mspacman.sh
./scripts/pacman.sh
./scripts/mahjong.sh
./scripts/minigolf.sh
./scripts/simsbowling.sh
./scripts/simspool.sh
```

Common options (all scripts support these):
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

These are set automatically by the launch scripts.

### Bundle Directory

Games live in the preservation dump at:
```
~/Downloads/16-ipod-games/Games_RO/<bundle_id>/
```

Override with environment variables:
```bash
TETRIS_BUNDLE=/path/to/66666 ./scripts/tetris.sh
CUBIS2_BUNDLE=/path/to/99999 ./scripts/cubis2.sh
# etc.
```

## Game Engines

| Engine | Games | Key Features |
|--------|-------|-------------|
| **Tetris Runtime** | Tetris, Cubis 2, Mini Golf, Mahjong, Ms. Pac-Man, Pac-Man, Sims Bowling, Sims Pool | `.pix` textures via AsyncFileIO:3, mode=7 quads |
| **Hold'em Runtime** | Texas Hold'em | `.ipd`/`.blob` via AsyncFileIO:3, `Filesytem` import |
| **iQuiz Engine** | TWA/iQuiz | `.ipd`/`.blob` via AsyncFileIO:7, `Filesytem` import |
| **PopCap Engine** | Zuma, Bejeweled | DMA register writes at 0x1402_000c |
| **Lost Engine** | Lost | `rserver.bin` resource server |

## Troubleshooting

### Green Screen
If you see a green screen, the GL HLE environment variables are not set. Use the launch scripts instead of running `eapp` directly.

### FatalMemException
If a game crashes with `FatalMemException`, it's trying to access unmapped memory. The HW stub at `0x14000000..0x17FFFFFF` handles most cases, but PopCap engine games (Zuma, Bejeweled) need proper DMA emulation.

### No Textures / Black Screen
If the game renders but shows no textures, the texture upload path may not be handled. This affects games that use `.ipd` files via `AsyncFileIO:7` instead of `.pix` files via `AsyncFileIO:3`.

## See Also

- [Compatibility Report](../game_tests/20260625_compatibility_report.md)
- [Debug Analysis](../game_tests/debug_analysis.md)
- [EAPP Format Specification](../EAPP_FORMAT_SPECIFICATION.md)
- [Emulator Architecture](../EMULATOR_ARCHITECTURE.md)
