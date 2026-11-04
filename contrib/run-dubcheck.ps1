param(
    [Parameter(Mandatory)][string]$Library,
    [string]$Exe = (Join-Path $PSScriptRoot 'dubcheck.exe'),
    [string]$OutDir = $PSScriptRoot,
    [string]$Ffprobe = '',
    [int]$MinHours = 6,
    [int]$Jobs = 2
)
# Optional wrapper for running dubcheck unattended (startup script, scheduled task, ...).
# Usage: powershell -File run-dubcheck.ps1 -Library <path to library>
# Read-only on the library.
# Never throws, never blocks Media ON, always exits 0. Output: latest.txt, latest.json, dubcheck.log.

$ErrorActionPreference = 'Stop'
$log = Join-Path $OutDir 'dubcheck.log'
$lock = Join-Path $OutDir 'dubcheck.lock'
$stamp = Join-Path $OutDir 'last-run.txt'
$haveLock = $false

function Write-Log($msg) {
    try { Add-Content -Path $log -Value ("{0}  {1}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $msg) } catch { }
}

try {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

    # keep the log from growing forever: roll over at 256 KB
    if ((Test-Path $log) -and (Get-Item $log).Length -gt 256KB) {
        Move-Item -Force $log "$log.old"
    }

    if (-not (Test-Path $Exe)) { Write-Log "skip: dubcheck.exe not found at $Exe"; exit 0 }
    if (-not (Test-Path $Library)) { Write-Log "skip: library not found at $Library"; exit 0 }

    $ffArgs = @()
    if ($Ffprobe) {
        if (-not (Test-Path $Ffprobe)) { Write-Log "skip: ffprobe not found at $Ffprobe"; exit 0 }
        $ffArgs = @('--ffprobe', $Ffprobe)
    } elseif (-not (Get-Command ffprobe -ErrorAction SilentlyContinue)) {
        Write-Log 'skip: ffprobe not on PATH'; exit 0
    }

    # recent enough already?
    if (Test-Path $stamp) {
        $age = (Get-Date) - (Get-Item $stamp).LastWriteTime
        if ($age.TotalHours -lt $MinHours) { Write-Log ("skip: last run {0:N1}h ago" -f $age.TotalHours); exit 0 }
    }

    # already running? (lock holds the PID; a lock whose process is gone, or older than 12h, is stale)
    if (Test-Path $lock) {
        $lockAge = (Get-Date) - (Get-Item $lock).LastWriteTime
        $lockPid = 0
        [void][int]::TryParse(((Get-Content $lock -ErrorAction SilentlyContinue) -join '').Trim(), [ref]$lockPid)
        $alive = $lockPid -gt 0 -and (Get-Process -Id $lockPid -ErrorAction SilentlyContinue)
        if ($alive -and $lockAge.TotalHours -lt 12) { Write-Log "skip: already running (pid $lockPid)"; exit 0 }
        Write-Log 'removing stale lock'
        Remove-Item $lock -Force -ErrorAction SilentlyContinue
    }
    Set-Content -Path $lock -Value $PID
    $haveLock = $true

    # be gentle: the library is on a spinning disk and qBittorrent is about to use it
    (Get-Process -Id $PID).PriorityClass = 'BelowNormal'

    Write-Log "start: scanning $Library (jobs $Jobs)"
    $cache = Join-Path $OutDir 'cache.json'
    $common = (@($Library, '--jobs', $Jobs, '--cache-file', $cache) + $ffArgs) | ForEach-Object { if ("$_" -match '\s') { '"' + $_ + '"' } else { "$_" } }
    $txtTmp = Join-Path $OutDir 'latest.txt.tmp'
    $jsonTmp = Join-Path $OutDir 'latest.json.tmp'
    $errTmp = Join-Path $OutDir 'latest.err.tmp'

    # text report first (does the slow probing and fills the cache), then JSON from the warm cache
    $env:NO_COLOR = '1'
    $p1 = Start-Process -FilePath $Exe -ArgumentList $common -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $txtTmp -RedirectStandardError $errTmp
    # exit codes 0/2/4/6 mean the scan worked (2 and 4 are bits for "problems found"); anything else is a failure
    if ($p1.ExitCode -notin 0, 2, 4, 6) {
        $why = (Get-Content $errTmp -ErrorAction SilentlyContinue | Select-Object -First 3) -join ' | '
        Write-Log "fail: exit $($p1.ExitCode) $why"
        exit 0
    }
    $p2 = Start-Process -FilePath $Exe -ArgumentList ($common + '--json') -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $jsonTmp -RedirectStandardError $errTmp
    if ($p2.ExitCode -notin 0, 2, 4, 6) { Write-Log "fail: json exit $($p2.ExitCode)"; exit 0 }

    Move-Item -Force $txtTmp (Join-Path $OutDir 'latest.txt')
    Move-Item -Force $jsonTmp (Join-Path $OutDir 'latest.json')
    Remove-Item $errTmp -Force -ErrorAction SilentlyContinue
    Set-Content -Path $stamp -Value (Get-Date -Format 'o')
    Write-Log "done: exit code $($p1.ExitCode) (0 clean, +2 structural, +4 no English)"
} catch {
    Write-Log "error: $($_.Exception.Message)"
} finally {
    if ($haveLock) { Remove-Item $lock -Force -ErrorAction SilentlyContinue }
}
exit 0
