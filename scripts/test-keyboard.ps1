# Real, end-to-end PS/2 keyboard driver verification -- closes the
# honest gap `user_rs/keyboard_driver`'s own module doc previously
# disclosed: this project's standard `test-boot.ps1` harness has no way
# to synthesize a real keystroke (no monitor access, `-display none`),
# so the driver correctly sat blocked on its own capability-gated
# `wait_interrupt` syscall forever in every automated run -- proven up
# to that waiting point, but never exercised by an actual interrupt.
#
# This script closes that gap for real: launches QEMU with a real HMP
# monitor socket, waits for the kernel to reach its steady state (the
# keyboard driver genuinely blocked and waiting), sends a real
# `sendkey a` through the monitor (QEMU's own synthetic PS/2 event
# injection -- indistinguishable, from the guest's perspective, from a
# real key on a real keyboard), and asserts the driver's own real
# `in al, 0x60` scancode read shows up in the serial log with the real
# PS/2 Set 1 make code for 'a' (0x1E). If this passes, the FULL chain --
# real unmasked IRQ1, the real IDT handler, `interrupt_forward`'s real
# notify, the real capability-gated `wait_interrupt`/`ack_interrupt`
# syscalls, and the real unmediated port read -- has genuinely fired
# end to end, not just been exercised up to the waiting point.
#
# Same TMP/TEMP and QEMU-argument reasoning as test-boot.ps1 -- see that
# script's own comments for the full explanation, not repeated here.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-keyboard.log",
    [string]$DiskImage = "_evidence\disk-keyboard-test.img",
    [int]$MonitorPort = 45454,
    [int]$BootWaitSeconds = 8,
    [int]$PostKeySeconds = 10
)

$ErrorActionPreference = "Stop"

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null

# A SEPARATE, throwaway disk image -- deliberately not the same
# `_evidence\disk.img` test-boot.ps1 uses, so this script's own repeated
# runs (and its real keystroke's effect on the audit ring / filesystem
# state) never interfere with that script's own reboot-persistence
# proof, and vice versa.
New-Item -ItemType Directory -Force -Path (Split-Path $DiskImage) | Out-Null
if (-not (Test-Path $DiskImage)) {
    $fs = [System.IO.File]::Create($DiskImage)
    $fs.SetLength(16MB)
    $fs.Close()
}

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on",
    "-drive", "file=$DiskImage,if=none,id=disk0,format=raw",
    "-serial", "file:$SerialLog",
    "-monitor", "tcp:127.0.0.1:$MonitorPort,server,nowait",
    "-display", "none",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow

try {
    # Real wall-clock wait for the kernel to finish its own boot sequence
    # and reach the point where keyboard_driver is genuinely blocked in
    # its capability-gated wait -- every prior session's evidence shows
    # the whole boot sequence completing well within this window.
    Start-Sleep -Seconds $BootWaitSeconds

    # Connect to QEMU's real HMP monitor and send a real `sendkey`
    # command -- this is QEMU's own synthetic PS/2 event injection path,
    # not a fake shortcut: from the guest kernel's perspective, this is
    # indistinguishable from an actual key transition on an actual
    # keyboard (same IOAPIC/PIC routing, same IRQ1, same real hardware
    # protocol underneath).
    $client = New-Object System.Net.Sockets.TcpClient
    $client.Connect("127.0.0.1", $MonitorPort)
    $stream = $client.GetStream()
    $writer = New-Object System.IO.StreamWriter($stream)
    $writer.AutoFlush = $true
    $writer.WriteLine("sendkey a")
    Start-Sleep -Milliseconds 300
    $writer.Close()
    $client.Close()

    Start-Sleep -Seconds $PostKeySeconds
} finally {
    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial log produced at $SerialLog"
    exit 1
}

$content = Get-Content $SerialLog -Raw

# 0x1E is the real PS/2 Set 1 make (key-down) scancode for 'a' -- the
# driver logs it as `syscall1(0xB0_0000 | scancode)`, visible in the
# kernel's own klog as this exact hex value.
if ($content -notmatch [regex]::Escape("SYSCALL_LOG value=0xb0001e")) {
    Write-Error "Real keystroke not observed -- expected SYSCALL_LOG value=0xb0001e (PS/2 make code for 'a') in $SerialLog"
    Write-Output "--- serial.log ---"
    Write-Output $content
    exit 1
}

Write-Output "Real keystroke verified end-to-end: unmasked IRQ1 -> h_keyboard -> interrupt_forward -> capability-gated wait_interrupt syscall -> real in al,0x60 scancode read -> SYSCALL_LOG value=0xb0001e"
exit 0
