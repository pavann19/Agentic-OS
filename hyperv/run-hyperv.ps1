# Isolated Hyper-V Generation 2 Launcher for Agentic OS

param(
    [string]$VMName = "AgenticOS-Gen2",
    [string]$VhdxPath = "D:\Operating_System\hyperv\AgenticOS.vhdx",
    [int64]$MemoryMB = 512
)

# Check if running as Administrator
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    try {
        Write-Host "Elevating with Administrator privileges to manage Hyper-V..." -ForegroundColor Yellow
        Start-Process powershell -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -NoExit -File `"$PSCommandPath`""
        exit
    } catch {
        Write-Host "Hyper-V requires Administrator privileges to create and manage VMs." -ForegroundColor Red
        Write-Host "Please open PowerShell as Administrator (Win + X -> Terminal (Admin)) and run:" -ForegroundColor Cyan
        Write-Host "  powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`"" -ForegroundColor White
        exit 1
    }
}

Write-Host "=== Hyper-V Generation 2 VM Setup for Agentic OS ===" -ForegroundColor Cyan

# 1. Remove existing VM if present
$existingVM = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if ($existingVM) {
    Write-Host "Stopping and removing existing VM '$VMName'..."
    if ($existingVM.State -eq 'Running') {
        Stop-VM -Name $VMName -TurnOff -Force
    }
    Remove-VM -Name $VMName -Force
}

# Check for freshly built VHDX update
$newVhdx = "D:\Operating_System\hyperv\AgenticOS-new.vhdx"
if (Test-Path $newVhdx) {
    Write-Host "Updating $VhdxPath with newly built image..."
    Copy-Item $newVhdx $VhdxPath -Force
    Remove-Item $newVhdx -Force
}

# Ensure VHDX file is NOT sparse (Hyper-V vhdmp driver fails with 0xC03A001A if sparse)
$vhdxFile = Get-Item $VhdxPath
if (($vhdxFile.Attributes -band [System.IO.FileAttributes]::SparseFile) -ne 0) {
    Write-Host "Clearing sparse attribute from $VhdxPath..."
    $tmpPath = "$VhdxPath.tmp"
    [System.IO.File]::Copy($VhdxPath, $tmpPath, $true)
    Remove-Item $VhdxPath -Force
    Move-Item $tmpPath $VhdxPath -Force
}

# 2. Create Generation 2 VM (Generation 2 uses 64-bit UEFI)
Write-Host "Creating Generation 2 VM '$VMName'..."
New-VM -Name $VMName -Generation 2 -MemoryStartupBytes ($MemoryMB * 1024 * 1024) -VHDPath $VhdxPath | Out-Null

# 3. Configure Firmware: Disable Secure Boot (Agentic OS kernel is freestanding/unsigned)
Write-Host "Configuring UEFI firmware (Disabling Secure Boot)..."
Set-VMFirmware -VMName $VMName -EnableSecureBoot Off

# 4. Configure Memory and Processor
Set-VMMemory -VMName $VMName -DynamicMemoryEnabled $false
Set-VMProcessor -VMName $VMName -Count 2

# 5. Disable Automatic Checkpoints (prevents 0xC03A001A differencing disk error)
Set-VM -Name $VMName -CheckpointType Disabled

# 6. Configure COM1 Serial Port to a named pipe for kernel logs
Write-Host "Configuring COM1 serial port (\\.\pipe\AgenticOS_COM1)..."
Set-VMComPort -VMName $VMName -Number 1 -Path "\\.\pipe\AgenticOS_COM1"

# 7. Launch serial monitor in separate window to capture live diagnostics
Write-Host "Launching live serial log monitor..."
Start-Process powershell -ArgumentList "-ExecutionPolicy Bypass -File `"$PSScriptRoot\capture-serial.ps1`""

# 8. Start VM
Write-Host "Starting VM '$VMName'..."
Start-VM -Name $VMName

# 9. Launch Hyper-V Console (vmconnect)
Write-Host "Launching VMConnect GUI console..."
Start-Process "vmconnect.exe" -ArgumentList "localhost `"$VMName`""
Write-Host "=== Hyper-V Gen 2 VM is running and connected! ===" -ForegroundColor Green

