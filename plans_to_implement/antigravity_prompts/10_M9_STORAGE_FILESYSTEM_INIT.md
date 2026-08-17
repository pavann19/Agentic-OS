# M9 Prompt: Storage, Filesystem, And Init

## Prompt For Antigravity

Implement milestone `M9: Storage, Filesystem, And Init`.

Inspect image creation in `Makefile`, bootloader disk layout, VMM/heap, process loader, and current QEMU command before editing.

Goal: load `/bin/init` from disk through a minimal filesystem path instead of embedding all behavior in the kernel.

## Required Changes

- Add block device abstraction:
  - `int block_read(BlockDevice* dev, uint64_t lba, uint32_t count, void* buffer)`
  - block size metadata
  - device name/id
- First block driver target: QEMU-friendly virtio-blk if feasible.
- If virtio-blk is too large for one stage, add a documented temporary boot-volume read path and leave virtio-blk as the next task.
- Add read-only VFS:
  - `vfs_mount`
  - `vfs_open`
  - `vfs_read`
  - `vfs_close`
  - path lookup
- Add read-only FAT32 support because the image already uses FAT tooling.
- Update image creation to include:
  - `/bin/init`
  - `/etc/system.conf` placeholder if useful
- Connect process loader to VFS so the kernel loads `/bin/init` from disk.

## Non-Goals

- Do not implement write support yet.
- Do not implement journaling.
- Do not implement permissions beyond read-only structure.
- Do not implement a package manager.

## Verification

Run:

```sh
make clean all
make test-boot
```

Serial log must show:

- root filesystem mounted
- `/bin/init` opened
- `/bin/init` loaded
- init entered or started

## Acceptance Criteria

- Kernel reads a file from the boot image through VFS.
- `/bin/init` is loaded from disk, not hardcoded into kernel behavior.
- Missing init produces a clear serial error.
- FAT parsing errors fail closed.
