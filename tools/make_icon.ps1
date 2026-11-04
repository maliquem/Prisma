# Gera res/prisma.ico com o prisma sobre fundo grafite.
Add-Type -AssemblyName System.Drawing
$sizes = 16, 24, 32, 48, 64, 256
$images = foreach ($s in $sizes) {
    $bmp = New-Object System.Drawing.Bitmap $s, $s
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.Clear([System.Drawing.Color]::Transparent)
    $r = [single]($s * 0.22)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = [single]($r * 2); $e = [single]($s - 1 - $d)
    $path.AddArc(0, 0, $d, $d, 180, 90)
    $path.AddArc($e, 0, $d, $d, 270, 90)
    $path.AddArc($e, $e, $d, $d, 0, 90)
    $path.AddArc(0, $e, $d, $d, 90, 90)
    $path.CloseFigure()
    $bg = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x1b, 0x1d, 0x22))
    $g.FillPath($bg, $path)
    $w = [single][Math]::Max(1.4, $s * 0.075)
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 0x7c, 0xc4, 0xff)), $w
    $pen.LineJoin = 'Round'
    $top = New-Object System.Drawing.PointF ([single]($s * 0.5)), ([single]($s * 0.2))
    $left = New-Object System.Drawing.PointF ([single]($s * 0.17)), ([single]($s * 0.8))
    $right = New-Object System.Drawing.PointF ([single]($s * 0.83)), ([single]($s * 0.8))
    $mid = New-Object System.Drawing.PointF ([single]($s * 0.5)), ([single]($s * 0.8))
    [System.Drawing.PointF[]]$tri = $top, $right, $left
    $g.DrawPolygon($pen, $tri)
    $g.DrawLine($pen, $top, $mid)
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    [pscustomobject]@{ Size = $s; Data = $ms.ToArray() }
}
$out = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $out
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($i in $images) {
    $b = if ($i.Size -ge 256) { 0 } else { $i.Size }
    $bw.Write([byte]$b); $bw.Write([byte]$b); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([uint32]$i.Data.Length); $bw.Write([uint32]$offset)
    $offset += $i.Data.Length
}
foreach ($i in $images) { $bw.Write($i.Data) }
$bw.Flush()
[IO.File]::WriteAllBytes((Join-Path $PSScriptRoot '..\res\prisma.ico'), $out.ToArray())
