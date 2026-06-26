# Sudoku (Bundle 50513)

**Status:** ✅ WORKS (splash) | **Draws:** 2 (8s) | **Frames Presented:** 120 | **Engine:** Sims/Sudoku/Solitaire Engine

## Quick Start
```bash
# Splash screen renders, game waits for input
./target/release/eapp /Users/kurt/Downloads/16-ipod-games/Games_RO/50513
```

## Bundle Info
- **Executable:** `Sudoku_1_1_2703081.bin` (eapp format)
- **Splash:** 320×240 Rgb565 fullscreen upload
- **Save File:** `savefile.dat` (loaded at 0 bytes if missing)

## Engine Behavior
Sudoku's engine never calls ordinal-158 (begin frame). Its per-frame loop
is **159 → 149 → 157** (bind → setup → present) with no draws when waiting
for input. This is now handled via auto-begin on present.

The game polls `InputEvents:0` every frame waiting for click wheel input.

## Assets
- Minimal — game logic is all in-code, only a single splash texture

## Environment
```bash
CLICKY_EXPERIMENTAL_GL_HLE=1
CLICKY_GL_GATE_B=1
CLICKY_GL_LIVE_CONTINUOUS=1
CLICKY_GL_PRESENT_VFLIP=1
```
