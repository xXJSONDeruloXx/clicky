# Tetris OpenGLES Trace Decoder Report

## Scope

This report is based on the captured Tetris trace fixture:

- `clicky-core/tests/fixtures/eapp/tetris_gl_trace.json`
- bundle: `Games_RO/66666`
- executable: `Executables/Tetris_1_1_2563292.bin`
- capture window: frames `0..=50`
- deduplication: identical repeated frames are merged

The fixture contains 5 unique frame records. Frame `4` repeats `47` times and is the steady-state render loop.

---

## Confirmed facts

### 1) The game enters a repeated frame loop after initialization

Captured frame sequence:

- frame `0`: ordinals `13`, `12`, `157`
- frame `1`: ordinals `35`, `165`, `167`, `165`, `157`
- frame `2`: texture-load burst plus frame setup
- frame `3`: transitional draw/setup frame
- frame `4`: stable repeating frame, repeated `47` times

This confirms that the runtime reaches a recurring render pass and does not stall after asset loading. The fixture `frame` value is the completed-frame counter observed when the import executed.

### 2) Ordinal `37` is `glDrawArrays`-like quad drawing

Direct disassembly from the Tetris binary shows:

- `mov r0, #7`
- `mov r1, #0`
- `mov r2, #4`
- `bl` import trampoline

The trace for that call records `r0=7`, `r1=0`, `r2=4`, which matches `glDrawArrays(GL_QUADS, 0, 4)`.

### 3) Ordinal `99` is the texture upload call

Evidence:

- called once per loaded asset in frame `2`
- receives a GL-style target in `r0` (`0x0de1`)
- receives a GL-style format enum in `r2` (`0x1906/0x1907/0x1908` depending on asset)
- receives width/height values and stack arguments consistent with upload metadata
- traced immediately after texture setup ordinals

### 4) Ordinal `158` is a present/swap-like frame boundary call

Evidence:

- called once per frame
- carries the constant surface token `0x0003f001` in `r0`
- carries a work-RAM pointer in `r2`
- occurs before the per-frame draw setup

### 5) Ordinal `169` is a translation/position-like call

Evidence:

- called with float-looking values (for example `160.0`, `240.0`)
- disassembly around the call site shows sign-bit XOR on float registers
- used as part of the per-sprite draw sequence

### 6) Ordinals `137` and `40` configure fixed-point vertex arrays

Evidence:

- `137` receives component counts and `0x140c` (`GL_FIXED`)
- `40` follows immediately and toggles array enables
- the pair appears repeatedly around each quad draw

### 7) Frame `4` is the stable render loop

Frame `4` contains one frame boundary call, one begin call, four draw clusters, and one submit call. The captured record repeats `47` times with the same signature.

---

## High-confidence interpretations

These are not fully proven by a single disassembly site, but the trace shape is consistent enough to treat them as likely:

- `45` = texture-object preparation / creation
- `4` = texture-state / bind step in the upload pipeline
- `165` = begin-frame / context binding
- `159` = bind texture + set vertex buffer
- `175` = bind draw state
- `125` = prepare draw
- `36` = post-draw cleanup
- `157` = frame submit / end-frame

Also likely:

- the `OpenGLES` ABI is a compact Apple graphics wrapper, not a full generic GLES entrypoint surface
- the game parses `.pix` data itself; the emulator only needs to deliver file bytes and support the runtime ABI

---

## Unresolved fields

These still need more tracing before being treated as facts:

1. **Exact signature of ordinal `4`**
   - it clearly participates in the texture pipeline
   - exact API name and argument order are still unclear

2. **Exact signature of ordinal `99`**
   - upload-like behavior is clear
   - the precise meaning of the stack arguments is still unresolved

3. **Exact signature of ordinals `45`, `165`, `175`, `125`, `36`, `157`**
   - each is render-critical
   - none has a fully confirmed semantic name yet

4. **Meaning of the `frame` field in the fixture**
   - it is a completed-frame counter observed at import time
   - it is not the same thing as a wall-clock or host VSync frame index

5. **Image-range pointer classification**
   - the capture currently labels file-image addresses as `code_pointer`
   - some of those pointers may actually refer to read-only data, not executable code

---

## Why this matters

This trace is enough to stop guessing and start building the renderer around the actual ABI shape:

- frame boundary token present
- texture upload path visible
- fixed-point vertex arrays visible
- four-vertex quad draw confirmed
- stable frame loop captured

That is enough to justify a renderer built from semantic commands, but not enough yet to hard-code exact ordinals without more evidence.