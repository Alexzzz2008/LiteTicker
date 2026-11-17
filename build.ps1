[CmdletBinding()]
param(
    [switch]$Test
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Invoke-NativeProcess {
    param([string]$FilePath, [string[]]$Arguments)

    # Start-Process joins ArgumentList; quote each Windows argv entry explicitly.
    $quoted = foreach ($value in $Arguments) {
        $escaped = [regex]::Replace($value, '(\\*)"', '$1$1\"')
        $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
        '"' + $escaped + '"'
    }
    $parameters = @{
        FilePath = $FilePath
        NoNewWindow = $true
        Wait = $true
        PassThru = $true
    }
    if ($quoted) { $parameters.ArgumentList = $quoted -join ' ' }
    $process = Start-Process @parameters
    try {
        if ($process.ExitCode -ne 0) { throw "Process failed ($($process.ExitCode)): $FilePath" }
    } finally { $process.Dispose() }
}

if ($env:OS -ne 'Windows_NT') { throw 'Build LiteTicker on Windows with .NET Framework 4.8.' }
$release = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full' -ErrorAction Stop).Release
if ($release -lt 528040) { throw '.NET Framework 4.8 or newer is required.' }

$framework = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319'
if (-not (Test-Path -LiteralPath (Join-Path $framework 'csc.exe'))) {
    $framework = Join-Path $env:WINDIR 'Microsoft.NET\Framework\v4.0.30319'
}
$compiler = Join-Path $framework 'csc.exe'
if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) { throw 'The Windows .NET Framework C# compiler was not found.' }

$sources = @(Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'src') -Filter '*.cs' -File | Sort-Object Name | ForEach-Object FullName)
if ($sources.Count -eq 0) { throw 'No C# source files found in src.' }
$dist = Join-Path $PSScriptRoot 'dist'
New-Item -ItemType Directory -Path $dist -Force | Out-Null
$icon = Join-Path $PSScriptRoot 'assets\liteticker.ico'
if (-not (Test-Path -LiteralPath $icon -PathType Leaf)) { & (Join-Path $PSScriptRoot 'scripts\new-icon.ps1') }

$common = @('/nologo', '/optimize+', '/platform:anycpu', '/codepage:65001', '/utf8output', '/warn:4', ('/resource:' + $icon + ',LiteTicker.Icon.ico'))
foreach ($assembly in @('System.dll', 'System.Core.dll', 'System.Drawing.dll', 'System.Windows.Forms.dll', 'System.Net.Http.dll', 'System.Web.Extensions.dll')) {
    $common += '/reference:' + (Join-Path $framework $assembly)
}

$application = Join-Path $dist 'LiteTicker.exe'
Invoke-NativeProcess -FilePath $compiler -Arguments ($common + @('/target:winexe', '/main:LiteTicker.Program', ('/win32icon:' + $icon), ('/out:' + $application)) + $sources)
Write-Host ('Built: {0} ({1:N0} bytes)' -f $application, (Get-Item -LiteralPath $application).Length)

if ($Test) {
    $tests = @(Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'tests') -Filter '*.cs' -File | Sort-Object Name | ForEach-Object FullName)
    if ($tests.Count -eq 0) { throw 'No C# tests found in tests.' }
    $testExecutable = Join-Path $dist 'LiteTicker.Tests.exe'
    Invoke-NativeProcess -FilePath $compiler -Arguments ($common + @('/target:exe', '/main:LiteTicker.Tests.TestProgram', ('/out:' + $testExecutable)) + $sources + $tests)
    Invoke-NativeProcess -FilePath $testExecutable -Arguments @()
}
