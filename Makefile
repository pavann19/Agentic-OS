ARCH            = x86_64
TARGET          = BOOTX64.EFI
SRCS            = boot/main.c
OBJS            = boot/main.o

EFIINC          = /usr/include/efi
EFIINCS         = -I$(EFIINC) -I$(EFIINC)/$(ARCH) -I$(EFIINC)/protocol
LIB             = /usr/lib
EFILIB          = /usr/lib
EFI_CRT_OBJS    = $(EFILIB)/crt0-efi-$(ARCH).o
EFI_LDS         = $(EFILIB)/elf_$(ARCH)_efi.lds

CFLAGS          = $(EFIINCS) -fno-stack-protector -fpic \
                  -fshort-wchar -mno-red-zone -Wall 

LDFLAGS         = -nostdlib -znocombreloc -T $(EFI_LDS) -shared \
                  -Bsymbolic -L $(EFILIB) $(EFI_CRT_OBJS)

KERNEL_SRCS     = kernel/kernel.c kernel/print.c kernel/gdt.c kernel/idt.c kernel/interrupts.c kernel/memory.c kernel/io.c kernel/pic.c kernel/keyboard.c kernel/paging.c kernel/graphics.c kernel/desktop.c kernel/serial.c kernel/klog.c
KERNEL_OBJS     = $(KERNEL_SRCS:.c=.o)
KERNEL_CFLAGS   = -ffreestanding -O2 -nostdlib -mno-red-zone -mgeneral-regs-only -fno-exceptions -Wall -Wextra -Iinclude

all: $(TARGET) kernel.elf image

boot/main.o: boot/main.c boot/elf.h include/bootinfo.h
	gcc $(CFLAGS) -c $< -o $@

boot/main.so: boot/main.o
	ld $(LDFLAGS) $< -o $@ -lefi -lgnuefi

$(TARGET): boot/main.so
	objcopy -j .text -j .sdata -j .data -j .dynamic \
		-j .dynsym  -j .rel -j .rela -j .reloc \
		--target=efi-app-$(ARCH) $^ $@

%.o: %.c
	gcc $(KERNEL_CFLAGS) -c $< -o $@

kernel.elf: $(KERNEL_OBJS) kernel/linker.ld
	gcc -T kernel/linker.ld -o kernel.elf $(KERNEL_CFLAGS) $(KERNEL_OBJS)

image: $(TARGET) kernel.elf
	# Only recreate image if the bootloader or kernel changed
	dd if=/dev/zero of=os-image.img bs=1M count=64
	mformat -i os-image.img -F ::
	mmd -i os-image.img ::/EFI
	mmd -i os-image.img ::/EFI/BOOT
	mcopy -i os-image.img $(TARGET) ::/EFI/BOOT/BOOTX64.EFI
	mcopy -i os-image.img kernel.elf ::/kernel.elf
	mcopy -i os-image.img font.psf ::/font.psf

run-qemu: image
	qemu-system-x86_64 -machine q35 -m 256M -bios /usr/share/OVMF/OVMF_CODE.fd -drive format=raw,file=os-image.img -serial stdio -display gtk

test-boot: image
	mkdir -p _evidence/latest
	timeout 20s qemu-system-x86_64 -machine q35 -m 256M -bios /usr/share/OVMF/OVMF_CODE.fd -drive format=raw,file=os-image.img -serial file:_evidence/latest/serial.log -display none -no-reboot || true
	grep -q "KERNEL_ENTER" _evidence/latest/serial.log
	grep -q "PMM_INIT_DONE" _evidence/latest/serial.log

test-host:
	mkdir -p _evidence/latest
	printf "No host unit tests exist yet.\n" | tee _evidence/latest/host-tests.log

clean:
	rm -f boot/*.o boot/*.so $(TARGET) kernel/*.o kernel.elf os-image.img
	rm -rf _evidence/latest
