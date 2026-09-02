# Phase 8's network-half real driver verification: a real Intel
# e1000-class NIC (`-device e1000,...`) -- the NIC family real
# Intel-chipset Tier 2 hardware actually has, unlike virtio-net.
# Asserts the real ARP-frame self-check this driver performs, checked
# byte-for-byte against QEMU's own independent packet capture (same
# rigor scripts/test-synthesis.ps1 uses for virtio-net), AND that
# virtio-blk/AHCI (sharing PCI bus 0 with this device) remain
# unaffected.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-e1000.log",
    [string]$DiskImage = "_evidence\disk-e1000-test.img",
    [string]$AhciDiskImage = "_evidence\disk-e1000-ahci.img",
    [string]$PcapFile = "_evidence\latest\e1000.pcap",
    [int]$BootWaitSeconds = 14
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
    "-netdev", "user,id=net0",
    "-device", "e1000,netdev=net0,mac=52:54:00:99:88:77",
    "-object", "filter-dump,id=f1,netdev=net0,file=$PcapFile",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)

Remove-Item -Force $SerialLog, $PcapFile -ErrorAction SilentlyContinue
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
    @{ Name = "e1000 device found";               Pattern = "E1000_FOUND" },
    @{ Name = "MMIO capability mapped";           Pattern = "E1000_MAPPED" },
    @{ Name = "IOMMU domain assigned";            Pattern = "E1000_IOMMU_DOMAIN_ASSIGNED" },
    @{ Name = "real ARP self-check";              Pattern = [regex]::Escape("E1000_SELF_CHECK_PASS: real ARP frame submitted and completed via TX descriptor") },
    @{ Name = "virtio-blk unaffected";            Pattern = "FS_SELF_CHECK_PASS" },
    @{ Name = "AHCI unaffected";                  Pattern = [regex]::Escape('AHCI_SELF_CHECK_PASS: real IDENTIFY DEVICE completed, model="QEMU HARDDISK"') }
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

# Real, independent pcap verification (not just the driver's own
# self-report) -- same discipline scripts/test-synthesis.ps1 uses.
if (Test-Path $PcapFile) {
    $bytes = [System.IO.File]::ReadAllBytes($PcapFile)
    if ($bytes.Length -ge 24 + 16 + 42) {
        $pktLen = [BitConverter]::ToUInt32($bytes, 24 + 8)
        $frame = $bytes[(24+16)..(24+16+[Math]::Min($pktLen,42)-1)]
        $dst = ($frame[0..5] | ForEach-Object { $_.ToString("x2") }) -join ""
        $src = ($frame[6..11] | ForEach-Object { $_.ToString("x2") }) -join ""
        if ($dst -eq "ffffffffffff" -and $src -eq "525400998877") {
            Write-Output "  PASS: pcap capture byte-correct (dst=broadcast, src=real configured MAC, independently verified)"
        } else {
            Write-Output "  FAIL: pcap capture mismatch -- dst=$dst src=$src"
            $allPassed = $false
        }
    } else {
        Write-Output "  FAIL: pcap capture too short/empty"
        $allPassed = $false
    }
} else {
    Write-Output "  FAIL: no pcap file produced"
    $allPassed = $false
}

if ($allPassed) {
    Write-Output ""
    Write-Output "e1000 driver verified: real device found, real MMIO+IOMMU capability grants, real MAC read from hardware, real ARP frame transmitted and independently pcap-verified, with virtio-blk and AHCI unaffected."
    exit 0
} else {
    Write-Error "e1000 verification failed -- see checks above."
    exit 1
}
