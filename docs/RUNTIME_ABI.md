# iPod Games Runtime ABI Analysis

## Summary

iPod clickwheel games (`.ipg` packages) contain native ARM executables in Apple's `eapp` container format. These games are **not** standalone firmware images or interpreted scripts—they are native ARM applications dynamically bound to an Apple/Pixo-derived runtime ABI.

This document intentionally keeps ordinal names conservative. Unless an item is directly backed by the trace fixture, decoder report, and/or disassembly, treat it as a research note.

See `docs/EAPP_GL_TRACE_DECODER_REPORT.md` and `clicky-core/tests/fixtures/eapp/tetris_gl_trace.json` for the current verified evidence.

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
0x24  aux pointer (recurring callback in Tetris)
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

### Confirmed call-site evidence

| Ordinal | Status | Evidence |
|---------|--------|----------|
| `GL:37` | confirmed | `DrawArrays(7, 0, 4)` from disassembly: `mov r0,#7(QUADS) mov r1,#0, mov r2,#4` |
| `GL:99` | confirmed | exact upload ABI table below |
| `GL:137` | confirmed | direct args `r0..r3`, plus stack args at `sp+0`, `sp+4` |
| `GL:40` | confirmed count | only `r0` is set at the call site; other registers are caller leftovers |
| `GL:159` | confirmed count | only `r0`, `r1` are set at the call site; other registers are caller leftovers |
| `GL:175` | confirmed count | only `r0`, `r1`, `r2` are set at the call site; `r3=0` |

### 99 ABI table

| Field | Location | Meaning |
|-------|----------|---------|
| `r0` | register | target |
| `r1` | register | level |
| `r2` | register | internal_format |
| `r3` | register | width |
| `sp+0x00` | stack | height |
| `sp+0x04` | stack | border |
| `sp+0x08` | stack | format |
| `sp+0x0c` | stack | pixel_type |
| `sp+0x10` | stack | source_ptr |

### Neutral / unresolved ordinals seen in Tetris

| Ordinal | Conservative label | Note |
|---------|---------------------|------|
| `45` | `Ordinal45State` | upload-prep-like; exact role unresolved |
| `4` | `Ordinal4State` | state/setup step before upload; exact role unresolved |
| `157` | `Ordinal157State` | frame-terminal call in Tetris; exact semantics unresolved |
| `158` | `Ordinal158State` | frame-initial call in Tetris; exact semantics unresolved |
| `165` | `Ordinal165State` | per-frame setup call; exact semantics unresolved |
| `169` | `Ordinal169State` | float-arg position/translation-like call; exact semantics unresolved |
| `125` | `Ordinal125State` | draw-state helper; exact semantics unresolved |
| `36` | `Ordinal36State` | post-draw helper; exact semantics unresolved |

### Per-frame sequence (conservative)

```
Ordinal158State(...)      → Tetris frame boundary, not universally proven as a global "present" call
Ordinal165State(...)      → per-frame setup
for each quad:
    Ordinal169State(...)  → float position/translation-like inputs
    Ordinal159State(...)  → small handle + vertex pointer-like inputs
    Ordinal137Array(...)  → position array format
    Ordinal40State(...)   → enable array
    Ordinal137Array(...)  → texcoord array format
    Ordinal40State(...)   → enable array
    Ordinal175State(...)
    Ordinal125State(...)
    DrawArrays(7, 0, 4)
    Ordinal36State(...)
Ordinal157State(...)      → frame-terminal call in Tetris, exact semantics unresolved
```

### Vertex format
- **Type:** GL_FIXED (16.16 fixed-point)
- **Position:** 4 components (XYZW)
- **TexCoord:** 2 components (ST)
- **Primitive:** GL_QUADS (4 vertices per draw)

---

## 5. Texture Formats (.pix files)

Guest parses `.pix` assets; the emulator only needs to implement the upload ABI.

**GL format constants:**
- `GL_RGB (0x1907)` → `_565` files (16-bit opaque)
- `GL_RGBA (0x1908)` → `_5551`, `_4444` files (16-bit alpha)
- `GL_ALPHA (0x1906)` → `_a8` files (8-bit mask)

**Payload offsets:** format- and asset-dependent. Do **not** assume a universal `~72` byte header. Use the guest-provided upload pointer and follow the trace-decoded offset for each asset.

One decoded example:
- `eaLogo_5551.pix` payload offset: `70` bytes

---

## 6. AsyncFileIO ABI (Ordinal 3)

**Request object layout:**
```
+0x04  operation type (6 = read)
+0x14  destination buffer pointer (guest-allocated)
+0x18  expected byte count
+0x34  completion callback PC
+0x38  completion callback context
+0x3c  source file handle / staging state (observed indirectly)
```

**HLE:** Copy file bytes to guest buffer, invoke callback.

---

## 7. Lifecycle Model

```
entry()            → game initialization
init(app_obj)      → optional runtime init
aux(app_obj)       → recurring callback (frame-like in Tetris; not universally confirmed as the sole frame callback)
```

---

## 8. Implementation Priority

### Tier 1: First Pixels
1. `Ordinal45State` + `Ordinal4State` + `GL:99` — texture upload path
2. `Ordinal169State` — conservative float position/translation call
3. `Ordinal159State` — small handle + vertex pointer-like call
4. `GL:37` — drawArrays (software rasterize)
5. `Ordinal158State` / `Ordinal157State` — frame boundary handling

### Tier 2: Playable
- `GL:137` + `GL:40` — vertex array setup
- `Ordinal165State` — per-frame setup
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

*This analysis enables the software sprite renderer approach: implement the verified upload + draw path first, then expand carefully from captured evidence.*
