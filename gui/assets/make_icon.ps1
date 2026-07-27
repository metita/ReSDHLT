# Generates the ReSDHLT application icon.
#
# The mark is an isometric brush (the unit a Half-Life map is built from) with
# its top face lit: geometry plus light, which is what the tools do. Dark
# rounded-square plate so it reads on both light and dark taskbars.

Add-Type -AssemblyName System.Drawing

$OutIco = $args[0]
if (-not $OutIco) { throw "uso: make_icon.ps1 <salida.ico>" }
$work = Join-Path ([System.IO.Path]::GetDirectoryName($OutIco)) "icon_png"
New-Item -ItemType Directory -Force $work | Out-Null

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

function New-Poly([float[][]]$pts) {
    $arr = New-Object 'System.Drawing.PointF[]' $pts.Length
    for ($i = 0; $i -lt $pts.Length; $i++) {
        $arr[$i] = New-Object System.Drawing.PointF($pts[$i][0], $pts[$i][1])
    }
    # Unary comma: without it PowerShell unrolls the array and GDI+ overload
    # resolution falls over.
    return , $arr
}

function Render([int]$S) {
    $bmp = New-Object System.Drawing.Bitmap($S, $S, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.PixelOffsetMode = 'HighQuality'
    $u = $S / 256.0   # everything is authored at 256 and scaled

    # --- plate -------------------------------------------------------------
    $inset = 6 * $u
    $plate = New-RoundedPath $inset $inset ($S - 2 * $inset) ($S - 2 * $inset) (58 * $u)
    $rect = New-Object System.Drawing.RectangleF(0, 0, $S, $S)
    $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $rect,
        [System.Drawing.Color]::FromArgb(255, 26, 44, 74),
        [System.Drawing.Color]::FromArgb(255, 9, 14, 24),
        90.0)
    $g.FillPath($grad, $plate)

    # Hairline rim: keeps the plate from dissolving into a dark background.
    $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(70, 150, 200, 255), (2.5 * $u))
    $g.DrawPath($pen, $plate)

    # --- glow above the lit face ------------------------------------------
    if ($S -ge 48) {
        $g.SetClip($plate)
        $glowR = 120 * $u
        $glowPath = New-Object System.Drawing.Drawing2D.GraphicsPath
        $glowPath.AddEllipse(($S / 2 - $glowR), (16 * $u - $glowR * 0.75), ($glowR * 2), ($glowR * 1.5))
        $glow = New-Object System.Drawing.Drawing2D.PathGradientBrush($glowPath)
        $glow.CenterColor = [System.Drawing.Color]::FromArgb(120, 120, 200, 255)
        $glow.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 90, 169, 255))
        $g.FillPath($glow, $glowPath)
        $g.ResetClip()
        $glow.Dispose(); $glowPath.Dispose()
    }

    # --- isometric brush ---------------------------------------------------
    $cx = $S / 2.0
    $cy = $S / 2.0 + 8 * $u
    $w = 84 * $u      # half width
    $th = 48 * $u     # top face half height
    $bh = 70 * $u     # side height

    $top = New-Poly @(
        @($cx, ($cy - $th - $bh / 2)),
        @(($cx + $w), ($cy - $bh / 2)),
        @($cx, ($cy + $th - $bh / 2)),
        @(($cx - $w), ($cy - $bh / 2)))
    $left = New-Poly @(
        @(($cx - $w), ($cy - $bh / 2)),
        @($cx, ($cy + $th - $bh / 2)),
        @($cx, ($cy + $th + $bh / 2)),
        @(($cx - $w), ($cy + $bh / 2)))
    $right = New-Poly @(
        @($cx, ($cy + $th - $bh / 2)),
        @(($cx + $w), ($cy - $bh / 2)),
        @(($cx + $w), ($cy + $bh / 2)),
        @($cx, ($cy + $th + $bh / 2)))

    # Lit top, mid-tone left, dark right: one light source, top-left.
    $topRect = New-Object System.Drawing.RectangleF(($cx - $w), ($cy - $th - $bh), ($w * 2), ($th * 2 + $bh))
    $topBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $topRect,
        [System.Drawing.Color]::FromArgb(255, 214, 238, 255),
        [System.Drawing.Color]::FromArgb(255, 108, 186, 255),
        70.0)
    $leftBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 46, 116, 196))
    $rightBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 22, 70, 128))

    $g.FillPolygon($rightBrush, $right)
    $g.FillPolygon($leftBrush, $left)
    $g.FillPolygon($topBrush, $top)

    # Lightmap on the lit face: the luxel grid RAD computes, drawn as two thin
    # lines each way. Only above 48px, where they are actually resolvable.
    if ($S -ge 48) {
        $g.SetClip((New-Object System.Drawing.Drawing2D.GraphicsPath))
        $g.ResetClip()
        $lm = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(60, 12, 46, 92), (1.6 * $u))
        foreach ($f in 0.33, 0.66) {
            # along one diagonal of the rhombus, then the other
            $a = New-Object System.Drawing.PointF(
                ($cx - $w + $w * $f), ($cy - $bh / 2 - $th * $f))
            $b = New-Object System.Drawing.PointF(
                ($cx + $w * $f), ($cy - $bh / 2 + $th * (1 - $f)))
            $g.DrawLine($lm, $a, $b)
            $c = New-Object System.Drawing.PointF(
                ($cx + $w - $w * $f), ($cy - $bh / 2 - $th * $f))
            $d = New-Object System.Drawing.PointF(
                ($cx - $w * $f), ($cy - $bh / 2 + $th * (1 - $f)))
            $g.DrawLine($lm, $c, $d)
        }
        $lm.Dispose()
    }

    # Edge highlight along the lit silhouette, dropped at small sizes where it
    # would only add mush.
    if ($S -ge 32) {
        $edge = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(200, 240, 250, 255), (2.4 * $u))
        $g.DrawPolygon($edge, $top)
        $edge.Dispose()
    }

    # Rim light down the front-left edge: one more cue that a light source is
    # doing the work.
    if ($S -ge 32) {
        $rim = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(150, 120, 200, 255), (2.2 * $u))
        $g.DrawLine($rim,
            (New-Object System.Drawing.PointF(($cx - $w), ($cy - $bh / 2))),
            (New-Object System.Drawing.PointF(($cx - $w), ($cy + $bh / 2))))
        $rim.Dispose()
    }

    # The light itself: a spark up in the corner the shading points back to.
    if ($S -ge 32) {
        $sx = $S * 0.235
        $sy = $S * 0.215
        $r = 9 * $u
        $halo = New-Object System.Drawing.Drawing2D.GraphicsPath
        $halo.AddEllipse(($sx - $r * 3), ($sy - $r * 3), ($r * 6), ($r * 6))
        $hb = New-Object System.Drawing.Drawing2D.PathGradientBrush($halo)
        $hb.CenterColor = [System.Drawing.Color]::FromArgb(190, 220, 240, 255)
        $hb.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 120, 200, 255))
        $g.FillPath($hb, $halo)
        $core = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 245, 251, 255))
        $g.FillEllipse($core, ($sx - $r * 0.5), ($sy - $r * 0.5), $r, $r)
        $hb.Dispose(); $halo.Dispose(); $core.Dispose()
    }

    $g.Dispose()
    $path = Join-Path $work "icon_$S.png"
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return $path
}

