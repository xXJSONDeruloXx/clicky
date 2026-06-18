# iPodLinux IDE IRQ findings

## Symptom

ZeroSlackr/iPodLinux reached IDE probing and filesystem reads, but the guest repeatedly printed:

```text
hda: lost interrupt
```

That indicated the guest was issuing ATA reads but not reliably observing the completion interrupt.

## Root cause

The PP EIDE interrupt latch and the ATA device INTRQ line were modeled as the same signal.

Real hardware has two related but distinct concepts:

- ATA INTRQ: asserted by the drive and cleared when software reads task-file `Status`.
- PP EIDE IRQ latch/status: surfaced through the PP EIDE config register and cleared by writing the latch/ack bits there.

Conflating them created two failure modes:

- clearing the PP config register could accidentally clear the drive interrupt before Linux consumed task-file `Status`, causing lost IDE interrupts;
- refusing to clear from the PP config register could leave the platform IRQ stuck and cause interrupt storms.

## Fix

`EIDECon` now owns a platform IRQ latch separate from the ATA controller IRQ:

- the generic IDE controller asserts a private ATA IRQ line;
- `EIDECon::update_irq_latch()` edge-samples that private ATA line;
- the PP interrupt controller sees only the EIDE latch;
- PP EIDE config writes clear only the EIDE latch;
- task-file `Status` reads still clear ATA INTRQ inside the generic IDE controller.

This matches the expected split between platform interrupt acknowledgement and ATA interrupt acknowledgement.

## Test helper

`clicky-desktop` also has `--autorun-zeroslackr`, which synthesizes the iPodLoader menu sequence for ZeroSlackr:

```text
Down, Down, Action
```

The synthetic button presses are held across UI frames so the emulated CPU can sample them reliably.

## Validation

Validated with:

```bash
cargo check -p clicky-desktop
RUST_LOG=error cargo run --release -p clicky-desktop -- \
  --autorun-zeroslackr \
  --hle=target/ipod-linux/ipodloader2_test.bin \
  --hdd=mem:file=target/ipod-linux/zeroslackr_256m.img
```

The autorun path selected ZeroSlackr without manual input and reached the IDE probe/read path.
