# iPod Games Runtime ABI Analysis

## Summary

iPod clickwheel games (`.ipg` packages) contain native ARM executables in Apple's "eapp" container format. These games are **not** standalone firmware images or interpreted scripts—they are native ARM applications dynamically bound to an Apple/Pixo-derived runtime ABI.

This document captures the reverse-engineered runtime interface needed to implement high-level emulation (HLE) for these games.

> Note: ordinal names and argument layouts below are research notes unless backed by the trace fixture and decoder report. See `docs/EAPP_GL_TRACE_DECODER_REPORT.md` and `clicky-core/tests/fixtures/eapp/tetris_gl_trace.json` for confirmed evidence.

---

## 1. Package Structure (.ipg)

An `.ipg` is a deployment package (ZIP-compatible) containing:

**Host/Install-layer:**
- `Manifest.plist` / signatures / store metadata
- `*.bin.sinf` (FairPlay authorization)
- `iTunesMetaData`, `iTunesArtwork`
- `Resources/<language>/Description.xml`

**Game/Runtime-layer:**
- Native ARM executable (`.bin`)
- Assets: `.pix`, `.tga`, `.ipd`, `.anm`, `.rlb`, `.ro`, `.wav`, `.m4a`
- Game-specific data files

**Emulator implication:** Focus on the runtime layer—games already receive keyboard input via `InputEvents:0` stubs.

---

## 2. eapp Executable Format

**Header (0x28 bytes):**
```
0x00  "eapp" signature
0x04  load-address-like value (0x08)
0x08  format/runtime version (0x0c)
0x0c  header size (0x10)
0x10  import module chain pointer
0x14  entry pointer
0x18  init pointer
0x1c  unknown
0x20  unknown
0x24  aux pointer (frame callback)
```

**Observed VMAs:** File pointers resolve correctly with base `0x18000000`.

---

## 3. Import Module Architecture

Each module provides a table of functions accessed by ordinal:

```
module name (string)
function count (u32)
next module pointer
stub array (each: ldr pc, [literal])
literal target array (module, ordinal) -> function
```

**Current modules observed:**
- `OpenGLES`
- `AsyncFileIO`
- `Audio`
- `InputEvents`
- `Settings`
- `Metadata`
- `miscTBD`

**HLE strategy:** Patch literal entries to synthetic trampolines keyed by `(module_name, ordinal)`.

---

## 4. Graphics API (Apple GL-like, not raw GLES 1.1)

### Confirmed Ordinals

| Ordinal | Function | Evidence |
|---------|----------|----------|
| `GL:37` | **glDrawArrays** | disasm: `mov r0,#7(QUADS) mov r1,#0, mov r2,#4` |
| `GL:45` | createTexture/initObj | width/height args, once per asset |
| `GL:99` | **glTexImage2D** | r0=GL_TEXTURE_2D, r1=level, r2=format enum |
| `GL:137` | setVertexArrayFormat | r0=array_idx, r1=components, r2=GL_FIXED |
| `GL:157` | **submitFrame** | LAST call in aux frame |
| `GL:158` | **presentFrame** | FIRST call in aux, r0=0x3f001 token |
| `GL:159` | bindTexture + setVertexBuffer | r0=tex_id (small int), r1=vtx_ptr |
| `GL:165` | beginFrame | after present, before draw loop |
| `GL:169` | setPosition/translate | r1=x, r2=y (float), screen coords |
| `GL:4` | glTexParameteri | r0=GL_TEXTURE_2D, called before upload |
| `GL:12` | glClear (init) | r0=0x4000=GL_COLOR_BUFFER_BIT |
| `GL:13` | glClearColor (init) | r0-r3=0,0,0,1.0 (black) |

### Per-Frame Sequence (Tetris)

```
presentFrame (GL:158)     → display previous frame
beginFrame (GL:165)       → allocate new framebuffer
for each quad:
    setPosition (GL:169)  → x, y translation
    bindTexture (GL:159)  → texture ID + vertex buffer
    setArrayFormat (GL:137) → position: 4 comps, GL_FIXED
    enableArray (GL:40)   → enable position array
    setArrayFormat (GL:137) → texcoord: 2 comps, GL_FIXED  
    enableArray (GL:40)   → enable texcoord array
    bindDrawState (GL:175)
    prepareDraw (GL:125)
    drawArrays (GL:37)    → glDrawArrays(GL_QUADS, 0, 4)
    postDraw (GL:36)
submitFrame (GL:157)      → commit for next present
```

### Vertex Format
- **Type:** GL_FIXED (16.16 fixed-point)
- **Position:** 4 components (XYZW)
- **TexCoord:** 2 components (ST)
- **Primitive:** GL_QUADS (4 vertices per draw)

---

## 5. Texture Formats (.pix files)

Guest parses `.pix` headers; we only need to implement the upload ABI.

**GL format constants:**
- `GL_RGB (0x1907)` → `_565` files (16-bit opaque)
- `GL_RGBA (0x1908)` → `_5551`, `_4444` files (16-bit alpha)
- `GL_ALPHA (0x1906)` → `_a8` files (8-bit mask)

**Header:** ~72 bytes before raw pixel data.

---

## 6. AsyncFileIO ABI (Ordinal 3)

**Request object layout:**
```
+0x04  operation type (6 = read)
+0x14  destination buffer pointer (guest-allocated)
+0x18  expected byte count
+0x34  completion callback PC
+0x38  completion callback context
```

**HLE:** Copy file bytes to guest buffer, invoke callback.

---

## 7. Lifecycle Model

```
entry()           → game initialization
init(app_obj)     → optional runtime init
aux(app_obj)      → frame callback (repeated)
```

**Frame sequence per aux():**
1. GL:158 (present previous frame)
2. GL:165 (begin current frame)
3. Draw quads...
4. GL:157 (submit current frame)

---

## 8. Implementation Priority

### Tier 1: First Pixels
1. `GL:45` + `GL:4` + `GL:99` — texture upload
2. `GL:169` — setPosition
3. `GL:159` — bindTexture + vertex buffer
4. `GL:37` — drawArrays (software rasterize)
5. `GL:158` — presentFrame (copy framebuffer to window)

### Tier 2: Playable
- `GL:137` + `GL:40` — vertex array setup
- `GL:165` + `GL:157` — frame lifecycle
- Input event contract
- Save file semantics

### Tier 3: Compatibility
- Alternative .eapp versions
- Different renderer variants
- Audio handles
- Music library APIs

---

## 9. HLE Implementation Guide

### Do
- Preserve native ARM execution
- Patch import literals for HLE dispatch
- Let guest parse `.pix`, `.tga`, etc.
- Return synthetic handles (non-zero) for object APIs
- Separate read-only assets from writable saves

### Don't
- Implement full GLES 1.1
- Parse `.pix` headers in the emulator
- Create save files for unresolved paths
- Stub all audio APIs with zero returns

---

## 10. Research Instrumentation

### Import Trace Format
```
frame | cycle | module | ordinal | PC | LR | r0-r12 | SP | stack[64] | ret
```

### Pointer Classification
- null, integer, image ptr, work RAM ptr, stack ptr, code ptr, string, object, array

### Differential Traces
Compare runs with:
- No input vs. button press
- Wheel clockwise vs. counterclockwise
- First launch vs. existing save

---

*This analysis enables the "software sprite renderer" approach—implement ~15 ordinals for textured quads, not a full GLES stack.*