# Vortex (12345) Fix Gameplan

## Problem Statement

Vortex crashes with `FatalMemException` at `pc=0x18014d58`, `fault_addr=0x4` (null pointer write). This is one of 3 crashers (iQuiz, Vortex, Texas Hold'em) blocking 19% of the clickwheel game library.

## Root Cause Analysis

### Fault Details
- **PC**: `0x18014d58` (file offset `0x14d38`)
- **Fault**: `kind=Write fault_addr=0x00000004`
- **Code**: Structure-fill routine at `0x14d44-0x14d54`:
  ```armasm
  0x14d44: ldr r0,[r0,#4]    ; read buffer pointer from object+4
  0x14d54: stmia r0!,{r7,r9} ; write to buffer
  0x14d58: stmia r0!,{r4,r5,r6,lr} ; <-- FAULT: r0 was null+8
  ```
- **Register state**: `r0=0x8` (destination advanced from null by one `stmia` pair), `lr=0x00010000` (invalid return)

### Call Chain
1. Function at `0x14d38` is called from `0x7868: bl 0x206b0`
2. Caller at `0x7854-0x7868`:
   ```armasm
   0x7854: str r0, [r4]         ; writes container vtable
   0x7858: ldr r1, [r4, #0x54]  ; reads count from container+0x54
   0x7864: ldr r0, [r4, #0x5c]  ; reads array ptr from container+0x5c
   0x7868: bl 0x206b0           ; calls release loop
   ```
3. `[object+4]` (the buffer pointer) is null because an upstream HLE call failed to populate it

### Suspected Missing HLE: OpenGLES:165

From the investigation notes:
- `OpenGLES:165` is currently a no-op stub
- It is described as "surface handle" / "beginFrame / bindContext"
- The call signature: `r0=ctx, r1=ptr, r2=vtx_buf_ptr`
- Vortex likely expects this to write a framebuffer/buffer pointer into a state object

## Hypothesis

Vortex calls `OpenGLES:165` early in GL setup, expecting it to:
1. Allocate or bind a surface/framebuffer
2. Write the buffer pointer into a state object at a known offset
3. This pointer is later read at `[object+4]` and used as a destination for structure fills

Since `OpenGLES:165` is currently a no-op (returns 0 without writing anything), the pointer remains null, causing the crash when the game tries to write to it.

## Fix Strategy

### Option A: Minimal Surface Bind (Recommended)

Implement a minimal `OpenGLES:165` that:
1. Recognizes when it's being called with a surface/context bind request
2. Allocates or returns a synthetic framebuffer pointer (work-RAM backed)
3. Writes the pointer into the expected state object location

Implementation approach:
```rust
// In OpenGLES:165 handler
if r1 != 0 {
    // r1 points to a state object where we should write the buffer ptr
    // Allocate or reuse a synthetic surface buffer in work-RAM
    let surface_buf = allocate_surface_buffer(width, height);
    // Write the pointer into the state object at the expected offset
    write_guest_u32(r1 + 4, surface_buf)?;
}
```

### Option B: Passthrough to Live GL

If `OpenGLES:165` maps to a standard GL ES 1.1 call (like `eglMakeCurrent` or surface bind), we could:
1. Map the guest context to a host GL context
2. Have the host GL allocate the surface
3. Return a handle that satisfies the game's expectations

This is more complex and may not be necessary for Vortex to boot.

### Option C: Detect and Skip the Write

Instead of fixing the null pointer, detect when the game is about to write to null and:
1. Log the skip
2. Return early from the structure-fill function
3. Hope the game continues (may cause visual corruption but could allow boot)

This is a hack and not recommended.

## Validation Plan

### Phase 1: Confirm the Hypothesis
1. Add `CLICKY_EAPP_TRACE=OpenGLES:165` logging to capture all calls
2. Run Vortex headed for ~1 second to see if `OpenGLES:165` fires before the crash
3. Check register args to confirm the expected pattern (`r0=ctx, r1=ptr, r2=vtx_buf_ptr`)

### Phase 2: Implement Minimal Fix
1. Implement Option A (minimal surface bind)
2. Run Vortex headed smoke test (8 seconds)
3. Verify: no crash, draws rendered > 0

### Phase 3: Regression Testing
1. Run all 16 games smoke test
2. Verify the 13 working games still work (0 fatals, draws stable)
3. Check Texas Hold'em (33333) - may share the same fix
4. Check iQuiz (11002) - different crash site, may need separate fix

### Phase 4: Verify Visual Output (if Vortex boots)
1. Run Vortex headed for longer duration (30 seconds)
2. Check if frames are visually coherent (not just noise)
3. Compare to expected Vortex gameplay screenshots if available

## Risk Mitigation

### Risk: Breaking Other Games
Mitigation:
- Gate the `OpenGLES:165` fix behind a check for Vortex-specific patterns (e.g., specific `r1` value range, or only enable after detecting Vortex binary)
- Or, implement as a generic but minimal fix that only writes when `r1` points to valid work-RAM and current value is null
- Run full regression before committing

### Risk: Fixing Vortex But Not Texas Hold'em
Mitigation:
- Texas Hold'em likely shares the same root cause (same engine family)
- After Vortex fix, immediately test Texas Hold'em
- If different crash, investigate separately

### Risk: iQuiz is Different
Mitigation:
- iQuiz crashes at `pc=0x18001b08` (memcpy null destination)
- This is a different code pattern (memcpy vs structure-fill)
- May be `Metadata` object provider gap, not GL surface
- Accept that iQuiz may need separate investigation

## Success Criteria

| Criterion | Target |
|-----------|--------|
| Vortex boots without crash | ✅ 0 fatals in 8s headed run |
| Vortex renders draws | ✅ > 100 draws rendered |
| No regressions in other games | ✅ 13/13 working games still work |
| Texas Hold'em also fixed | ✅ or ❌ (document if separate) |

## Timeline

1. **Phase 1** (15 min): Add tracing, confirm hypothesis
2. **Phase 2** (30 min): Implement minimal `OpenGLES:165` fix
3. **Phase 3** (20 min): Run regression test
4. **Phase 4** (15 min): Visual verification if successful

Total: ~80 minutes of focused work

## Related Issues

- iQuiz (11002): `pc=0x18001b08` - memcpy null destination, likely different root cause
- Texas Hold'em (33333): Same crash family as Vortex, test after fix
- `OpenGLES:165` may also help LOST (1B200) which has zero draws after texture fix

## Code Pointers

- Current `OpenGLES:165` handler: `clicky-core/src/sys/eapp/mod.rs`
- Search for `OpenGLES::Ordinal(165)` or similar pattern
- The ordinal handlers are typically in a large match statement
- Vortex binary: `Games_RO/12345/Executables/vortex_1_1_2563290.bin`
- Load address: `0x18000000` (same as all games)

## Notes from Documentation

From `IPOD_GAMES_BRINGUP.md`:
> - `12345` Vortex: `FatalMemException pc=0x18014d58 kind=Write off=0x00000004`
>   - null-destination struct-fill at `pc=0x18014d58` after inline GL uploads
>   - `[object+4]` buffer ptr is null, likely GL surface bind gap

From investigation section:
> - Vortex: likely GL surface bind (`OpenGLES:165` "surface handle")

The documentation confirms our hypothesis. The fix is to implement `OpenGLES:165` surface binding.
