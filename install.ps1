[CmdletBinding()]
param(
    [string]$InstallDir,
    [switch]$Start
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitOperatingSystem) { throw '轻盘需要 Windows x64。原生程序无需 .NET 或 VC++ 运行库安装。' }

function Release-ComObject {
    param([object]$Value)
    if ($null -ne $Value -and [System.Runtime.InteropServices.Marshal]::IsComObject($Value)) {
        [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($Value)
    }
}

function Test-NativeSource {
    param([string]$Path)
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 512 -or $bytes.Length -gt 20MB -or [BitConverter]::ToUInt16($bytes, 0) -ne 0x5a4d) { throw '安装文件不是有效的原生轻盘程序。' }
    $pe = [BitConverter]::ToInt32($bytes, 0x3c)
    if ($pe -lt 64 -or $pe -gt $bytes.Length - 264 -or [BitConverter]::ToUInt32($bytes, $pe) -ne 0x4550 -or
        [BitConverter]::ToUInt16($bytes, $pe + 4) -ne 0x8664 -or [BitConverter]::ToUInt16($bytes, $pe + 24) -ne 0x20b -or
        [BitConverter]::ToUInt64($bytes, $pe + 24 + 112 + 14 * 8) -ne 0) { throw '仅可安装 Windows x64 原生轻盘，不能使用旧版 .NET 或其他架构程序。' }
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path)
    if ($info.ProductName -cne 'LiteTicker' -or $info.OriginalFilename -cne 'LiteTicker.exe' -or $info.ProductVersion -notmatch '^[1-9][0-9]*\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)$' -or
        ([version]$info.ProductVersion).Major -lt 2 -or $info.FileVersion -cne ($info.ProductVersion + '.0')) { throw '程序身份或正式版本号不正确，请重新下载官方安装包。' }
    $expected = [version]($info.ProductVersion + '.0')
    if ($info.FileMajorPart -ne $expected.Major -or $info.FileMinorPart -ne $expected.Minor -or $info.FileBuildPart -ne $expected.Build -or $info.FilePrivatePart -ne 0 -or $info.ProductMajorPart -ne $expected.Major -or $info.ProductMinorPart -ne $expected.Minor -or $info.ProductBuildPart -ne $expected.Build -or $info.ProductPrivatePart -ne 0) { throw '程序固定版本信息不匹配。' }
}

