# Cleanup script for Hyper-V Generation 2 VM

param(
    [string]$VMName = "AgenticOS-Gen2"
)

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Start-Process powershell -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -File `"$PSCommandPath`""
    exit
}

$existingVM = Get-VM -Name $VMName -ErrorAction SilentlyContinue
if ($existingVM) {
    if ($existingVM.State -eq 'Running') {
        Stop-VM -Name $VMName -TurnOff -Force
    }
    Remove-VM -Name $VMName -Force
    Write-Host "Hyper-V VM '$VMName' has been cleanly removed." -ForegroundColor Green
} else {
    Write-Host "VM '$VMName' not found." -ForegroundColor Yellow
}
