# Native (Docker-free) build. See NATIVE_BUILD.md for what's verified vs.
# wired-but-untested, and why each tool/flag choice was made.
#
# Docker/gnu-efi/mtools (Dockerfile, docker-compose.yml) are NOT used by any
# target below anymore. They're left in the tree, unused, in case a
# reproducible/CI build wants them back later — nothing here depends on them.
#
# boot/*.c and kernel/*.c (the pre-Rust C sources) are also NOT built by any
# target below. They stay as a porting reference for boot_rs/ and kernel_rs/
# — see PHASE0_PROGRESS.md and NATIVE_BUILD.md for the per-subsystem port
# checklist. Nothing here compiles them; `grep -r` them for reference only.

CARGO      = cargo
QEMU       = qemu-system-x86_64.exe
# Bundled with the winget QEMU install (share/) — this build's stand-in for
# OVMF; both are EDK2 firmware builds, this one just ships with QEMU itself.
OVMF_CODE ?= C:/Program Files/qemu/share/edk2-x86_64-code.fd

BOOT_DIR    = boot_rs
KERNEL_DIR  = kernel_rs
USER_DIR    = user_rs/serial_driver
FATDIR      = $(BOOT_DIR)/qemu_fatdir
BOOT_EFI    = $(BOOT_DIR)/target/x86_64-unknown-uefi/release/agentic_bootloader.efi
KERNEL_ELF  = $(KERNEL_DIR)/target/x86_64-unknown-none/release/agentic_kernel

.PHONY: all bootloader kernel userland image run-qemu test-boot test-host clean

all: image

bootloader:
	cd $(BOOT_DIR) && $(CARGO) build --release

# Built BEFORE kernel: kernel_rs/src/user_driver.rs embeds this crate's
# compiled ELF64 output via include_bytes! at kernel compile time (no
# filesystem exists yet to load it from at runtime — see elf.rs's doc
# comment), so the kernel build fails outright if this hasn't run first.
userland:
	cd $(USER_DIR) && $(CARGO) build --release

kernel: userland
	cd $(KERNEL_DIR) && $(CARGO) build --release

# "image" here means the QEMU virtual-FAT directory, not a real .img file —
# no mtools/mformat is installed on this machine yet. See NATIVE_BUILD.md.
# Staging kernel.elf here is a no-op today: boot_rs does not read it from
# disk yet (that's the next porting increment), but staging it now means
# this target doesn't need to change again once boot_rs does.
image: bootloader kernel
	mkdir -p "$(FATDIR)/EFI/BOOT"
	cp "$(BOOT_EFI)" "$(FATDIR)/EFI/BOOT/BOOTX64.EFI"
	cp "$(KERNEL_ELF)" "$(FATDIR)/kernel.elf"
	cp font.psf "$(FATDIR)/font.psf"

run-qemu: image
	"$(QEMU)" -machine q35 -m 256M \
		-drive if=pflash,format=raw,readonly=on,file="$(OVMF_CODE)" \
		-drive file=fat:rw:$(FATDIR),format=raw \
		-serial stdio

# Delegates to scripts/test-boot.ps1 as a REAL PowerShell process rather than
# invoking qemu directly from this recipe. Root cause: this MSYS2 make.exe
# does not propagate TMP/TEMP (or any exported Makefile variable — confirmed
# by testing) into the real Win32 environment of processes it spawns; `export`
# in a Makefile here only sets a shell-local variable in make's own recipe
# interpreter, it never reaches child-process environ. QEMU's
# `-drive file=fat:rw:DIR` needs a writable TMP/TEMP for scratch files and
# fails with "Could not open temporary file 'C:\...'" (silently falling back
# to the unwritable C:\ root) without it. A real PowerShell process has a
# genuine Win32 environment block, so this sidesteps the problem entirely
# instead of fighting make/MSYS's environment layer. Full debugging trail in
# NATIVE_BUILD.md if this breaks again.
# No args passed: scripts/test-boot.ps1's defaults already match this
# Makefile's QEMU/OVMF_CODE/FATDIR values. Override there if those diverge.
# Asserts BOOT_START -> EXIT_BOOT_SERVICES_OK -> KERNEL_ENTER: the full
# chain now works (bootloader loads kernel.elf and jumps to it for real).
test-boot: image
	powershell -ExecutionPolicy Bypass -File scripts/test-boot.ps1

# Real assertions against kernel_common (the pure logic kernel_rs actually
# runs, shared via a path dependency — not a parallel reimplementation
# being tested instead). See host_tests/src/lib.rs.
# --lib skips the doctest phase deliberately: rustdoc's doctest runner
# needs its own scratch temp dir, which hits the exact same make/MSYS2
# TMP-stripping issue documented at run-qemu above (Access is denied on
# C:\WINDOWS) -- and there are zero doc examples in host_tests/ to lose by
# skipping it. The real pass/fail gate is the grep below, not cargo test's
# own exit code through the pipe (which shell captures that reliably here
# is not worth relying on given everything else this Makefile already
# fought with this make/MSYS2 combination) -- a crash or panic before
# printing "test result: ok" fails the grep and this target either way.
test-faults: image
	powershell -ExecutionPolicy Bypass -File scripts/test-faults.ps1

test-host:
	mkdir -p _evidence/latest
	cd host_tests && $(CARGO) test --lib 2>&1 | tee ../_evidence/latest/host-tests.log
	grep -q "test result: ok\. [0-9]* passed; 0 failed" _evidence/latest/host-tests.log

clean:
	rm -rf $(BOOT_DIR)/target $(KERNEL_DIR)/target $(USER_DIR)/target $(FATDIR)
	rm -rf _evidence/latest
