# Lost (Bundle 1B200)

**Status:** ❌ CRASH | **Draws:** 0 | **Engine:** Lost Engine

## Quick Start
```bash
# Crashes with divide-by-zero
./target/release/eapp /Users/kurt/Downloads/16-ipod-games/Games_RO/1B200
```

## Issue
Game loads `rserver.bin` (105KB) successfully, initializes audio subsystem,
then hits an ARM divide-by-zero exception:

```
Arithmetic exception: Divide By Zero
```

This is an actual CPU exception, not a rendering issue. Likely caused by
an uninitialized variable in the audio/timing code.

## Bundle Info
- **Executable:** `Lost_1_1_2917525.bin` (eapp format)
- **Asset Format:** `rserver.bin` (resource server) + `LuminanceAlpha88` textures

## Fix Needed
ARM UDIV exception handling (return 0 instead of crashing) or identify
the uninitialized divisor variable.

## Environment
```bash
CLICKY_EXPERIMENTAL_GL_HLE=1
CLICKY_GL_GATE_B=1
CLICKY_GL_LIVE_CONTINUOUS=1
CLICKY_GL_PRESENT_VFLIP=1
```
