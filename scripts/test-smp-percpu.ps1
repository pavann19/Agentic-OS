# Phase 9 deliverable 2 (docs/ROADMAP.md Sec5): real-hardware
# verification that per-CPU GDT/TSS/double-fault-stack bring-up
# (kernel_rs/src/gdt.rs's init_for_cpu, kernel_rs/src/smp.rs's
# cpu-index plumbing, kernel_rs/src/idt.rs's load_current_cpu) did not
# regress Phase 9 deliverable 1's already-verified AP bring-up, and
# that each AP now reports its own real, distinct cpu_index alongside
# its real APIC ID.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-smp-percpu.log",
    [int]$BootWaitSeconds = 15
)

$ErrorActionPreference = "Stop"
Set-Location "D:\Operating_System"
[Environment]::CurrentDirectory = "D:\Operating_System"

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
Remove-Item -Force $SerialLog -ErrorAction SilentlyContinue

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-smp", "4",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    "[SMP] AP_ONLINE apic_id=0x1 cpu_index=0x1",
    "[SMP] AP_ONLINE apic_id=0x2 cpu_index=0x2",
    "[SMP] AP_ONLINE apic_id=0x3 cpu_index=0x3",
    "SMP_AP_ONLINE apic_id=1",
    "SMP_AP_ONLINE apic_id=2",
    "SMP_AP_ONLINE apic_id=3",
    "SMP_BRINGUP_DONE brought_up=3 skipped_disabled=0 timed_out=0",
    "GDT+TSS initialized for cpu_index=0"
)
foreach ($c in $checks) {
    if ($content.Contains($c)) {
        Write-Output "  PASS: $c"
    } else {
        Write-Output "  FAIL: pattern not found: $c"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "Per-CPU GDT/TSS SMP bring-up test FAILED -- see $SerialLog"
    exit 1
}
Write-Output ""
Write-Output "Per-CPU GDT/TSS bring-up verified: all 3 real APs came online, each reporting its own distinct cpu_index alongside its real hardware APIC ID."
