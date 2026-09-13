# Draws the app icon at several sizes and packs them into a multi-resolution .ico.
# Design: rounded gradient tile with a white mixer-fader glyph (track + knob cap).
param([string]$OutPath = "$PSScriptRoot\..\assets\icon.ico")

Add-Type -AssemblyName System.Drawing

function New-RoundedPath([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $r * 2
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

    $s = [float]$size
    $inset = $s * 0.04
    $tile = $s - ($inset * 2)
    $radius = $s * 0.235

    # Gradient tile
    $tilePath = New-RoundedPath $inset $inset $tile $tile $radius
    $rect = New-Object System.Drawing.RectangleF(0, 0, $s, $s)
    $c1 = [System.Drawing.Color]::FromArgb(255, 96, 175, 255)
    $c2 = [System.Drawing.Color]::FromArgb(255, 31, 94, 214)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $c1, $c2, 55.0)
    $g.FillPath($brush, $tilePath)

    # Subtle top highlight for depth
    $hlRect = New-Object System.Drawing.RectangleF(0, 0, $s, $s * 0.55)
    $hl1 = [System.Drawing.Color]::FromArgb(46, 255, 255, 255)
    $hl2 = [System.Drawing.Color]::FromArgb(0, 255, 255, 255)
    $hlBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($hlRect, $hl1, $hl2, 90.0)
    $oldClip = $g.Clip
    $g.SetClip($tilePath)
    $g.FillRectangle($hlBrush, $hlRect)
    $g.Clip = $oldClip

    # Speaker silhouette: neck + cone as one polygon, white.
    $white = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
    $speaker = New-Object System.Drawing.Drawing2D.GraphicsPath
    $pts = @(
        (New-Object System.Drawing.PointF(($s * 0.235), ($s * 0.405))),
        (New-Object System.Drawing.PointF(($s * 0.345), ($s * 0.405))),
        (New-Object System.Drawing.PointF(($s * 0.520), ($s * 0.225))),
        (New-Object System.Drawing.PointF(($s * 0.520), ($s * 0.775))),
        (New-Object System.Drawing.PointF(($s * 0.345), ($s * 0.595))),
        (New-Object System.Drawing.PointF(($s * 0.235), ($s * 0.595)))
    )
    $speaker.AddPolygon([System.Drawing.PointF[]]$pts)
    $speaker.CloseFigure()
    $g.FillPath($white, $speaker)

    # Sound arcs; the outer one is dropped at tiny sizes so it stays legible.
    $penW = [Math]::Max(1.25, $s * 0.075)
    $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::White, $penW)
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $cx = $s * 0.505
    $cy = $s * 0.50
    $radii = if ($size -le 20) { @(0.175) } else { @(0.165, 0.275) }
    foreach ($rf in $radii) {
        $r = $s * $rf
        $g.DrawArc($pen, ($cx - $r), ($cy - $r), ($r * 2), ($r * 2), -52, 104)
    }

    $g.Dispose()
    return $bmp
}

$sizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
$pngs = @()
foreach ($sz in $sizes) {
    $bmp = New-IconBitmap $sz
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngs += ,@($sz, $ms.ToArray())
    $bmp.Dispose()
    $ms.Dispose()
}

$dir = Split-Path $OutPath -Parent
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

$fs = [System.IO.File]::Create($OutPath)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([UInt16]0)
$bw.Write([UInt16]1)
$bw.Write([UInt16]$pngs.Count)
$offset = 6 + (16 * $pngs.Count)
foreach ($entry in $pngs) {
    $sz = $entry[0]; $data = $entry[1]
    $dim = if ($sz -ge 256) { 0 } else { $sz }
    $bw.Write([Byte]$dim)
    $bw.Write([Byte]$dim)
    $bw.Write([Byte]0)
    $bw.Write([Byte]0)
    $bw.Write([UInt16]1)
    $bw.Write([UInt16]32)
    $bw.Write([UInt32]$data.Length)
    $bw.Write([UInt32]$offset)
    $offset += $data.Length
}
foreach ($entry in $pngs) { $bw.Write($entry[1]) }
$bw.Close()
$fs.Close()

# Preview sheet so the design can be eyeballed at real sizes
$preview = New-Object System.Drawing.Bitmap(560, 300, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$pg = [System.Drawing.Graphics]::FromImage($preview)
$pg.Clear([System.Drawing.Color]::FromArgb(255, 32, 32, 36))
$big = New-IconBitmap 256
$pg.DrawImage($big, 16, 16, 256, 256)
$x = 300
foreach ($sz in @(16, 24, 32, 48, 64)) {
    $b = New-IconBitmap $sz
    $pg.DrawImage($b, $x, 140, $sz, $sz)
    $x += $sz + 18
    $b.Dispose()
}
$pg.Dispose()
$preview.Save("$PSScriptRoot\..\assets\icon_preview.png", [System.Drawing.Imaging.ImageFormat]::Png)
$preview.Dispose()
$big.Dispose()

Write-Output "wrote $OutPath ($((Get-Item $OutPath).Length) bytes)"