$sizes = @(256, 128, 64, 48, 32, 24, 16)
$files = @()
foreach ($s in $sizes) { $files += Render $s }

# --- pack the ICO ---------------------------------------------------------
# PNG-compressed entries, supported by Windows Vista and later.
$fs = [System.IO.File]::Create($OutIco)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([uint16]0)               # reserved
$bw.Write([uint16]1)               # type: icon
$bw.Write([uint16]$sizes.Count)

$offset = 6 + 16 * $sizes.Count
$blobs = @()
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $bytes = [System.IO.File]::ReadAllBytes($files[$i])
    $blobs += , $bytes
    $dim = $sizes[$i]
    $bw.Write([byte]($(if ($dim -ge 256) { 0 } else { $dim })))
    $bw.Write([byte]($(if ($dim -ge 256) { 0 } else { $dim })))
    $bw.Write([byte]0)             # palette
    $bw.Write([byte]0)             # reserved
    $bw.Write([uint16]1)           # planes
    $bw.Write([uint16]32)          # bits per pixel
    $bw.Write([uint32]$bytes.Length)
    $bw.Write([uint32]$offset)
    $offset += $bytes.Length
}
foreach ($b in $blobs) { $bw.Write($b) }
$bw.Flush(); $bw.Close(); $fs.Close()

"ICO: $OutIco ({0:N1} KB, {1} tamaños)" -f ((Get-Item $OutIco).Length / 1KB), $sizes.Count
