# Lost (1B200) — Technical Deep Dive

**Status:** ❌ NO GFX | **Draws:** 0 | **Engine:** Lost Engine (rserver.bin ARM rendering)

## Architecture Overview

Lost uses a fundamentally different rendering architecture from all other iPod games:

```
┌─────────────────────────────────────────────┐
│  Game Binary (0x10000000+)                   │
│  ┌─── EAPP Header (0x28 bytes) ────────────┐ │
│  │  magic="eapp", load_addr=0x10001000, ... │ │
│  └─────────────────────────────────────────┘ │
│  ┌─── Code/Data (0x10001xxx) ──────────────┐ │
│  │  ARM exception vectors, game logic       │ │
│  │  *** OVERWRITTEN BY rserver.bin ***       │ │
│  └─────────────────────────────────────────┘ │
│  ┌─── Game Assets (0x10005xxx+) ───────────┐ │
│  │  Textures, strings, etc.                  │ │
│  └─────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘

After AsyncFileIO:3 loads rserver.bin to 0x10001038:

┌─────────────────────────────────────────────┐
│  0x10001038..0x10001237: rserver header     │
│    ALL ZEROS (0x200 bytes = 128 words)      │
│    ← Ordinal 164 would write state here     │
│                                              │
│  0x10001238..0x1001A35C: rserver ARM code   │
│    The game's rendering engine               │
│    Executes on the iPod's ARM CPU directly   │
└─────────────────────────────────────────────┘
```

## rserver.bin Analysis

**File size:** 105,020 bytes (105KB)  
**Loaded at:** Guest address 0x10001038 (via AsyncFileIO:3)  
**Header:** 0x200 bytes of all zeros  
**Code:** Starts at offset 0x200 (guest 0x10001238)

The rserver.bin is NOT a shader binary for the GPU — it's ARM code that the iPod's CPU executes directly. The game's original binary at 0x10001038 (the ARM exception vectors and early code) gets completely overwritten by rserver.bin.

### Why Zeros in the Header?

On a real iPod, ordinal 164 (shader create) would:
1. Parse the rserver.bin binary structure
2. Extract function entry points and configuration data
3. Write the parsed results into the first 0x200 bytes (the "header")
4. Return a program handle

Since our ordinal 164 stub does NOT parse the binary, the header stays all zeros. The game's rendering engine code reads from the header region, finds null pointers/data, and skips all rendering.

## Material Object Analysis

When Lost binds material 0xe via ordinal 159, the state_ptr points to a structure at 0x18060910:

```
Offset  Value           Meaning
------  -----           -------
0x00    0x00000000      Flags/status
0x04    0x00000000      Flags/status
0x08    0x00000988      Texture 1 data size (2440 = 122×10×2 LA8)
0x0C    0x0000007A      Texture 1 width (122)
0x10    0x0000000A      Texture 1 height (10)
0x14    0x0000007A      Texture 1 width (confirmed)
0x18    0x0000000A      Texture 1 height (confirmed)
0x1C    0x00000000      Zero
0x20    0x0000190A      Texture 1 format (GL_LUMINANCE_ALPHA)
0x24    0x00001401      Texture 1 type (GL_UNSIGNED_BYTE)
0x28    0x00000000      Zero
0x2C    0x00000001      Texture 1 index
0x30    0x00000001      Texture 1 count
0x34    0x00000348      Texture 2 data size (840 = 42×10×2 LA8)
0x38    0x0000002A      Texture 2 width (42)
0x3C    0x0000000A      Texture 2 height (10)
0x40    0x0000002A      Texture 2 width
0x44    0x0000000A      Texture 2 height
0x48    0x00000000      Zero
0x4C    0x0000190A      Texture 2 format (GL_LUMINANCE_ALPHA)
0x50    0x00001401      Texture 2 type (GL_UNSIGNED_BYTE)
0x54    0x00000000      Zero
0x58    0x00000002      Texture 2 index
0x5C    0x00000002      Texture 2 count
0x60    0xFFFFFFFF      **Shader compilation state** (-1 = not compiled)
0x64+   0x00000000×7    All zeros
```

## Ordinal Trace

