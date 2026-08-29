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
FATDIR      = $(BOOT_DIR)/qemu_fatdir
BOOT_EFI    = $(BOOT_DIR)/target/x86_64-unknown-uefi/release/agentic_bootloader.efi
KERNEL_ELF  = $(KERNEL_DIR)/target/x86_64-unknown-none/release/agentic_kernel

.PHONY: all bootloader kernel image run-qemu test-boot test-host clean

all: image

bootloader:
	cd $(BOOT_DIR) && $(CARGO) build --release

kernel:
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

test-host:
	mkdir -p _evidence/latest
	printf "No host unit tests exist yet.\n" | tee _evidence/latest/host-tests.log

clean:
	rm -rf $(BOOT_DIR)/target $(KERNEL_DIR)/target $(FATDIR)
	rm -rf _evidence/latest
