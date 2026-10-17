$ErrorActionPreference = 'Stop'

$repoRoot = $PSScriptRoot
$packageScript = Join-Path $repoRoot 'scripts\package_windows_conpty.py'
$tempRoot = [System.IO.Path]::GetTempPath()
$zigHome = Join-Path $tempRoot 'herdr-zig-0.16.0'
$zigExe = Join-Path $zigHome 'zig-x86_64-windows-0.16.0\zig.exe'
$zigUri = 'https://ziglang.org/download/0.16.0/zig-x86_64-windows-0.16.0.zip'
$zigSha256 = '68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e'

try {
    if (-not (Test-Path -LiteralPath $packageScript -PathType Leaf)) {
        throw "Herdr Windows packaging script was not found: $packageScript"
    }
    if (-not (Get-Command python -ErrorAction SilentlyContinue)) {
        throw 'Python 3 is required. Install Python and make sure python is on PATH.'
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'Rust Cargo is required. Install the Rust toolchain and make sure cargo is on PATH.'
    }

    $zigCandidates = [System.Collections.Generic.List[string]]::new()
    if ($env:ZIG) { $zigCandidates.Add($env:ZIG) }
    $zigOnPath = Get-Command zig.exe -ErrorAction SilentlyContinue
    if ($zigOnPath -and $zigOnPath.Source) { $zigCandidates.Add($zigOnPath.Source) }
    $zigCandidates.Add($zigExe)

    $usableZig = $null
    foreach ($candidate in ($zigCandidates | Select-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { continue }
        try {
            $candidateVersion = (& $candidate version 2>$null | Out-String).Trim()
            if ($LASTEXITCODE -eq 0 -and $candidateVersion -eq '0.16.0') {
                $usableZig = $candidate
                break
            }
        } catch {
            # Ignore unusable candidates and bootstrap the pinned tool below.
        }
    }

    if (-not $usableZig) {
        Write-Host 'Downloading the pinned Zig 0.16.0 Windows x64 toolchain...'
        $zigArchive = Join-Path $tempRoot "herdr-zig-0.16.0-$PID.zip"
        try {
            Invoke-WebRequest -Uri $zigUri -OutFile $zigArchive
            $actualHash = (Get-FileHash -LiteralPath $zigArchive -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actualHash -ne $zigSha256) {
                throw "Zig archive SHA-256 mismatch: expected $zigSha256, got $actualHash"
            }
            New-Item -ItemType Directory -Path $zigHome -Force | Out-Null
            Expand-Archive -LiteralPath $zigArchive -DestinationPath $zigHome -Force
        } finally {
            if (Test-Path -LiteralPath $zigArchive) {
                Remove-Item -LiteralPath $zigArchive -Force
            }
        }

        if (-not (Test-Path -LiteralPath $zigExe -PathType Leaf)) {
            throw "zig.exe was not found after extracting the verified archive: $zigExe"
        }
        $usableZig = $zigExe
    }

    Write-Host "Using Zig $((& $usableZig version).Trim())"
    $oldZig = $env:ZIG
    try {
        $env:ZIG = $usableZig
        & python $packageScript build-local
        if ($LASTEXITCODE -ne 0) {
            throw "Herdr build failed with exit code $LASTEXITCODE"
        }
    } finally {
        if ($null -eq $oldZig) {
            Remove-Item Env:ZIG -ErrorAction SilentlyContinue
        } else {
            $env:ZIG = $oldZig
        }
    }

    $targetRoot = Join-Path $repoRoot 'target'
    if ($env:CARGO_TARGET_DIR) {
        $targetRoot = if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
            $env:CARGO_TARGET_DIR
        } else {
            Join-Path $repoRoot $env:CARGO_TARGET_DIR
        }
    }
    $releaseDir = Join-Path $targetRoot 'release'
    $exePath = Join-Path $releaseDir 'herdr.exe'
    $conptyPath = Join-Path $releaseDir 'conpty\conpty.dll'
    if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) {
        throw "Build finished without the expected executable: $exePath"
    }
    if (-not (Test-Path -LiteralPath $conptyPath -PathType Leaf)) {
        throw "Build finished without the required ConPTY runtime: $conptyPath"
    }

    $version = (& $exePath --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "The generated executable could not be started: $exePath"
    }
    Write-Host ''
    Write-Host "Build complete: $exePath"
    Write-Host "Version check: $version"
    Write-Host 'Keep the conpty folder beside herdr.exe when copying the build.'
} catch {
    Write-Host "Build failed: $_" -ForegroundColor Red
    exit 1
}
