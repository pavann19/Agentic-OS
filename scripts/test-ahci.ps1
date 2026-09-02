# Phase 8's storage-half real driver verification: a real AHCI (SATA)
# disk attached to QEMU's built-in `ich9-ahci` controller (00:1f.2,
# already enumerated since Phase 3) -- `-device ide-hd,...,bus=ide.1`
# (`ide.0` is already the FAT boot drive's own default interface unit).
# Asserts the real IDENTIFY DEVICE self-check this driver performs.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-ahci.log",
    [string]$DiskImage = "_evidence\disk-ahci-test.img",
    [string]$AhciDiskImage = "_evidence\disk-ahci-sata.img",
    [int]$BootWaitSeconds = 12
)

$ErrorActionPreference = "Stop"

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
foreach ($d in @($DiskImage, $AhciDiskImage)) {
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
    @{ Name = "AHCI device found";        Pattern = "AHCI_FOUND 00:1f\.2" },
    @{ Name = "MMIO capability mapped";   Pattern = "AHCI_MAPPED" },
    @{ Name = "IOMMU domain assigned";    Pattern = "AHCI_IOMMU_DOMAIN_ASSIGNED" },
    @{ Name = "real IDENTIFY self-check"; Pattern = [regex]::Escape('AHCI_SELF_CHECK_PASS: real IDENTIFY DEVICE completed, model="QEMU HARDDISK"') },
    @{ Name = "virtio-blk unaffected (real IOMMU context-table fix verification)"; Pattern = "FS_SELF_CHECK_PASS" }
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
    Write-Output "AHCI driver verified: real device found, real MMIO+IOMMU capability grants, real ATA IDENTIFY DEVICE command completed and decoded, with virtio-blk (a separate device sharing the same PCI bus) unaffected."
    exit 0
} else {
    Write-Error "AHCI verification failed -- see checks above."
    exit 1
}
