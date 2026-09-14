# Interactive Serial Console for Agentic OS on Hyper-V
# Connects to \\.\pipe\AgenticOS_COM1 with bidirectional Read/Write so you can type live into Agentic OS!

param(
    [string]$PipeName = "AgenticOS_COM1",
    [string]$OutputFile = "$PSScriptRoot\hyperv-serial.log"
)

$Host.UI.RawUI.WindowTitle = "Agentic OS - Interactive Serial Console"
Write-Host "=== Agentic OS Interactive Serial Console ===" -ForegroundColor Cyan
Write-Host "Connected Pipe: \\.\pipe\$PipeName" -ForegroundColor Gray
Write-Host "Session Log:    $OutputFile" -ForegroundColor Gray
Write-Host "Waiting for VM to start..." -ForegroundColor Yellow

$csharpCode = @"
using System;
using System.IO;
using System.IO.Pipes;
using System.Text;
using System.Threading;

public class HyperVPipeBridge {
    public static void Run(string pipeName, string logFile) {
        using (var pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous)) {
            pipe.Connect(30000);
            Console.ForegroundColor = ConsoleColor.Green;
            Console.WriteLine("\n>>> Connected to Agentic OS Serial Bridge! Interactive keyboard ready.\n");
            Console.ForegroundColor = ConsoleColor.Magenta;
            Console.WriteLine("Tip: Typing here sends keystrokes directly into Agentic OS (Terminal window / shell)!");
            Console.WriteLine("Keys supported: Letters, Digits, Enter, Backspace, Arrow keys.\n");
            Console.ResetColor();

            var cts = new CancellationTokenSource();
            var readThread = new Thread(() => {
                byte[] buf = new byte[1024];
                try {
                    using (var writer = new StreamWriter(logFile, false, Encoding.UTF8)) {
                        writer.AutoFlush = true;
                        while (!cts.IsCancellationRequested && pipe.IsConnected) {
                            int n = pipe.Read(buf, 0, buf.Length);
                            if (n <= 0) break;
                            string text = Encoding.UTF8.GetString(buf, 0, n);
                            Console.Write(text);
                            writer.Write(text);
                        }
                    }
                } catch {}
            });
            readThread.IsBackground = true;
            readThread.Start();

            while (pipe.IsConnected && !cts.IsCancellationRequested) {
                if (Console.KeyAvailable) {
                    var key = Console.ReadKey(true);
                    if (key.Key == ConsoleKey.Enter) {
                        pipe.Write(new byte[] { 13 }, 0, 1);
                        pipe.Flush();
                    } else if (key.Key == ConsoleKey.Backspace) {
                        pipe.Write(new byte[] { 8 }, 0, 1);
                        pipe.Flush();
                    } else if (key.Key == ConsoleKey.LeftArrow) {
                        pipe.Write(new byte[] { 0x1B, (byte)'[', (byte)'D' }, 0, 3);
                        pipe.Flush();
                    } else if (key.Key == ConsoleKey.RightArrow) {
                        pipe.Write(new byte[] { 0x1B, (byte)'[', (byte)'C' }, 0, 3);
                        pipe.Flush();
                    } else if (key.Key == ConsoleKey.UpArrow) {
                        pipe.Write(new byte[] { 0x1B, (byte)'[', (byte)'A' }, 0, 3);
                        pipe.Flush();
                    } else if (key.Key == ConsoleKey.DownArrow) {
                        pipe.Write(new byte[] { 0x1B, (byte)'[', (byte)'B' }, 0, 3);
                        pipe.Flush();
                    } else {
                        char c = key.KeyChar;
                        if (c >= 32 && c <= 126) {
                            pipe.Write(new byte[] { (byte)c }, 0, 1);
                            pipe.Flush();
                        }
                    }
                }
                Thread.Sleep(10);
            }
            cts.Cancel();
        }
    }
}
"@

try {
    Add-Type -TypeDefinition $csharpCode -ErrorAction SilentlyContinue
} catch {}

try {
    [HyperVPipeBridge]::Run($PipeName, $OutputFile)
} catch [System.TimeoutException] {
    Write-Host "`n[ERROR] Timed out waiting for named pipe \\.\pipe\$PipeName." -ForegroundColor Red
} catch {
    Write-Host "`n[ERROR] Pipe exception: $($_.Exception.Message)" -ForegroundColor Red
} finally {
    Write-Host "`n=== Interactive Session Ended ===" -ForegroundColor Yellow
}
