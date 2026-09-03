# Phase 8's other storage-half real driver verification: a real NVMe
# controller (`-device nvme,drive=...`) -- QEMU's own NVMe emulation,
# the same protocol docs/TIER2_HARDWARE.md's research names as the
# recommended T480's actual primary storage. Asserts the real Identify
# Controller self-check this driver performs, AND that virtio-blk/AHCI
# (both sharing PCI bus 0 with this device) remain unaffected -- the
# real regression the iommu.rs per-bus context-table fix targets, now
# checked with a FOURTH same-bus device.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-nvme.log",
    [string]$DiskImage = "_evidence\disk-nvme-test.img",
    [string]$NvmeDiskImage = "_evidence\disk-nvme-drive.img",
    [string]$AhciDiskImage = "_evidence\disk-nvme-ahci.img",
    [int]$BootWaitSeconds = 14
)

$ErrorActionPreference = "Stop"

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
foreach ($d in @($DiskImage, $NvmeDiskImage, $AhciDiskImage)) {
    if (-not (Test-Path $d)) {
        $fs = [System.IO.File]::Create($d)
        $fs.SetLength(16MB)
        $fs.Close()
    }
}

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
    "-drive", "file=$DiskImage,if=none,id=disk0,format=raw",
    "-drive", "if=none,id=ahcidisk,file=$AhciDiskImage,format=raw",
    "-device", "ide-hd,drive=ahcidisk,bus=ide.1",
    "-drive", "if=none,id=nvmedisk,file=$NvmeDiskImage,format=raw",
    "-device", "nvme,drive=nvmedisk,serial=deadbeef",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)

Remove-Item -Force $SerialLog -ErrorAction SilentlyContinue
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output"
    exit 1
}

$content = Get-Content $SerialLog -Raw
$checks = @(
    @{ Name = "NVMe device found";                Pattern = "NVME_FOUND" },
    @{ Name = "MMIO capability mapped";           Pattern = "NVME_MAPPED" },
    @{ Name = "IOMMU domain assigned";            Pattern = "NVME_IOMMU_DOMAIN_ASSIGNED" },
    @{ Name = "real Identify Controller self-check"; Pattern = [regex]::Escape('NVME_SELF_CHECK_PASS: real Identify Controller completed, model="QEMU NVMe Ctrl"') },
    @{ Name = "virtio-blk unaffected (4th same-bus device)"; Pattern = "FS_SELF_CHECK_PASS" },
    @{ Name = "AHCI unaffected (4th same-bus device)"; Pattern = [regex]::Escape('AHCI_SELF_CHECK_PASS: real IDENTIFY DEVICE completed, model="QEMU HARDDISK"') }
)

$allPassed = $true
foreach ($c in $checks) {
    if ($content -match $c.Pattern) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found"
        $allPassed = $false
    }
}

if ($allPassed) {
    Write-Output ""
    Write-Output "NVMe driver verified: real admin queue init, real Identify Controller command, real model string decoded, with virtio-blk AND AHCI (two more devices sharing the same PCI bus) both unaffected."
    exit 0
} else {
    Write-Error "NVMe verification failed -- see checks above."
    exit 1
}
