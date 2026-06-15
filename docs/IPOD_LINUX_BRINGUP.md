# iPodLinux bring-up status

This is a checkpoint for the iPod 4G grayscale iPodLinux bring-up in clicky.
Generated firmware, HDD, rootfs, and patched historical Linux 2.4 build-tree artifacts live under ignored `target/ipod-linux/`.

## Current milestone

The emulator now gets iPodLinux well past early hardware initialization and into the root filesystem mount path:

- HLE firmware boots the iPodLinux loader/kernel image.
- Kernel reaches `prepare_namespace` with bring-up shims for the currently broken Linux 2.4 `context_thread`/keventd path.
- Static initcalls are restored in the ignored kernel tree, so ext2 and IDE init run normally.
- IDE probing and special commands complete; the previous ATA/IRQ stalls are fixed in tracked source.
- `/dev/root` resolves to the HDD's third partition (`/dev/hda3`, `0x0303`).
- `ext3` is tried first and fails as expected.
- `ext2_read_super` for `/dev/root` reads the superblock, matches magic `0xef53`, reads group descriptors, and reaches `iget(sb, EXT2_ROOT_INO)` for the root inode.

## Latest blocker

Root inode loading currently stalls after submitting the inode-table block read:

```text
ext2_read_super iget root inode
iget4 sb=0x0027bc00 ino=0x00000002
ext2_read_inode inode=0x002ff080 ino=0x00000002
ext2_read_inode bread dev=0x00000303 block=0x00000084 size=0x00000400
submit_bh rw=READ bh=0x00253f60
__wait_on_buffer bh=0x00253f60
__wait_on_buffer scheduling bh=0x00253f60
```

Earlier ext2 superblock and group-descriptor reads complete through IDE interrupt and `end_buffer_io_sync`, so the active question is why this later root-inode block read does not complete within the diagnostic window.

## Important ignored artifact shims

These are intentionally under ignored `target/ipod-linux/kernel-2.4-build/` and are not committed source changes:

- Linux 2.4 modern-toolchain build fixes.
- `copy_thread`/`memset` compatibility shims for old ARM assembly assumptions.
- `include/linux/init.h` `used` attributes to retain static initcalls with modern GCC.
- Temporary bring-up bypasses for `start_context_thread`, `flush_scheduled_tasks`, and selected thread-backed initcalls.
- Temporary `sys_mount` cleanup leak in `fs/namespace.c` to bypass old-kernel slab/free corruption while rootfs bring-up is diagnosed.

## Rootfs image note

The rootfs must be generated with Linux-2.4-compatible 128-byte ext2 inodes. The current ignored image was rebuilt with:

```bash
/opt/homebrew/bin/mke2fs -q -t ext2 -b 1024 -I 128 \
  -O filetype,sparse_super,large_file \
  -d target/ipod-linux/rootfs-src \
  -F target/ipod-linux/rootfs.ext2 32768

dd if=target/ipod-linux/rootfs.ext2 \
  of=target/ipod-linux/ipodlinux_hdd.img \
  bs=512 seek=47104 conv=notrunc status=none
```

Verification of partition 3 currently shows:

```text
magic=53ef inode_size=128 compat=0x38 incompat=0x2 ro_compat=0x3
```

## Next priorities

1. Instrument the specific root inode read path: `ext2_read_inode`, `bread`, `ll_rw_block`, `submit_bh`, `__wait_on_buffer`, IDE request completion, and `end_buffer_io_sync` for block `0x84`.
2. Determine whether the root-inode read is losing an IDE completion/interrupt, stuck in buffer wait state, or reading malformed/uninitialized data.
3. If root inode loads, continue to `d_alloc_root`, `prepare_namespace` success, `sys_chroot`, `run_init_process`, and then document/add real ARM/uClinux userspace as needed.
