# Live Agent Bridge Host CLI (scripts/agent-bridge-send.ps1)
# Sends a typed, length-prefixed request frame to the running Agentic OS guest over COM2 (TCP socket)
# and prints the decoded, typed response.
#
# Usage examples:
#   powershell -ExecutionPolicy Bypass -File scripts/agent-bridge-send.ps1 -Command ListWindows
#   powershell -ExecutionPolicy Bypass -File scripts/agent-bridge-send.ps1 -Command InjectKey -Surface 1 -Arg0 0x1E
#   powershell -ExecutionPolicy Bypass -File scripts/agent-bridge-send.ps1 -Command InjectClick -Surface 1 -Arg0 10 -Arg1 20
#   powershell -ExecutionPolicy Bypass -File scripts/agent-bridge-send.ps1 -Command FocusWindow -Surface 1
#   powershell -ExecutionPolicy Bypass -File scripts/agent-bridge-send.ps1 -Command QueryBounds -Surface 1

param(
    [string]$HostAddr = "127.0.0.1",
    [int]$Port = 4444,
    [Parameter(Mandatory=$true)]
    [ValidateSet("ListWindows", "InjectKey", "InjectClick", "FocusWindow", "QueryBounds")]
    [string]$Command,
    [uint32]$Surface = 1,
    [uint32]$Arg0 = 0,
    [uint32]$Arg1 = 0,
    [int]$TimeoutMs = 5000
)

$ErrorActionPreference = "Stop"

# Protocol request IDs
$REQ_LIST_WINDOWS = 1
$REQ_INJECT_KEY    = 2
$REQ_INJECT_CLICK  = 3
$REQ_FOCUS_WINDOW  = 4
$REQ_QUERY_BOUNDS  = 5

# Protocol response IDs
$RESP_OK_WINDOWS   = [uint32]0x80000001L
$RESP_OK_ACTION    = [uint32]0x80000002L
$RESP_DENIED       = [uint32]0x80000003L
$RESP_ERR          = [uint32]0x80000004L

$payload = [System.Collections.Generic.List[byte]]::new()

switch ($Command) {
    "ListWindows" {
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$REQ_LIST_WINDOWS))
    }
    "InjectKey" {
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$REQ_INJECT_KEY))
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$Surface))
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$Arg0))
    }
    "InjectClick" {
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$REQ_INJECT_CLICK))
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$Surface))
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$Arg0))
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$Arg1))
    }
    "FocusWindow" {
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$REQ_FOCUS_WINDOW))
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$Surface))
    }
    "QueryBounds" {
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$REQ_QUERY_BOUNDS))
        $payload.AddRange([System.BitConverter]::GetBytes([uint32]$Surface))
    }
}

$frameLen = [uint32]$payload.Count
$frame = [System.Collections.Generic.List[byte]]::new()
$frame.AddRange([System.BitConverter]::GetBytes($frameLen))
$frame.AddRange($payload)

$client = New-Object System.Net.Sockets.TcpClient
try {
    $connectTask = $client.ConnectAsync($HostAddr, $Port)
    if (-not $connectTask.Wait($TimeoutMs)) {
        Write-Error "Connection to Live Agent Bridge at $HostAddr`:$Port timed out."
        exit 1
    }
    $stream = $client.GetStream()
    $stream.ReadTimeout = $TimeoutMs
    $stream.WriteTimeout = $TimeoutMs

    # Send request frame
    $frameBytes = $frame.ToArray()
    $stream.Write($frameBytes, 0, $frameBytes.Length)
    $stream.Flush()

    function Read-Exact([System.IO.Stream]$s, [int]$count) {
        $b = New-Object byte[] $count
        $readTotal = 0
        while ($readTotal -lt $count) {
            $n = $s.Read($b, $readTotal, $count - $readTotal)
            if ($n -le 0) { throw "Unexpected EOF reading from Agent Bridge socket" }
            $readTotal += $n
        }
        return $b
    }

    # Read 4-byte response length
    $lenBytes = Read-Exact $stream 4
    $respLen = [System.BitConverter]::ToUInt32($lenBytes, 0)
    if ($respLen -lt 4) {
        throw "Invalid response frame length: $respLen"
    }

    # Read response type
    $typeBytes = Read-Exact $stream 4
    $respType = [System.BitConverter]::ToUInt32($typeBytes, 0)
    $remaining = $respLen - 4

    if ($respType -eq $RESP_OK_WINDOWS) {
        if ($remaining -lt 4) { throw "Malformed RespOkWindows" }
        $countBytes = Read-Exact $stream 4
        $winCount = [System.BitConverter]::ToUInt32($countBytes, 0)
        Write-Output "OK: $winCount active window(s) enumerated:"
        for ($i = 0; $i -lt $winCount; $i++) {
            $winBytes = Read-Exact $stream 60
            $surf = [System.BitConverter]::ToUInt32($winBytes, 0)
            $x = [System.BitConverter]::ToInt32($winBytes, 4)
            $y = [System.BitConverter]::ToInt32($winBytes, 8)
            $w = [System.BitConverter]::ToUInt32($winBytes, 12)
            $h = [System.BitConverter]::ToUInt32($winBytes, 16)
            $focused = [System.BitConverter]::ToUInt32($winBytes, 20)
            $titleLen = [System.BitConverter]::ToUInt32($winBytes, 24)
            $title = [System.Text.Encoding]::ASCII.GetString($winBytes, 28, [Math]::Min(32, [int]$titleLen)).TrimEnd("`0")
            Write-Output "  Window[$i]: surface=$surf pos=($x,$y) size=($w`x$h) focused=$focused title=`"$title`""
        }
    } elseif ($respType -eq $RESP_OK_ACTION) {
        $statusBytes = Read-Exact $stream 4
        $status = [System.BitConverter]::ToUInt32($statusBytes, 0)
        Write-Output "OK: Action completed successfully (status=$status)"
    } elseif ($respType -eq $RESP_DENIED) {
        $reasonBytes = Read-Exact $stream 4
        $reason = [System.BitConverter]::ToUInt32($reasonBytes, 0)
        Write-Output "DENIED: In-guest kernel capability check rejected request (reason=$reason)"
    } elseif ($respType -eq $RESP_ERR) {
        $codeBytes = Read-Exact $stream 4
        $code = [System.BitConverter]::ToUInt32($codeBytes, 0)
        Write-Output "ERR: Request failed with error code $code"
    } else {
        Write-Output ("UNKNOWN_RESPONSE: 0x{0:X8}" -f $respType)
    }

} finally {
    $client.Close()
}
