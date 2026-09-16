# Native build — no Docker, no cross-platform build container. See
# README.md for the toolchain this targets and why.

CARGO      = cargo
# Real per-OS defaults, both still overridable with `make QEMU=... run-qemu`.
# On Windows: a full path, not a bare name -- make's recipes run under
# MSYS2 sh, whose PATH does not include the winget QEMU install dir even
# when it's on the invoking PowerShell's own PATH (confirmed: "command
# not found" from sh despite `qemu-system-x86_64.exe` working fine
# interactively). On Linux (CI, `apt-get install qemu-system-x86 ovmf`):
# both ship on PATH/a fixed share dir, no such workaround needed.
ifeq ($(OS),Windows_NT)
QEMU      ?= C:/Program Files/qemu/qemu-system-x86_64.exe
# Bundled with the winget QEMU install (share/) -- this build's stand-in
# for OVMF; both are EDK2 firmware builds, this one just ships with QEMU
# itself.
OVMF_CODE ?= C:/Program Files/qemu/share/edk2-x86_64-code.fd
else
QEMU      ?= qemu-system-x86_64
OVMF_CODE ?= /usr/share/OVMF/OVMF_CODE.fd
endif

BOOT_DIR    = boot_rs
KERNEL_DIR  = kernel_rs
FATDIR      = $(BOOT_DIR)/qemu_fatdir
BOOT_EFI    = $(BOOT_DIR)/target/x86_64-unknown-uefi/release/agentic_bootloader.efi
KERNEL_ELF  = $(KERNEL_DIR)/target/x86_64-unknown-none/release/agentic_kernel

# Every user_rs crate kernel_rs's OWN default (no --features) build
# embeds via include_bytes! -- confirmed exhaustively against a real
# clean-checkout CI failure (18 "couldn't read" errors, one per crate
# below, virtio_net_driver counted twice for its two variants), not
# assumed from reading source. This list existing at all is itself the
# fix for a real, previously-hidden gap: `make image` only ever worked
# on this machine because of years of leftover target/ build output
# from `scripts/test-*.ps1` runs masking the fact that the Makefile's
# own `userland` target never built most of what the kernel actually
# needs. A genuinely clean checkout (this repo's own CI) surfaced it.
USER_CRATES = \
	user_rs/serial_driver \
	user_rs/framebuffer_driver \
	user_rs/keyboard_driver \
	user_rs/virtio_blk_driver \
	user_rs/ahci_driver \
	user_rs/nvme_driver \
	user_rs/usb_xhci_driver \
	user_rs/netstack_driver \
	user_rs/net_client \
	user_rs/child_proc \
	user_rs/helper_proc \
	user_rs/shell \
	user_rs/terminal_emulator \
	user_rs/text_editor \
	user_rs/file_manager \
	user_rs/mouse_driver \
	user_rs/compositor_driver \
	user_rs/window_client_driver \
	user_rs/agent_demo \
	user_rs/agent_gateway \
	user_rs/e1000_driver

.PHONY: all bootloader kernel userland image run-qemu test-boot test-host clean

all: image

bootloader:
	cd $(BOOT_DIR) && $(CARGO) build --release

# Built BEFORE kernel: kernel_rs/src/user_driver.rs (and many other
# kernel_rs modules) embed these crates' compiled ELF64 output via
# include_bytes! at kernel compile time (no filesystem exists yet to
# load them from at runtime — see elf.rs's doc comment), so the kernel
# build fails outright if any of them hasn't run first.
userland:
	@for dir in $(USER_CRATES); do \
		echo "cd $$dir && $(CARGO) build --release"; \
		(cd $$dir && $(CARGO) build --release) || exit 1; \
	done
	@# virtio_net_driver is a special case: kernel_rs/src/virtio_net.rs
	@# embeds TWO separately-built variants of it (variants/*_good and
	@# variants/*_induced_fault, chosen at kernel build time by the
	@# synthesis_induced_fault feature) -- see
	@# scripts/build-virtio-net-variants.ps1's own doc comment for the
	@# full story. Replicated here with plain cargo so `make image`
	@# doesn't need PowerShell on the core Linux/CI path.
	mkdir -p user_rs/virtio_net_driver/variants
	cd user_rs/virtio_net_driver && $(CARGO) build --release
	cp user_rs/virtio_net_driver/target/x86_64-unknown-none/release/virtio_net_driver \
		user_rs/virtio_net_driver/variants/virtio_net_driver_good
	cd user_rs/virtio_net_driver && $(CARGO) build --release --features induced_fault
	cp user_rs/virtio_net_driver/target/x86_64-unknown-none/release/virtio_net_driver \
		user_rs/virtio_net_driver/variants/virtio_net_driver_induced_fault

kernel: userland
	cd $(KERNEL_DIR) && $(CARGO) build --release

# "image" here means the QEMU virtual-FAT directory, not a real .img file —
# no mtools/mformat is installed on this machine.
image: bootloader kernel
	mkdir -p "$(FATDIR)/EFI/BOOT"
	cp "$(BOOT_EFI)" "$(FATDIR)/EFI/BOOT/BOOTX64.EFI"
	cp "$(KERNEL_ELF)" "$(FATDIR)/kernel.elf"
	cp font.psf "$(FATDIR)/font.psf"

# WHPX (Windows Hypervisor Platform) hardware acceleration, not TCG software
# emulation -- real, measured fix for the ~5s input lag under the old default
# (see kernel_rs/src/thread.rs's switch_to/thread_trampoline doc comments for
# the real scheduler race this exposed and the fix, verified via 15/15 clean
# WHPX boots + the full TCG regression suite). WHPX requires
# kernel-irqchip=on (split is unsupported by WHPX) plus a real IOMMU device
# (kernel_rs/src/iommu.rs's own hard dependency once a device is assigned),
# matching the exact flags used in that verification. Requires "Windows
# Hypervisor Platform" enabled in Windows Features (Turn Windows features on
# or off) -- same mechanism as Hyper-V/WSL2, not risky to enable. Automated
# test scripts under scripts/ stay on TCG: several rely on
# kernel-irqchip=split for IOMMU-dependent device-assignment tests, which
# WHPX does not support.
run-qemu: image
	"$(QEMU)" -machine q35,accel=whpx,kernel-irqchip=on -m 256M \
		-device intel-iommu,intremap=on \
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
# instead of fighting make/MSYS's environment layer.
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
	rm -rf $(BOOT_DIR)/target $(KERNEL_DIR)/target $(USER_DIR)/target $(USER_DIR2)/target $(USER_DIR3)/target $(USER_DIR4)/target $(FATDIR)
	rm -rf _evidence/latest
