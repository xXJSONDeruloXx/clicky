# Lost (Bundle 1B200)

**Status:** ❌ NO GFX | **Draws:** 0 | **Engine:** Lost Engine (Programmable GPU)

## Quick Start
```bash
# Renders nothing - requires shader execution
./target/release/eapp /Users/kurt/Downloads/16-ipod-games/Games_RO/1B200
```

## Issue

Lost uses a **programmable GPU pipeline** — the only iPod game known to
do so. The game loads `rserver.bin` (105KB) via AsyncFileIO:3, then calls
`OpenGLES:164` with a pointer to that data as the shader binary. This is
`glCreateShaderProgram` or equivalent — the game compiles and links
a real GPU shader program during init.

### What we've stubbed
| Ordinal | Function | Our Return |
|---------|----------|------------|
| 164 | Shader create/link (rserver.bin) | Log + return 0 |
| 167 | Shader bind/use | Log + return |
| 152 | Program query (link status) | Write GL_TRUE=1 to buffer |
| 153 | Viewport | Log dimensions |

### Why it doesn't help
Even with ordinal-152 returning "link success", the game never issues
draw calls (37/38). The frame loop is just:

```
12 (clear) → 159 → 157 (present)
```

Zero draw ordinals per frame. The game's rendering code path is
entirely gated behind functional shader execution. Without a real
shader pipeline, the game draws nothing.

### What would be needed
1. **Shader binary parser** — rserver.bin appears to be compiled OpenGL ES
   shader programs in a custom binary format. Need to parse the format to
   extract vertex/fragment shader pairs.
2. **Shader compiler/interpreter** — Either compile to host GLSL/Metal
   shaders or interpret the shader bytecode in software.
3. **Uniform binding** — The game likely sets shader uniforms (textures,
   matrices) that our fixed-function HLE renderer doesn't track.

This is a fundamental architectural gap — our HLE renderer only supports
the fixed-function GL ES pipeline, while Lost uses programmable shaders.

## Init Ordinal Trace
```
153→164→152→153→152→4→99→4→99  (shader setup + 2 LA8 texture uploads)
```

Frame loop:
```
13→12→159(h0xe)→157(h0x0)  (clear, bind, present — no draws)
```

## Bundle Info
- **Executable:** `Lost_1_2_4526771.bin` (eapp format)
- **Shader binary:** `rserver.bin` (105,612 bytes, loaded via AsyncFileIO:3)
- **Textures:** 2 LA8 uploads (122×10, 42×10) — likely UI labels
- **Save data:** `options.sav` (loaded via AsyncFileIO:0)
- **Import Modules:** AsyncFileIO, miscTBD, OpenGLES, Metadata

## Environment
```bash
CLICKY_EXPERIMENTAL_GL_HLE=1
CLICKY_GL_GATE_B=1
CLICKY_GL_LIVE_CONTINUOUS=1
CLICKY_GL_PRESENT_VFLIP=1
```
