# Pointer-Backed Material Draws — ABI Analysis

## Summary

Two material handles (`0x100e38e0`, `0x100e5260`) are **guest-RAM pointers**, not
small integer material IDs. They drive the skipped draws 9–14 and 21–29 in the
29-draw menu frame. This document records the evidence-backed object layout and
isolates the **next missing primitive** required to render them correctly.

---

## 1. Pointer Handle Object Layout

### Discovery: handles point to EMPTY allocation buffers

Dumping 0x40 words (256 bytes) at each pointer handle address:

```
ptr_handle_object handle=0x100e38e0 addr=0x100e38e0 words=[all zeros × 64]
ptr_handle_object handle=0x100e5260 addr=0x100e5260 words=[all zeros × 64]
```

**Both handles point to zero-initialized scratch memory.** The pointer is not a
material object — it is an opaque token / allocation ID. The real material data
lives in the **state pointer** passed as `r1` at ordinal 159.

### Material state block layout (at `state_ptr`)

```
offset  0x00: vtable / setup function pointer (image memory)
                 0x100e38e0 state: 0x18023e24
                 0x100e5260 state: 0x18023e24  (same setup function)
offset  0x04: 0x00000001  (flags / mode)
offset  0x08: 0x00000000
offset  0x0c: 0x00000000
offset  0x10: 0x00000000
offset  0x14: 0x3f800000  (1.0f — scale or constant)
offset  0x18: 0x00000000
offset  0x1c: 0x41400000  (12.0f)   ← 0x100e38e0  (glyph cell height)
                 0x41800000  (16.0f)   ← 0x100e5260
offset  0x20: 0x00000000
offset  0x24: 0x3f800000  (1.0f)
offset  0x28: 0x41200000  (10.0f)   ← 0x100e38e0  (glyph cell width)
                 0x41800000  (16.0f)   ← 0x100e5260
offset  0x2c: 0x41400000  (12.0f)   ← 0x100e38e0
                 0x41800000  (16.0f)   ← 0x100e5260
```

The float pairs (10.0, 12.0) and (16.0, 16.0) match the **glyph cell dimensions**
observed in the position array bounds. This is a **texture matrix / texgen
parameter block**, not a simple color material.

---

## 2. Ordinal 148 — Font/Glyph Descriptor Configuration

Ordinal 148 is called immediately before pointer-backed menu draws with:
```
r0 = 4
r1 = 1
r2 = pointer to descriptor struct (work RAM)
r3 = 0
```

### Descriptor struct layout (at `r2`)

```
offset 0x00..0x0c: 4× fixed-point tile sizes
                    e.g. [1.0, 1.0, 1.0, 0.3]  (0x00010000, 0x00010000,
                                                  0x00010000, 0x00004ccc)
offset 0x10..0x1c: 4× float scale multipliers (all 1.0f = 0x3f800000)
offset 0x20:       0x01010001  (flags: format=1, type=1, ...)
offset 0x24..0x30: [1, 0, 1, 0]  (counts / indices)
offset 0x34..0x4c: 7× work-RAM pointers (glyph matrix tables)
                    ptr[0] at +0x34, ptr[1] at +0x38, ... ptr[6] at +0x4c
```

### Glyph matrix tables (the 7 sub-pointers)

Each of the 7 pointers targets a 16-word block that looks like **matrix rows**
(interleaved with zeros — likely IEEE-float 4×4 matrices stored sparsely):

```
slot 13: [fontObjPtr, 0,0,0,  0,0,0,0,  0,0,0,0,  atlasW,0,0,0]
slot 14: [atlasW,0,0,0,  glyphX,0,0,0,  1,0,0,0,  1,0,0,0]
slot 15: [glyphX,0,0,0,  1,0,0,0,  1,0,0,0,  0,0,0,0]
slot 16: [0,0,0,0,  0,0,0,0,  atlasW,0,0,0,  glyphX,0,0,0]
slot 17: [0,0,0,0,  atlasW,0,0,0,  glyphX,0,0,0,  1,0,0,0]
slot 18: [1,0,0,0,  1,0,0,0,  0,0,0,0,  0,0,0,0]
slot 19: [1,0,0,0,  0,0,0,0,  0,0,0,0,  0,0,0,0]
```

The float values (e.g. `171.0`, `170.0`, `172.0`) correspond to **atlas pixel
dimensions and glyph offsets**. These matrices transform a unit quad into the
per-glyph UV sub-rectangle within the font atlas.

**Conclusion:** Ordinal 148 configures a **batched text render descriptor**. The
7 sub-pointers are per-glyph transformation matrices. The setup function at
`state_ptr[0]` (0x18023e24) applies these matrices via **texgen** to generate
per-vertex UVs from the unit position quad.

---

## 3. Draw Group Analysis

### Handle `0x100e38e0` — draws 9–14 (6 glyphs)

- **Position array** (`0x101aa218`): ONE quad, 10×12 pixels (single glyph cell)
- **UV array** (`0x101aa0a8`): ONE quad mapping to **full atlas** `(0.5,90.5)→(132.5,-0.5)`
  → `menuTetrisLogo_4444.pix` (132×91)
