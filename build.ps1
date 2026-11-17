[CmdletBinding()]
param([switch]$Test)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Build on Windows with Rust 1.95+ and the MSVC C++/Windows SDK build tools. The finished app needs no .NET runtime.' }
. (Join-Path $PSScriptRoot 'scripts\verify-native.ps1')
$version = Get-NativeVersion $PSScriptRoot
$cargo = (Get-Command cargo.exe -ErrorAction Stop).Source
$native = Join-Path $PSScriptRoot 'native'
$versionInfo = [Diagnostics.ProcessStartInfo]::new()
$versionInfo.FileName = $cargo; $versionInfo.Arguments = '--version'; $versionInfo.WorkingDirectory = $native
$versionInfo.UseShellExecute = $false; $versionInfo.CreateNoWindow = $true; $versionInfo.RedirectStandardOutput = $true
$versionProcess = [Diagnostics.Process]::Start($versionInfo)
try {
    $cargoVersion = $versionProcess.StandardOutput.ReadToEnd(); $versionProcess.WaitForExit()
    if ($versionProcess.ExitCode -ne 0 -or $cargoVersion -notmatch '^cargo ([0-9]+\.[0-9]+\.[0-9]+)' -or [version]$Matches[1] -lt [version]'1.95.0') { throw 'Cargo/Rust 1.95 or newer is required; install the pinned Rust 1.96.0 MSVC toolchain.' }
} finally { $versionProcess.Dispose() }
$target = 'x86_64-pc-windows-msvc'
$targetDirectory = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $native 'target' }

function Invoke-Cargo {
    param([string[]]$Arguments)
    $quoted = foreach ($value in $Arguments) {
        $escaped = [regex]::Replace($value, '(\\*)"', '$1$1\"')
        $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
        '"' + $escaped + '"'
    }
    $process = Start-Process -FilePath $cargo -ArgumentList ($quoted -join ' ') -WorkingDirectory $native -NoNewWindow -Wait -PassThru
    try { if ($process.ExitCode -ne 0) { throw "Cargo failed with exit code $($process.ExitCode)." } }
    finally { $process.Dispose() }
}

$previousFlags = $env:RUSTFLAGS
$previousEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
try {
    $env:RUSTFLAGS = ($previousFlags + ' -C target-feature=+crt-static').Trim()
    if ($previousEncodedFlags) { $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags + [char]31 + '-C' + [char]31 + 'target-feature=+crt-static' }
    Invoke-Cargo -Arguments @('--version')
    Invoke-Cargo -Arguments @('build', '--locked', '--release', '--target', $target, '--target-dir', $targetDirectory, '--bin', 'LiteTicker')
    if ($Test) { Invoke-Cargo -Arguments @('test', '--locked', '--release', '--target', $target, '--target-dir', $targetDirectory, '--all-targets') }
} finally {
    $env:RUSTFLAGS = $previousFlags
    $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags
}
$executable = Join-Path $targetDirectory ($target + '\release\LiteTicker.exe')
$verified = Test-NativeExecutable $executable $version
$dist = Join-Path $PSScriptRoot 'dist'
New-Item -ItemType Directory -Path $dist -Force | Out-Null
Copy-Item -LiteralPath $executable -Destination (Join-Path $dist 'LiteTicker.exe') -Force
[IO.File]::WriteAllText((Join-Path $dist 'LiteTicker.exe.sha256'), ($verified.Sha256 + "  LiteTicker.exe`n"), [Text.UTF8Encoding]::new($false))
$verified | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $dist 'native-build.json') -Encoding UTF8
Write-Host ('Built native Windows x64 LiteTicker {0}: {1:N0} bytes; static CRT; no CLR.' -f $version, $verified.Bytes)
