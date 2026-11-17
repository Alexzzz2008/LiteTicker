[CmdletBinding()]
param(
    [string]$InstallDir,
    [switch]$Start
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'LiteTicker requires Windows.' }

function Test-DotNetFramework {
    $framework = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full' -Name Release -ErrorAction SilentlyContinue
    return $null -ne $framework -and $framework.Release -ge 528040
}

function Release-ComObject {
    param([object]$Value)
    if ($null -ne $Value -and [System.Runtime.InteropServices.Marshal]::IsComObject($Value)) {
        [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($Value)
    }
}

function Export-ShortcutIcon {
    param([string]$Executable)
    # Load bytes so reading embedded resources cannot keep the installed EXE locked.
    $assembly = [Reflection.Assembly]::Load([IO.File]::ReadAllBytes($Executable))
    $resource = $assembly.GetManifestResourceStream('LiteTicker.Icon.ico')
    if ($null -eq $resource) { throw 'LiteTicker.exe 缺少内嵌图标，请重新构建或下载完整安装包。' }
    $memory = [IO.MemoryStream]::new()
    try { $resource.CopyTo($memory); $bytes = $memory.ToArray() }
    finally { $memory.Dispose(); $resource.Dispose() }
    if ($bytes.Length -eq 0) { throw 'LiteTicker.exe 的内嵌图标为空。' }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $digest = [BitConverter]::ToString($sha.ComputeHash($bytes)).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
    $icon = Join-Path ([IO.Path]::GetDirectoryName($Executable)) ('LiteTicker-icon-' + $digest + '.ico')
    if ([IO.File]::Exists($icon) -and (Get-FileHash -LiteralPath $icon -Algorithm SHA256).Hash -eq $digest) { return $icon }
    $temporaryIcon = $icon + '.' + [Guid]::NewGuid().ToString('N') + '.tmp'
    try {
        [IO.File]::WriteAllBytes($temporaryIcon, $bytes)
        if ((Get-FileHash -LiteralPath $temporaryIcon -Algorithm SHA256).Hash -ne $digest) { throw '桌面图标复制校验失败。' }
        if ([IO.File]::Exists($icon)) { [IO.File]::Replace($temporaryIcon, $icon, [NullString]::Value) }
        else { [IO.File]::Move($temporaryIcon, $icon) }
    } finally { if ([IO.File]::Exists($temporaryIcon)) { [IO.File]::Delete($temporaryIcon) } }
    return $icon
}

function Update-ShellPath {
    param([string]$Path)
    if (-not ('LiteTicker.InstallerShell' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace LiteTicker {
    public static class InstallerShell {
        [DllImport("shell32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        public static extern void SHChangeNotify(uint eventId, uint flags, string path, IntPtr unused);
    }
}
'@
    }
    # UPDATEITEM + PATHW + FLUSHNOWAIT: refresh only this path without blocking Explorer.
    [LiteTicker.InstallerShell]::SHChangeNotify(0x2000, 0x3005, $Path, [IntPtr]::Zero)
}

if (-not (Test-DotNetFramework)) {
    $downloadUrl = 'https://dotnet.microsoft.com/zh-cn/download/dotnet-framework/net48'
    Write-Host '未检测到 .NET Framework 4.8 或更新版本，暂时无法安装轻盘。' -ForegroundColor Yellow
    Write-Host ('请从微软官网下载并安装运行库，然后重新运行 install.cmd：' + $downloadUrl)
    $answer = ''
    try { $answer = Read-Host '是否打开微软官方下载页面？输入 Y 打开，其他输入取消' } catch { }
    if ($answer -match '^[Yy]$') {
        try { Start-Process -FilePath $downloadUrl } catch { Write-Warning '无法自动打开浏览器，请复制上方地址手动打开。' }
    }
    throw '安装已停止。安装 .NET Framework 4.8 或更新版本后，请重新运行安装程序。'
}

$source = Join-Path $PSScriptRoot 'dist\LiteTicker.exe'
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { $source = Join-Path $PSScriptRoot 'LiteTicker.exe' }
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw '未找到 LiteTicker.exe。请先完整解压安装包；源码用户请先运行 build.ps1。' }
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = if (Test-Path -LiteralPath 'D:\LiteTicker\LiteTicker.exe' -PathType Leaf) { 'D:\LiteTicker' }
        else { Join-Path $env:LOCALAPPDATA 'Programs\LiteTicker' }
}
$InstallDir = [System.IO.Path]::GetFullPath($InstallDir)
$target = Join-Path $InstallDir 'LiteTicker.exe'
$running = @(Get-Process -Name LiteTicker -ErrorAction SilentlyContinue | Where-Object { $_.Path -and [string]::Equals($_.Path, $target, [System.StringComparison]::OrdinalIgnoreCase) })
if ($running.Count -gt 0) { throw '轻盘仍在运行。请在托盘菜单中选择“退出轻盘”（点击窗口 × 只会隐藏），再重新运行 install.cmd。' }

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
if (-not [string]::Equals([System.IO.Path]::GetFullPath($source), $target, [System.StringComparison]::OrdinalIgnoreCase)) {
    $temporary = Join-Path $InstallDir ('LiteTicker.' + [Guid]::NewGuid().ToString('N') + '.tmp')
    try {
        Copy-Item -LiteralPath $source -Destination $temporary
        if ((Get-FileHash -LiteralPath $source).Hash -ne (Get-FileHash -LiteralPath $temporary).Hash) { throw 'The executable copy did not pass verification.' }
        if (Test-Path -LiteralPath $target -PathType Leaf) {
            $backups = Join-Path $InstallDir 'backups'
            New-Item -ItemType Directory -Path $backups -Force | Out-Null
            $backup = Join-Path $backups ('LiteTicker-' + [DateTime]::Now.ToString('yyyyMMdd-HHmmss-fffffff') + '.exe')
            [System.IO.File]::Replace($temporary, $target, $backup)
        } else { [System.IO.File]::Move($temporary, $target) }
    } finally { if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary } }
}

$iconPath = Export-ShortcutIcon $target
$shell = New-Object -ComObject WScript.Shell
$shortcut = $null
try {
    $desktop = $shell.SpecialFolders.Item('Desktop')
    $shortcutPath = Join-Path $desktop '轻盘 · 桌面看盘助手.lnk'
    $legacyShortcutPath = Join-Path $desktop '轻盘.lnk'
    if ((Test-Path -LiteralPath $legacyShortcutPath -PathType Leaf) -and -not (Test-Path -LiteralPath $shortcutPath)) {
        $legacyShortcut = $shell.CreateShortcut($legacyShortcutPath)
        try {
            if ([string]::Equals($legacyShortcut.TargetPath, $target, [System.StringComparison]::OrdinalIgnoreCase)) {
                Move-Item -LiteralPath $legacyShortcutPath -Destination $shortcutPath
            }
        } finally { Release-ComObject $legacyShortcut }
    }
    if (Test-Path -LiteralPath $shortcutPath -PathType Leaf) {
        $shortcutBackup = $shortcutPath + '.' + [DateTime]::Now.ToString('yyyyMMdd-HHmmss-fffffff') + '.bak'
        Copy-Item -LiteralPath $shortcutPath -Destination $shortcutBackup
    }
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $target
    $shortcut.WorkingDirectory = $InstallDir
    $shortcut.IconLocation = $iconPath + ',0'
    $shortcut.Description = '轻盘 · 桌面看盘助手：股票行情、自选股与桌面行情小窗'
    $shortcut.Save()
} finally {
    Release-ComObject $shortcut
    Release-ComObject $shell
}

try { Update-ShellPath $iconPath; Update-ShellPath $shortcutPath }
catch { Write-Warning ('已保存新图标及快捷方式，但通知桌面刷新失败：' + $_.Exception.Message) }

Write-Host "Installed: $target"
Write-Host "Desktop shortcut: $shortcutPath"
Write-Host 'User settings in %LOCALAPPDATA%\LiteTicker were preserved.'
if ($Start) {
    Start-Process -FilePath $target -WorkingDirectory $InstallDir
    Write-Host 'LiteTicker started.'
} else { Write-Host 'The app was not started.' }
