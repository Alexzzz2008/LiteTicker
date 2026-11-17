[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Candidate,
    [string]$Installer = (Join-Path $PSScriptRoot '..\..\install.ps1')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Join-Path ([IO.Path]::GetTempPath()) ('LiteTicker-shortcut-tests-' + [guid]::NewGuid().ToString('N'))
$repo = Join-Path $root 'package with space'
$desktop = Join-Path $root 'fake-desktop'
$install = Join-Path $root 'installed with space'
$fixtureProfile = Join-Path $root 'fake-profile'
foreach ($path in @($repo, $desktop, $install, (Join-Path $fixtureProfile 'LiteTicker'))) {
    [void](New-Item -ItemType Directory -Path $path -Force)
}
Copy-Item -LiteralPath $Installer -Destination (Join-Path $repo 'install.ps1')
Copy-Item -LiteralPath $Candidate -Destination (Join-Path $repo 'LiteTicker.exe')
$target = Join-Path $install 'LiteTicker.exe'
$newShortcut = Join-Path $desktop '轻量看盘.lnk'
$oldShortcuts = @('轻盘.lnk', '轻盘 · 桌面看盘助手.lnk') | ForEach-Object { Join-Path $desktop $_ }
$config = Join-Path $fixtureProfile 'LiteTicker\settings.json'
[IO.File]::WriteAllText($config, '{"Symbols":["600519.SH"],"OpacityPercent":83}')
$configHash = (Get-FileHash -LiteralPath $config).Hash
$script:checks = 0
function Check([bool]$Value, [string]$Message) {
    if (-not $Value) { throw $Message }
    $script:checks++
}

Add-Type -ReferencedAssemblies System.Web.Extensions -TypeDefinition @'
using System;
using System.IO;
using System.Collections.Generic;
using System.Web.Script.Serialization;
namespace LiteTicker.ShortcutFixture {
    public sealed class Folders {
        public string Path;
        public string Item(string name) {
            if(name != "Desktop") throw new Exception("Only the fixture desktop is allowed.");
            return Path;
        }
    }
    public sealed class Shortcut {
        public string TargetPath { get; set; }
        public string WorkingDirectory { get; set; }
        public string IconLocation { get; set; }
        public string Description { get; set; }
        public string Arguments { get; set; }
        public string Hotkey { get; set; }
        public int WindowStyle { get; set; }
        readonly string path;
        readonly bool failSave;
        public Shortcut(string path, bool failSave) {
            this.path = path;
            this.failSave = failSave;
            if(File.Exists(path)) {
                var previous = new JavaScriptSerializer().Deserialize<Dictionary<string, object>>(File.ReadAllText(path));
                TargetPath = Convert.ToString(previous["TargetPath"]);
                object value;
                if(previous.TryGetValue("Arguments", out value)) Arguments = Convert.ToString(value);
                if(previous.TryGetValue("Hotkey", out value)) Hotkey = Convert.ToString(value);
                if(previous.TryGetValue("WindowStyle", out value)) WindowStyle = Convert.ToInt32(value);
            }
        }
        public void Save() {
            if(failSave) throw new IOException("Owned shortcut save failure.");
            File.WriteAllText(path, new JavaScriptSerializer().Serialize(this));
        }
    }
    public sealed class Shell {
        public Folders SpecialFolders { get; set; }
        public string FailSavePath { get; set; }
        public object CreateShortcut(string path) {
            if(!String.Equals(System.IO.Path.GetDirectoryName(path), SpecialFolders.Path, StringComparison.OrdinalIgnoreCase))
                throw new Exception("A real desktop must never be touched.");
            return new Shortcut(path, String.Equals(path, FailSavePath, StringComparison.OrdinalIgnoreCase));
        }
    }
}
'@
$global:LiteTickerShortcutFixture = [LiteTicker.ShortcutFixture.Shell]::new()
$global:LiteTickerShortcutFixture.SpecialFolders = [LiteTicker.ShortcutFixture.Folders]::new()
$global:LiteTickerShortcutFixture.SpecialFolders.Path = $desktop
function New-Object {
    param([string]$ComObject)
    if ($ComObject -cne 'WScript.Shell') { throw 'Unexpected COM request.' }
    return $global:LiteTickerShortcutFixture
}
function Save-Link([string]$Path, [string]$Target) {
    $link = $global:LiteTickerShortcutFixture.CreateShortcut($Path)
    $link.TargetPath = $Target
    $link.Save()
}
$previousLocalAppData = $env:LOCALAPPDATA
try {
    $env:LOCALAPPDATA = $fixtureProfile
    Save-Link $oldShortcuts[0] $target.ToUpperInvariant()
    Save-Link $oldShortcuts[1] $target
    $custom = $global:LiteTickerShortcutFixture.CreateShortcut($oldShortcuts[0])
    $custom.Arguments = '--fixture "C:\owned path\quotes.json"'
    $custom.Hotkey = 'CTRL+ALT+J'
    $custom.WindowStyle = 7
    $custom.Save()
    & (Join-Path $repo 'install.ps1') -InstallDir $install
    Check (Test-Path -LiteralPath $newShortcut -PathType Leaf) 'The new Chinese shortcut name must be created.'
    $created = Get-Content -LiteralPath $newShortcut -Raw -Encoding UTF8 | ConvertFrom-Json
    Check ($created.TargetPath -ceq $target) 'The new shortcut must point at the isolated installation.'
    Check ($created.Description -ceq '桌面端股票轻量看盘工具') 'The new shortcut must contain the exact Chinese description.'
    Check ($created.Arguments -ceq $custom.Arguments -and $created.Hotkey -ceq $custom.Hotkey -and $created.WindowStyle -eq $custom.WindowStyle) 'Migrating a same-target legacy shortcut must preserve its arguments, hotkey and window style.'
    foreach ($old in $oldShortcuts) { Check (-not (Test-Path -LiteralPath $old)) 'Both same-target legacy names must be cleaned.' }

    foreach ($old in $oldShortcuts) { Save-Link $old (Join-Path $root 'different installation\LiteTicker.exe') }
    $unrelatedHashes = @($oldShortcuts | ForEach-Object { (Get-FileHash -LiteralPath $_).Hash })
    & (Join-Path $repo 'install.ps1') -InstallDir $install
    for ($i = 0; $i -lt $oldShortcuts.Count; $i++) {
        Check ((Get-FileHash -LiteralPath $oldShortcuts[$i]).Hash -ceq $unrelatedHashes[$i]) 'A legacy shortcut to another target must remain unchanged.'
    }

    Save-Link $oldShortcuts[0] $target
    & (Join-Path $repo 'install.ps1') -InstallDir $install
    Check (-not (Test-Path -LiteralPath $oldShortcuts[0])) 'A same-target legacy shortcut must be cleaned even if the new shortcut already exists.'
    Check ((Get-FileHash -LiteralPath $oldShortcuts[1]).Hash -ceq $unrelatedHashes[1]) 'Mixed legacy targets must be evaluated independently.'

    Save-Link $oldShortcuts[0] $target
    $legacyHash = (Get-FileHash -LiteralPath $oldShortcuts[0]).Hash
    Remove-Item -LiteralPath $newShortcut
    $global:LiteTickerShortcutFixture.FailSavePath = $newShortcut
    $failed = $false
    try { & (Join-Path $repo 'install.ps1') -InstallDir $install } catch { $failed = $true }
    Check $failed 'The owned shortcut save failure must reach the caller.'
    Check ((Get-FileHash -LiteralPath $oldShortcuts[0]).Hash -ceq $legacyHash) 'An old shortcut must survive if saving the new shortcut fails.'
    Check ((Get-FileHash -LiteralPath $config).Hash -ceq $configHash) 'The fixture user settings must remain unchanged.'
    [ordered]@{ Passed = $true; Checks = $script:checks; Directory = $root; NoProductGuiStarted = $true; NoRealDesktopTouched = $true } |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $root 'test-result.json') -Encoding UTF8
    Write-Output ('PASS shortcut migration checks=' + $script:checks + '; evidence=' + $root)
} finally {
    $env:LOCALAPPDATA = $previousLocalAppData
    Remove-Variable -Name LiteTickerShortcutFixture -Scope Global
}
