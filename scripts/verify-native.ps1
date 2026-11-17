Set-StrictMode -Version Latest

function Get-NativeVersion {
    param([string]$RepositoryDirectory)
    $source = [IO.File]::ReadAllText((Join-Path $RepositoryDirectory 'native\Cargo.toml'))
    $match = [regex]::Match($source, '(?m)^version\s*=\s*"(?<version>[1-9][0-9]*\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))"\s*$')
    if (-not $match.Success -or ([version]$match.Groups['version'].Value).Major -lt 2) { throw 'native/Cargo.toml must contain a stable native version (2.0.0 or newer).' }
    return $match.Groups['version'].Value
}

function Test-NativeExecutable {
    param([string]$Path, [string]$Version)
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 512 -or $bytes.Length -gt 20MB) { throw 'The native executable must be between 512 bytes and 20 MiB.' }
    if ([BitConverter]::ToUInt16($bytes, 0) -ne 0x5a4d) { throw 'The executable is not a Windows PE file.' }
    $pe = [BitConverter]::ToInt32($bytes, 0x3c)
    if ($pe -lt 64 -or $pe -gt $bytes.Length - 264 -or [BitConverter]::ToUInt32($bytes, $pe) -ne 0x4550) { throw 'The PE header is invalid.' }
    $optional = $pe + 24
    $optionalSize = [BitConverter]::ToUInt16($bytes, $pe + 20)
    if ([BitConverter]::ToUInt16($bytes, $pe + 4) -ne 0x8664 -or [BitConverter]::ToUInt16($bytes, $optional) -ne 0x20b -or $optionalSize -lt 240) { throw 'LiteTicker must be a Windows x64 PE32+ executable.' }
    if ([BitConverter]::ToUInt16($bytes, $optional + 68) -ne 2 -or ([BitConverter]::ToUInt16($bytes, $pe + 22) -band 0x2000)) { throw 'LiteTicker must be a Windows GUI executable, not a DLL.' }
    if ([BitConverter]::ToUInt64($bytes, $optional + 112 + 14 * 8) -ne 0) { throw 'Managed CLR executables are not accepted for the native release.' }
    $binary = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path)
    if ($binary.ProductName -cne 'LiteTicker' -or $binary.OriginalFilename -cne 'LiteTicker.exe' -or $binary.ProductVersion -cne $Version -or $binary.FileVersion -cne ($Version + '.0')) { throw 'The PE product name, original filename or version does not match the native release.' }
    $expected = [version]($Version + '.0')
    if ($binary.FileMajorPart -ne $expected.Major -or $binary.FileMinorPart -ne $expected.Minor -or $binary.FileBuildPart -ne $expected.Build -or $binary.FilePrivatePart -ne 0 -or $binary.ProductMajorPart -ne $expected.Major -or $binary.ProductMinorPart -ne $expected.Minor -or $binary.ProductBuildPart -ne $expected.Build -or $binary.ProductPrivatePart -ne 0) { throw 'The fixed PE version does not match the native release.' }
    $sections = [BitConverter]::ToUInt16($bytes, $pe + 6)
    $sectionTable = $optional + $optionalSize
    if ($sectionTable + $sections * 40 -gt $bytes.Length) { throw 'The PE section table is incomplete.' }
    function File-Offset([uint32]$Rva) {
        if ($Rva -lt [BitConverter]::ToUInt32($bytes, $optional + 60)) { return [int]$Rva }
        for ($i = 0; $i -lt $sections; $i++) {
            $section = $sectionTable + $i * 40
            $start = [BitConverter]::ToUInt32($bytes, $section + 12)
            $size = [BitConverter]::ToUInt32($bytes, $section + 16)
            if ($Rva -ge $start -and $Rva - $start -lt $size) { return [int]([BitConverter]::ToUInt32($bytes, $section + 20) + $Rva - $start) }
        }
        throw 'A PE import address is outside the file sections.'
    }
    $imports = [Collections.Generic.List[string]]::new()
    $rva = [BitConverter]::ToUInt32($bytes, $optional + 120)
    if ($rva) {
        $offset = File-Offset $rva
        for ($i = 0; $i -lt 512; $i++) {
            if ($offset -gt $bytes.Length - 20) { throw 'The PE import table is incomplete.' }
            $nameRva = [BitConverter]::ToUInt32($bytes, $offset + 12)
            if (-not $nameRva) { break }
            $name = File-Offset $nameRva
            $end = $name
            while ($end -lt $bytes.Length -and $bytes[$end] -ne 0 -and $end - $name -lt 260) { $end++ }
            if ($end -ge $bytes.Length -or $bytes[$end] -ne 0) { throw 'The PE import name is invalid.' }
            $imports.Add([Text.Encoding]::ASCII.GetString($bytes, $name, $end - $name))
            $offset += 20
        }
        if ($i -eq 512) { throw 'The PE import table did not terminate.' }
    }
    if ([BitConverter]::ToUInt64($bytes, $optional + 112 + 13 * 8) -ne 0) { throw 'Inspect delayed imports before adding them to the single-file release.' }
    if ($imports.Count -eq 0 -or @($imports | Where-Object { $_ -match '^(?:vcruntime|msvcp|msvcr|ucrtbase|api-ms-win-crt|coreclr|clr|mscoree)' }).Count) { throw 'The executable imports a CLR or shared C/C++ runtime; static CRT is required.' }
    return [pscustomobject]@{ Version = $Version; Machine = 'AMD64'; Managed = $false; StaticCrt = $true; Bytes = $bytes.Length; Imports = @($imports); Sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
}
