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
│  ┌─── 0x10001038+ ──────────────────────────┐ │
│  │  *** OVERWRITTEN BY rserver.bin ***       │ │
│  │  Original: ARM exception vectors + code   │ │
│  │  After load: rserver render engine code   │ │
│  └─────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

## rserver.bin — The Rendering Engine

- **Size:** 105,020 bytes (105KB)
- **Loaded to:** Guest address 0x10001038 (via AsyncFileIO:3)
- **Header region:** 0x10001038..0x10001237 (0x200 bytes, all zeros)
- **Code region:** 0x10001238+ (ARM/Thumb rendering functions)

**Key discovery:** rserver.bin is NOT a shader binary for the GPU. It's ARM code that the iPod's CPU executes directly. The game's original binary at 0x10001038 gets overwritten during loading.

## Experiments & Results

| Experiment | What We Did | Result |
|-----------|-------------|--------|
| Program handle return | ordinal 164 returns 1 (was 0) | Still 0 draws |
| Program query success | ordinal 152 writes GL_TRUE=1 + size=4 | Still 0 draws |
| Shader state patch | Write 0 to [material+0x60] (was 0xffffffff) | Still 0 draws |
| Skip rserver.bin load | Don't overwrite original game code | Still 0 draws (original also zero-padded) |
| **Fill header with values** | Write 1..128 to all 128 header words | **Still 0 draws** |

## Root Cause

The game's frame loop is `ordinal 13 → 12 → 159 → 157` (cull, clear, bind, present). **Zero draw calls per frame.** This is because:

1. **No shader activation:** Lost never calls ordinal 167 (glUseProgram). The game expects the shader to be implicitly active after ordinal 164, but our stubs don't make that happen.

2. **No vertex/draw setup:** The game doesn't call ordinals 137 (vertex array def), 40 (enable array), or 37/38 (draw). The entire rendering pipeline is gated behind conditions in the rserver.bin ARM code that we can't satisfy.

3. **Header isn't the blocker:** Even filling the entire 0x200-byte header with non-zero values doesn't enable rendering. The game's code has additional checks beyond the header.

## Material Object (0x18060910)

```
Offset  Value           Meaning
0x00    0x00000000      Flags
0x08    0x00000988      Tex1 size (2440 = 122×10×2 LA8)
0x0C    0x0000007A      Tex1 width (122)
0x10    0x0000000A      Tex1 height (10)
0x20    0x0000190A      Format: GL_LUMINANCE_ALPHA
0x24    0x00001401      Type: GL_UNSIGNED_BYTE
0x34    0x00000348      Tex2 size (840 = 42×10×2 LA8)
0x38    0x0000002A      Tex2 width (42)
0x3C    0x0000000A      Tex2 height (10)
0x60    0xFFFFFFFF      Shader state (-1 = not compiled)
```

## Ordinal Sequences

### Init (frame 1):
```
153 → viewport(0, 0xFFFF, 0xFFFF, 0)
164 → shader_create(r1=rserver_addr, r2=0xFFFFFFFF)
152 → program_query(buf, size_ptr)
153 → viewport(0, 0, 0, 0)
152 → program_query(buf, size_ptr)
4   → bind_texture(0x84F5, 0)
99  → upload(0x190A, 0x1401, 122×10)   ← LA8 text label
4   → bind_texture(0x84F5, 0)
99  → upload(0x190A, 0x1401, 42×10)    ← LA8 text label
```

### Frame loop (repeats forever):
```
13  → glCullFace(GL_NONE)
12  → glClear(GL_COLOR_BUFFER_BIT)
159 → bind_material(0xe, 0x18060910)
157 → present(0x0)
```

## Paths Forward

### A. Full rserver.bin reverse engineering
Parse the ARM code to find the rendering dispatch table and
what conditions it checks. Requires significant ARM RE effort.

### B. Memory access tracing
Add read-watchpoints on the header region to see exactly which
offsets the game reads and what values it expects. Would reveal
the header structure without fully reverse engineering ordinal 164.

### C. Shader interpreter
Parse the PowerVR MBX shader format from rserver.bin and
implement a software shader interpreter that produces the
rendering dispatch table entries.

### D. Fixed-function workaround
Force a basic rendering pipeline for Lost's material:
- Read the material object's texture descriptors
- Synthesize vertex arrays from the texture dimensions
- Inject a draw call after bind_material
- This would produce visible but wrong rendering

## Technical Details

- **rserver header:** 0x200 bytes at 0x10001038, all zeros before and after ordinal 164
- **Code entry:** 0x10001238 (offset 0x200 in rserver.bin)
- **Material state:** 0x18060910 with two LA8 texture descriptors
- **Shader state flag:** 0xffffffff at [0x18060910+0x60], patched to 0 by emulator
- **Program handle:** Returned as 1 by ordinal 164 (was 0)
- **GL imports:** Only 4, 12, 13, 99, 152, 153, 157, 159, 164