function Get-OfficialFile {
    param([uri]$Uri, [string]$Path, [long]$Limit)
    $original = $Uri.AbsoluteUri
    for ($redirect = 0; $redirect -le 5; $redirect++) {
        if ($Uri.Scheme -cne 'https' -or -not $Uri.IsDefaultPort -or $Uri.UserInfo -or $Uri.Fragment -or
            ($Uri.Host -cne 'github.com' -and $Uri.Host -cne 'release-assets.githubusercontent.com' -and $Uri.Host -cne 'objects.githubusercontent.com' -and $Uri.Host -cne 'github-releases.githubusercontent.com') -or
            ($Uri.Host -ceq 'github.com' -and $Uri.AbsoluteUri -cne $original)) { throw '官方下载跳转地址无效，安装已停止。' }
        $request = [Net.HttpWebRequest]::CreateHttp($Uri)
        $request.AllowAutoRedirect = $false; $request.Timeout = 20000; $request.ReadWriteTimeout = 20000
        $request.UserAgent = 'LiteTicker-Installer'; $request.CookieContainer = $null
        $response = $request.GetResponse()
        try {
            $status = [int]$response.StatusCode
            if ($status -ge 300 -and $status -lt 400) {
                if ($redirect -eq 5 -or -not $response.Headers['Location']) { throw '官方下载跳转过多或缺少地址。' }
                $Uri = [uri]::new($Uri, $response.Headers['Location']); continue
            }
            if ($status -ne 200 -or $response.ContentLength -gt $Limit) { throw '官方下载状态或文件大小无效。' }
            $input = $response.GetResponseStream(); $output = [IO.File]::Create($Path)
            try {
                $buffer = [byte[]]::new(81920); $total = 0L
                while (($count = $input.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    $total += $count; if ($total -gt $Limit) { throw '官方下载文件超出大小限制。' }
                    $output.Write($buffer, 0, $count)
                }
                if ($total -eq 0) { throw '官方下载文件为空。' }
            } finally { $input.Dispose(); $output.Dispose() }
            return
        } finally { $response.Dispose() }
    }
}

function Get-PublicRelease {
    param([string]$Directory)
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $metadata = Invoke-RestMethod -Uri 'https://api.github.com/repos/Alexzzz2008/LiteTicker/releases/latest' -Headers @{ 'User-Agent' = 'LiteTicker-Installer'; 'Accept' = 'application/vnd.github+json' } -TimeoutSec 20 -MaximumRedirection 0
    $tag = [string]$metadata.tag_name
    if ($metadata.draft -ne $false -or $metadata.prerelease -ne $false -or $tag -notmatch '^v([1-9][0-9]*\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))$' -or ([version]$tag.Substring(1)).Major -lt 2 -or
        $metadata.html_url -cne ('https://github.com/Alexzzz2008/LiteTicker/releases/tag/' + $tag)) { throw '当前官方发布尚无可核验的原生正式版，请从仓库 Releases 下载 v2.0.0 或更新版本。' }
    $url = 'https://github.com/Alexzzz2008/LiteTicker/releases/download/' + $tag + '/LiteTicker.exe'
    $assets = @($metadata.assets | Where-Object { $_.name -ceq 'LiteTicker.exe' })
    if ($assets.Count -ne 1 -or $assets[0].state -cne 'uploaded' -or $assets[0].browser_download_url -cne $url -or $assets[0].size -le 0 -or $assets[0].size -gt 20MB) { throw '官方轻盘资产名称、地址或大小无效。' }
    $digest = if ($assets[0].PSObject.Properties['digest']) { [string]$assets[0].digest } else { '' }
    if ($digest -cmatch '^sha256:([0-9a-fA-F]{64})$') { $hash = $Matches[1].ToLowerInvariant() }
    elseif ($digest) { throw '官方程序摘要格式无效，安装已停止。' }
    else {
        $sidecars = @($metadata.assets | Where-Object { $_.name -ceq 'LiteTicker.exe.sha256' })
        if ($sidecars.Count -ne 1 -or $sidecars[0].state -cne 'uploaded' -or $sidecars[0].browser_download_url -cne ($url + '.sha256')) { throw '官方发布缺少可核验的程序摘要。' }
        $checksum = Join-Path $Directory 'LiteTicker.exe.sha256'
        Get-OfficialFile ([uri]($url + '.sha256')) $checksum 256
        $text = [IO.File]::ReadAllText($checksum)
        if ($text -cnotmatch '\A([0-9a-fA-F]{64})  LiteTicker\.exe\r?\n?\z') { throw '官方程序摘要格式无效。' }
        $hash = $Matches[1].ToLowerInvariant()
    }
    $path = Join-Path $Directory 'LiteTicker.exe'
    Get-OfficialFile ([uri]$url) $path 20MB
    if ((Get-Item -LiteralPath $path).Length -ne $assets[0].size -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $hash) { throw '官方下载的程序大小或 SHA256 不匹配，安装已停止。' }
    Test-NativeSource $path
    if ([Diagnostics.FileVersionInfo]::GetVersionInfo($path).ProductVersion -cne $tag.Substring(1)) { throw '下载程序的版本与官方标签不匹配。' }
    return $path
}

function Get-NativeIconBytes {
    param([string]$Executable)
    if (-not ('LiteTicker.InstallerIcon' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
namespace LiteTicker {
    public static class InstallerIcon {
        [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr LoadLibraryEx(string path, IntPtr unused, uint flags);
        [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr FindResource(IntPtr module, IntPtr name, IntPtr type);
        [DllImport("kernel32.dll")] static extern uint SizeofResource(IntPtr module, IntPtr resource);
        [DllImport("kernel32.dll")] static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
        [DllImport("kernel32.dll")] static extern IntPtr LockResource(IntPtr resource);
        [DllImport("kernel32.dll")] static extern bool FreeLibrary(IntPtr module);
        static byte[] Read(IntPtr module, int type, int id) {
            IntPtr resource=FindResource(module,new IntPtr(id),new IntPtr(type));
            uint size=SizeofResource(module,resource);
            if(resource==IntPtr.Zero || size==0 || size>4*1024*1024) throw new InvalidDataException("Native icon resource is missing or invalid.");
            var bytes=new byte[(int)size]; Marshal.Copy(LockResource(LoadResource(module,resource)),bytes,0,bytes.Length); return bytes;
        }
        public static byte[] ReadIcon(string path) {
            IntPtr module=LoadLibraryEx(path,IntPtr.Zero,0x22);
            if(module==IntPtr.Zero) throw new IOException("Cannot read the native icon resource.");
            try {
                byte[] group=Read(module,14,1);
                if(group.Length<6) throw new InvalidDataException("Invalid native icon header.");
                int count=BitConverter.ToUInt16(group,4);
                if(count<1 || count>32 || group.Length!=6+count*14) throw new InvalidDataException("Invalid native icon group.");
                var images=new byte[count][];
                using(var output=new MemoryStream()) using(var writer=new BinaryWriter(output)) {
                    writer.Write((ushort)0); writer.Write((ushort)1); writer.Write((ushort)count);
                    uint offset=(uint)(6+count*16);
                    for(int i=0;i<count;i++) {
                        int entry=6+i*14; images[i]=Read(module,3,BitConverter.ToUInt16(group,entry+12));
                        writer.Write(group,entry,8); writer.Write((uint)images[i].Length); writer.Write(offset); offset+=(uint)images[i].Length;
                    }
                    foreach(var image in images) writer.Write(image);
                    return output.ToArray();
                }
            } finally { FreeLibrary(module); }
        }
    }
}
'@
    }
    return ,([LiteTicker.InstallerIcon]::ReadIcon($Executable))
}

function Export-ShortcutIcon {
    param([string]$Executable, [byte[]]$Bytes)
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

$source = Join-Path $PSScriptRoot 'dist\LiteTicker.exe'
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { $source = Join-Path $PSScriptRoot 'LiteTicker.exe' }
$downloadDirectory = $null
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    $downloadDirectory = Join-Path ([IO.Path]::GetTempPath()) ('LiteTicker-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $downloadDirectory | Out-Null
}
try {
if ($downloadDirectory) { $source = Get-PublicRelease $downloadDirectory }
Test-NativeSource $source
$sidecar = $source + '.sha256'
if (Test-Path -LiteralPath $sidecar -PathType Leaf) {
    $checksum = [IO.File]::ReadAllText($sidecar)
    if ($checksum -cnotmatch '\A([0-9a-fA-F]{64})  LiteTicker\.exe\r?\n?\z' -or (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne $Matches[1]) { throw '安装文件 SHA256 不匹配，请重新下载完整安装包。' }
}
$sourceIcon = Get-NativeIconBytes $source
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = if (Test-Path -LiteralPath 'D:\LiteTicker\LiteTicker.exe' -PathType Leaf) { 'D:\LiteTicker' }
        else { Join-Path $env:LOCALAPPDATA 'Programs\LiteTicker' }
}
$InstallDir = [System.IO.Path]::GetFullPath($InstallDir)
$target = Join-Path $InstallDir 'LiteTicker.exe'
$processes = @(Get-Process -Name LiteTicker -ErrorAction SilentlyContinue)
$running = @($processes | Where-Object { -not $_.Path -or [string]::Equals($_.Path, $target, [System.StringComparison]::OrdinalIgnoreCase) })
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

$iconPath = Export-ShortcutIcon $target $sourceIcon
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
} finally {
    if ($downloadDirectory -and (Test-Path -LiteralPath $downloadDirectory)) { Remove-Item -LiteralPath $downloadDirectory -Recurse -Force }
}