- **Per-draw translation** advances X by 10px each draw
- **Result with current fix**: renders the full atlas 6 times side by side.
  Visually incorrect — should render 6 different glyph sub-regions.

The guest intends each draw to sample a **different sub-rectangle** of the atlas,
selected by the texgen matrix from the ordinal-148 descriptor. Without texgen,
all 6 draws sample the same full-atlas region.

### Handle `0x100e5260` — draws 21–29 (9 glyphs)

- **Position array** (`0x101a3748`): ONE quad, 16×16 pixels (single glyph cell)
- **UV array**: **NONE DEFINED** — only array slot 0 (position) is set
- **Per-draw translation** advances X by 20px each draw
- **Result**: skipped (no UV data available)

This handle relies **entirely on texgen** from the state block to generate UVs.
There is no client UV array at all. The texture matrix in the state block
(offsets 0x1c–0x2c) defines the glyph-to-atlas mapping.

---

## 4. Why UV Span Is None

| Handle | Root Cause |
|--------|-----------|
| `0x100e38e0` | UV array exists but was rejected by epoch check (now bypassed for pointer handles); UVs decode to full-atlas mapping, not per-glyph sub-rects |
| `0x100e5260` | No UV array defined at all; UVs must come from texgen / texture matrix in state block |

The common missing piece: **texture coordinate generation (texgen)** using the
matrix parameters from the material state block and the ordinal-148 descriptor.

---

## 5. Correlation with Loaded Assets

The pointer-backed draws correlate with these loaded A8 font atlases:

| Atlas | Dimensions | Likely Use |
|-------|-----------|-----------|
| `menuTetrisLogo_4444.pix` | 132×91 | Handle 0x100e38e0 (full-atlas UV currently) |
| `f10x12text1/2/3_a8.pix` | 980×24 | 10×12 font (matches 0x100e38e0 glyph cell 10×12) |
| `f16x16menu1/2/3_a8.pix` | 1568×32 | 16×16 font (matches 0x100e5260 glyph cell 16×16) |
| `f13x13menu1/2/3_a8.pix` | 1276×26 | 13×13 menu font |
| `menus_a8.pix` | 320×99 | Menu UI elements |

The glyph cell dimensions (10×12 and 16×16) match the font atlas tile sizes
exactly, confirming these are **text rendering draws**.

---

## 6. Current Implementation Status

### Applied evidence-backed fixes

1. **Epoch bypass for pointer handles**: UV arrays from previous materials are
   reused regardless of epoch when the current handle is a work-RAM pointer.
   (Rationale: pointer-backed materials use shared client arrays, not their own
   epoch-tagged definitions.)

2. **Array slot 2 fallback**: When array slot 1 has 4 components (color data,
   not UVs), attempt to use array slot 2 for 2-component UVs.

3. **Bounded skip warnings**: First occurrence per (handle, reason) pair is
   logged; subsequent identical skips are suppressed to avoid log flooding.

### Resulting behavior

- `0x100e38e0` draws 9–14: **rasterize** (full atlas × 6 — visually wrong but
  no longer skipped)
- `0x100e5260` draws 21–29: **still skipped** (no UV source available without
  texgen)

---

## 7. Next Missing Primitive

**Texture coordinate generation (texgen) via the material state texture matrix.**

To render pointer-backed draws correctly, the renderer must:

1. Parse the material state block at `state_ptr`:
   - Extract the texture matrix from offsets 0x1c–0x2c (glyph cell W/H and
     atlas mapping parameters).
2. Apply the ordinal-148 descriptor's per-glyph matrices:
   - The 7 sub-pointers define per-glyph UV transforms.
   - Each draw selects a glyph matrix based on draw index within the group.
3. Generate per-vertex UVs by transforming the unit position quad (0..1) through
   the combined texture matrix.
4. Use the generated UVs to sample the correct sub-rectangle of the bound font
   atlas.

This requires implementing the OpenGL ES 1.1 fixed-function **texture coordinate
generation** pipeline, which is a significant addition to the software
rasterizer. It should NOT be shortcut by hardcoding glyph positions or string
mappings.

---

## 8. Audit: "patched 15 placeholder Tetris resource slots"

The existing patch in `maybe_patch_guest_state` (pc range 0x18013d4c–0x18014020)
allocates placeholder resource slots (entries + 0x200-byte payloads) for indices
20–37 in a resource array owned by guest register r9.

- **What it changes**: zeroes 18 array slots → fills slots 20–37 with allocated
  entry/payload pointers.
- **Why it exists**: the guest resource loader leaves these slots unpopulated
  (likely awaiting async file delivery); without the patch, the guest
  dereferences NULL and crashes.
- **Dependency**: the pointer-backed material objects (`0x100e38e0`,
  `0x100e5260`) are allocated **after** this patch runs and are independent of
  the placeholder slot contents. The patch does not populate the font atlas
  descriptors or texgen matrices.

This patch should NOT be expanded to inject texgen data. The correct path is to
implement texgen in the renderer.

---

## 9. Artifacts

- Pointer-handle object dumps: `/tmp/tetris_ptr_probe.log`
- Array content dumps: `/tmp/tetris_array_probe.log`
- Glyph table dumps: `/tmp/tetris_glyph_probe.log`
- Draw detail with decoded UVs: same logs, `draw_detail` lines
