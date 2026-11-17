[CmdletBinding()]
param(
    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $OutputPath) { $OutputPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'assets\liteticker.ico' }
if ($env:OS -ne 'Windows_NT') { throw 'Generate the icon on Windows.' }
Add-Type -AssemblyName System.Drawing

$sizes = @(16, 24, 32, 48, 64, 128, 256)
[byte[][]]$images = @()
foreach ($size in $sizes) {
    # Render at 4x before reducing so the same mark stays legible at small sizes.
    $renderSize = $size * 4
    $bitmap = [System.Drawing.Bitmap]::new($renderSize, $renderSize)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $font = [System.Drawing.Font]::new('Microsoft YaHei UI', $renderSize * 0.76, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $brush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(244, 249, 248))
    $format = [System.Drawing.StringFormat]::new()
    $scaled = [System.Drawing.Bitmap]::new($size, $size)
    $painter = [System.Drawing.Graphics]::FromImage($scaled)
    $memory = [System.IO.MemoryStream]::new()
    try {
        $graphics.Clear([System.Drawing.Color]::FromArgb(134, 31, 43))
        $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
        $format.Alignment = [System.Drawing.StringAlignment]::Center
        $format.LineAlignment = [System.Drawing.StringAlignment]::Center
        $graphics.DrawString('轻', $font, $brush, [System.Drawing.RectangleF]::new(0, -$renderSize * 0.014, $renderSize, $renderSize), $format)
        $painter.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $painter.DrawImage($bitmap, 0, 0, $size, $size)
        $scaled.Save($memory, [System.Drawing.Imaging.ImageFormat]::Png)
        $images += ,$memory.ToArray()
    } finally {
        $memory.Dispose(); $painter.Dispose(); $scaled.Dispose()
        $format.Dispose(); $brush.Dispose(); $font.Dispose()
        $graphics.Dispose(); $bitmap.Dispose()
    }
}

$OutputPath = [System.IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Path (Split-Path -Parent $OutputPath) -Force | Out-Null
$stream = [System.IO.File]::Create($OutputPath)
$writer = [System.IO.BinaryWriter]::new($stream)
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$sizes.Count)
    $offset = 6 + 16 * $sizes.Count
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $dimension = if ($sizes[$i] -eq 256) { 0 } else { $sizes[$i] }
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension)
        $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]$images[$i].Length); $writer.Write([uint32]$offset)
        $offset += $images[$i].Length
    }
    foreach ($bytes in $images) { $writer.Write([byte[]]$bytes) }
} finally { $writer.Dispose(); $stream.Dispose() }
Write-Host "Icon: $OutputPath"
