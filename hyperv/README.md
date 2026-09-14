# Agentic OS — Hyper-V (Generation 2) Isolated Test Environment

This folder contains the completely isolated Hyper-V Generation 2 test setup for Agentic OS.

## Files
- `AgenticOS.vhdx`: Bootable virtual hard disk containing the FAT32 EFI partition with `BOOTX64.EFI`, `kernel.elf`, and `font.psf`.
- `run-hyperv.ps1`: Script to create, configure (UEFI Gen 2, Secure Boot Disabled), connect COM1 to a named pipe, launch the serial monitor, start the VM, and open the `vmconnect` console window. Self-elevates to Administrator if run from a standard prompt.
- `capture-serial.ps1`: Connects to `\\.\pipe\AgenticOS_COM1` to stream live boot diagnostics and save them to `hyperv-serial.log`.
- `cleanup-hyperv.ps1`: Script to stop and remove the `AgenticOS-Gen2` VM cleanly.

## Running
In an elevated Administrator PowerShell prompt:
```powershell
powershell -ExecutionPolicy Bypass -File D:\Operating_System\hyperv\run-hyperv.ps1
```
This will:
1. Re-create the `AgenticOS-Gen2` VM cleanly with COM1 redirected to `\\.\pipe\AgenticOS_COM1`.
2. Pop up the **Agentic OS - Hyper-V Serial Output** window showing live bootloader and kernel diagnostics.
3. Launch the **VMConnect** graphical console window.
