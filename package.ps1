[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Package LiteTicker on Windows after building it.' }
. (Join-Path $PSScriptRoot 'scripts\verify-native.ps1')
$version = Get-NativeVersion $PSScriptRoot
$dist = Join-Path $PSScriptRoot 'dist'
$executable = Join-Path $dist 'LiteTicker.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw 'dist\LiteTicker.exe was not found. Run build.ps1 first.' }
$verified = Test-NativeExecutable $executable $version
[IO.File]::WriteAllText((Join-Path $dist 'LiteTicker.exe.sha256'), ($verified.Sha256 + "  LiteTicker.exe`n"), [Text.UTF8Encoding]::new($false))
$archive = Join-Path $dist 'LiteTicker-win.zip'
$staging = Join-Path ([IO.Path]::GetTempPath()) ('LiteTicker-package-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $staging | Out-Null
try {
    foreach ($file in @($executable, (Join-Path $dist 'LiteTicker.exe.sha256'), (Join-Path $PSScriptRoot 'install.ps1'))) { Copy-Item -LiteralPath $file -Destination $staging }
    $command = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'install.cmd')).Replace("`r`n", "`n").Replace("`r", "`n").Replace("`n", "`r`n")
    [IO.File]::WriteAllText((Join-Path $staging 'install.cmd'), $command, [Text.UTF8Encoding]::new($false))
    $files = @('LiteTicker.exe', 'LiteTicker.exe.sha256', 'install.ps1', 'install.cmd') | ForEach-Object { Join-Path $staging $_ }
    Compress-Archive -LiteralPath $files -DestinationPath $archive -Force
} finally { Remove-Item -LiteralPath $staging -Recurse -Force }
$checksums = foreach ($name in @('LiteTicker.exe', 'LiteTicker-win.zip')) {
    $hash = (Get-FileHash -LiteralPath (Join-Path $dist $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText((Join-Path $dist ($name + '.sha256')), ($hash + '  ' + $name + "`n"), [Text.UTF8Encoding]::new($false))
    $hash + '  ' + $name
}
[IO.File]::WriteAllText((Join-Path $dist 'SHA256SUMS.txt'), (($checksums -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))
Write-Host ('Packaged native Windows x64 LiteTicker {0}: {1:N0} bytes.' -f $version, (Get-Item -LiteralPath $archive).Length)
