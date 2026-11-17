[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Package LiteTicker on Windows after building it.' }

$dist = Join-Path $PSScriptRoot 'dist'
$executable = Join-Path $dist 'LiteTicker.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw 'dist\LiteTicker.exe was not found. Run build.ps1 first.' }
$source = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'src\AppVersion.cs') -Raw
$match = [regex]::Match($source, 'public\s+const\s+string\s+Current\s*=\s*"(?<version>[0-9]+\.[0-9]+\.[0-9]+)"\s*;')
if (-not $match.Success) { throw 'AppVersion.Current must contain a stable three-part version.' }
$version = $match.Groups['version'].Value
$binary = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable)
$assembly = [Reflection.AssemblyName]::GetAssemblyName($executable)
if ($binary.ProductVersion -cne $version -or $assembly.Name -cne 'LiteTicker' -or $assembly.Version -ne [version]($version + '.0')) {
    throw 'The compiled executable does not match AppVersion.Current. Rebuild before packaging.'
}
$files = @($executable, (Join-Path $PSScriptRoot 'install.ps1'), (Join-Path $PSScriptRoot 'install.cmd'))
foreach ($file in $files) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw ('Package file was not found: ' + $file) }
}
$archive = Join-Path $dist 'LiteTicker-win.zip'
$staging = Join-Path ([IO.Path]::GetTempPath()) ('LiteTicker-package-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $staging | Out-Null
try {
    Copy-Item -LiteralPath $files[0..1] -Destination $staging
    # Keep the Windows entry point reliable regardless of Git's checkout line endings.
    $command = [IO.File]::ReadAllText($files[2]).Replace("`r`n", "`n").Replace("`r", "`n").Replace("`n", "`r`n")
    [IO.File]::WriteAllText((Join-Path $staging 'install.cmd'), $command, [Text.UTF8Encoding]::new($false))
    $packageFiles = @('LiteTicker.exe', 'install.ps1', 'install.cmd') | ForEach-Object { Join-Path $staging $_ }
    Compress-Archive -LiteralPath $packageFiles -DestinationPath $archive -Force
} finally { Remove-Item -LiteralPath $staging -Recurse -Force }
Write-Host ('Packaged: {0} ({1:N0} bytes), version {2}' -f $archive, (Get-Item -LiteralPath $archive).Length, $version)