### Init (Frame 1)
```
153 → viewport(0, 0xFFFF, 0xFFFF, 0)
164 → shader_create(r1=0x10001038)     ← rserver.bin address
152 → program_query(buf, size_ptr)        ← check link status
153 → viewport(0, 0, 0, 0)
152 → program_query(buf, size_ptr)        ← second query
4   → bind_texture(0x84F5, 0)             ← GL_TEXTURE_2D
99  → upload(0x84F5, ..., 0x190A, 0x1401) ← 122×10 LA8
4   → bind_texture(0x84F5, 0)
99  → upload(0x84F5, ..., 0x190A, 0x1401) ← 42×10 LA8
```

### Frame Loop (repeats forever)
```
13  → glCullFace(0)                       ← no culling
12  → glClear(0x4000)                     ← clear color buffer
159 → bind_material(0xe, 0x18060910)      ← shader material
157 → present(0x0)                         ← show frame (nothing drawn)
```

**0 draws per frame.** The game never calls ordinal 37 (draw arrays) or 38 (draw elements).

## What We've Tried

### 1. Shader Program Handle Return
Made ordinal 164 return 1 (program handle) instead of 0.
**Result:** No change. Game still does `13,12,159,157`.

### 2. Program Query Success
Made ordinal 152 write GL_TRUE=1 to the query buffer and return program handle 1.
**Result:** No change.

### 3. Shader State Patch
Wrote 0 to [state_ptr+0x60] during present (to clear the 0xffffffff "not compiled" flag).
**Result:** Flag patched to 0, but game still doesn't draw. The rendering engine
has additional checks beyond this single flag.

### 4. Skip rserver.bin Load
Used `CLICKY_EAPP_SKIP_RSERVER=1` to prevent loading rserver.bin over the
original game binary.
**Result:** The original binary also had mostly zeros at 0x10001038 (it was
designed to have rserver.bin overwrite it). No change in rendering behavior.

## Root Cause

Lost's rendering engine IS the ARM code in rserver.bin. Our CPU emulator
DOES execute this code, but the engine's internal logic checks multiple
conditions before issuing draw calls:

1. **Header population:** The rendering engine reads from the 0x200-byte
   header at 0x10001038..0x10001237, expecting function pointers and
   configuration written by ordinal 164. Since all values are zero,
   the engine treats all function pointers as null and skips rendering.

2. **Shader binary parsing:** Ordinal 164 on a real iPod would parse the
   rserver binary format, find rendering functions, and write their
   addresses into the header region. This creates a dispatch table that
   the game's frame loop code uses to call the appropriate rendering
   functions.

Without a real implementation of ordinal 164, the rendering engine has
no valid dispatch table and cannot render.

## Possible Paths Forward

### A. Reverse Engineer Ordinal 164
Parse the rserver.bin format to understand:
- Where the function entry points are encoded
- What configuration data needs to be written to the header
- What the expected header layout is

This would require significant binary reverse engineering of both
rserver.bin and the iPod's GL ES implementation.

### B. Instrumented Tracing
Add read-watchpoints on the 0x200-byte header region to see exactly
which offsets the game code reads and what values it expects. This
would reveal the header structure without fully reverse engineering
ordinal 164.

### C. Shader State Machine Emulation
Build a complete implementation of the "shader compile" process:
- Parse rserver.bin format
- Extract rendering function addresses
- Write them to the header
- Let the game's ARM code call through them naturally

This is the most complete solution but also the most complex.

### D. Fixed-Function Fallback
If the game has a fixed-function rendering path (used when no shader
is available), we could trigger it by:
- Making ordinal 164 return 0 (compilation failed)
- The game might fall back to a simple rendering mode
- But from our experiments, this doesn't happen either

## Data Points

| Experiment | Shader State at 0x60 | Program Handle | Draw Count |
|-----------|----------------------|----------------|------------|
| Baseline  | 0xFFFFFFFF           | 0              | 0          |
| Handle=1  | 0xFFFFFFFF           | 1              | 0          |
| Patched   | 0x00000000           | 1              | 0          |
| Skip load  | 0x00000000 (orig)   | 0              | 0          |

The shader state and program handle are necessary but not sufficient
conditions for drawing. The rendering engine has additional internal
checks that we haven't identified yet.
