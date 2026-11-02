param([string]$Engine = (Join-Path $env:LOCALAPPDATA 'Programs\Glancy\Engine'))
# End-to-end test of the engine: changes real settings in HKCU\Software\Glancy\Thumbnails (as the app would),
# generates thumbnails with the installed DLL through COM and checks the effect and the history. When it finishes it
# restores the exact previous configuration (reg export / import).
$ErrorActionPreference = 'Stop'
# glancy-thumb writes UTF-8; without this, "×" and "·" are garbled depending on the console code page.
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$key ='HKCU\Software\Glancy\Thumbnails'
$psKey = 'HKCU:\Software\Glancy\Thumbnails'
$dll = Join-Path $Engine 'Glancy.Thumbnails.dll'
$exe = Join-Path $Engine 'glancy-thumb.exe'
$work = Join-Path ([IO.Path]::GetTempPath()) ("glancy-e2e-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$fixtures = Join-Path $work 'fixtures'
$backup = Join-Path $work 'backup.reg'
$history = Join-Path $env:LOCALAPPDATA 'Glancy\Logs\history.log'
New-Item -ItemType Directory -Force $fixtures | Out-Null
$script:fails = 0
function Check($name, [bool]$ok, $detail = '') {
    if ($ok) { "PASS  $name" } else { "FAIL  $name  $detail"; $script:fails++ }
}

# A 200×100 SVG: the left half red, the rest transparent.
$svg = Join-Path $fixtures 'half.svg'
Set-Content $svg -Encoding utf8 '<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="100" height="100" fill="#f00"/></svg>'

function Thumb([string]$file, [int]$size = 256) {
    # Expected glancy-thumb errors arrive on stderr; here they must not stop the script.
    $ErrorActionPreference = 'Continue'
    $out = Join-Path $work ("t{0}.png" -f [guid]::NewGuid().ToString('N').Substring(0, 6))
    $text = (& $exe com $dll $file $size $out 'item' 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) { return @{ ok = $false; text = $text.Trim() } }
    if ($text -notmatch '(\d+)×(\d+) in \d+ ms · WTS_ALPHATYPE (\d+) · non-opaque pixels: (\d+)') { throw "Unexpected output: $text" }
    @{ ok = $true; w = [int]$Matches[1]; h = [int]$Matches[2]; alpha = [int]$Matches[3]; transparent = [int]$Matches[4] }
}

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class Assoc {
    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
    static extern int AssocQueryStringW(int flags, int str, string assoc, string extra, StringBuilder outBuf, ref uint len);
    [ComImport, Guid("c46ca590-3c3f-11d2-bee6-0000f805ca57"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IQueryAssociations {
        [PreserveSig] int Init(int flags, [MarshalAs(UnmanagedType.LPWStr)] string assoc, IntPtr hkProgid, IntPtr hwnd);
        [PreserveSig] int GetString(int flags, int str, [MarshalAs(UnmanagedType.LPWStr)] string extra, StringBuilder outBuf, ref uint len);
        [PreserveSig] int GetKey(int flags, int key, [MarshalAs(UnmanagedType.LPWStr)] string extra, out IntPtr hkey);
        [PreserveSig] int GetData(int flags, int data, [MarshalAs(UnmanagedType.LPWStr)] string extra, IntPtr outBuf, ref uint len);
    }
    [ComImport, Guid("43826d1e-e718-42ee-bc55-a1e261c37bfe"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellItem {
        [PreserveSig] int BindToHandler(IntPtr pbc, ref Guid bhid, ref Guid riid, [MarshalAs(UnmanagedType.IUnknown)] out object ppv);
    }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern int SHCreateItemFromParsingName(string path, IntPtr pbc, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out IShellItem item);

    public static string ProgId(string ext) {
        var sb = new StringBuilder(512); uint len = 512;
        return AssocQueryStringW(0, 20 /* ASSOCSTR_PROGID */, ext, null, sb, ref len) == 0 ? sb.ToString() : null;
    }
    // A value as Explorer resolves it for the file, through its whole association (null = absent everywhere).
    public static string Resolved(string path, string name) {
        Guid iidItem = typeof(IShellItem).GUID, bhid = new Guid("3981E225-F559-11D3-8E3A-00C04F6837D5") /* BHID_SFUIObject */;
        Guid iidQa = typeof(IQueryAssociations).GUID;
        IShellItem item; object o;
        if (SHCreateItemFromParsingName(path, IntPtr.Zero, ref iidItem, out item) != 0) return null;
        if (item.BindToHandler(IntPtr.Zero, ref bhid, ref iidQa, out o) != 0) return null;
        uint len = 4096; IntPtr buf = Marshal.AllocCoTaskMem((int)len);
        try {
            if (((IQueryAssociations)o).GetData(0, 6 /* ASSOCDATA_VALUE */, name, buf, ref len) != 0) return null;
            return len == 4 ? Marshal.ReadInt32(buf).ToString() : Marshal.PtrToStringUni(buf).TrimEnd('\0');
        } finally { Marshal.FreeCoTaskMem(buf); }
    }
}
'@

function Sync {
    # Like the app after an Apply: the engine writes Treatment/TypeOverlay at once.
    $p = Start-Process (Join-Path $env:windir 'System32\regsvr32.exe') "/s /n /i:sync `"$dll`"" -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "regsvr32 /n /i:sync failed ($($p.ExitCode))" }
}

function Apply([hashtable]$values, [string]$sub = 'Formats\svg') {
    $path = Join-Path $psKey $sub
    New-Item -Force $path | Out-Null
    foreach ($k in $values.Keys) {
        $v = $values[$k]
        if ($v -is [array]) { New-ItemProperty $path $k -PropertyType MultiString -Value $v -Force | Out-Null }
        else { New-ItemProperty $path $k -PropertyType DWord -Value $v -Force | Out-Null }
    }
    # Like the app: Revision is incremented at the end of every Apply.
    $rev = (Get-ItemProperty $psKey -ErrorAction SilentlyContinue).Revision
    New-ItemProperty $psKey Revision -PropertyType DWord -Value ([int]$rev + 1) -Force | Out-Null
}

$hadKey = Test-Path $psKey
if ($hadKey) {
    cmd /c "reg export `"$key`" `"$backup`" /y >nul 2>&1"
    # Without a backup nothing is touched: the user's settings could not be restored.
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $backup) -or (Get-Item $backup).Length -eq 0) {
        Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
        throw "Could not export $key; the test changes nothing."
    }
}
try {
    if ($hadKey) { Remove-Item $psKey -Recurse -Force }
    New-Item -Force $psKey | Out-Null

    $r = Thumb $svg
    Check 'Defaults: transparent SVG 256×128' ($r.ok -and $r.w -eq 256 -and $r.h -eq 128 -and $r.transparent -gt 10000) ($r | Out-String)
    Check 'HBITMAP with WTSAT_ARGB' ($r.alpha -eq 2)

    Apply @{ transparency = 1 }
    $r = Thumb $svg
    Check 'Canvas color: no transparent pixels' ($r.ok -and $r.transparent -eq 0) ($r | Out-String)

    Apply @{ transparency = 2; fullFill = 1 }
    $r = Thumb $svg
    Check 'Full square canvas: 256×256' ($r.ok -and $r.w -eq 256 -and $r.h -eq 256) ($r | Out-String)

    Apply @{ fullFill = 0 }
    Apply @{ transparency = 0 } 'Formats\svg\Extensions\.svg'
    $r = Thumb $svg
    Check 'The extension override wins over the format (opaque .svg)' ($r.ok -and $r.transparent -eq 0) ($r | Out-String)
    Remove-Item (Join-Path $psKey 'Formats\svg\Extensions') -Recurse -Force

    Apply @{ mode = 0 }
    $r = Thumb $svg
    Check 'Format off: no thumbnail' (-not $r.ok)
    Apply @{ mode = 1 }

    Apply @{ blockedFolders = [string[]]@($fixtures) } 'Global'
    $r = Thumb $svg
    Check 'Excluded folder: no thumbnail' (-not $r.ok)
    Apply @{ blockedFolders = [string[]]@() } 'Global'

    Apply @{ enabled = 0 } 'Global'
    $r = Thumb $svg
    Check 'Master switch off: no thumbnail' (-not $r.ok)
    Apply @{ enabled = 1 } 'Global'

    $r = Thumb $svg 32
    Check 'Small icons off by default (32 px)' (-not $r.ok)
    Apply @{ smallIcons = 1 }
    $r = Thumb $svg 32
    Check 'Small icons with Glancy (32 px)' ($r.ok -and $r.w -eq 32) ($r | Out-String)

    $bad = Join-Path $fixtures 'broken.svg'
    Set-Content $bad -Encoding utf8 '<svg'
    $r = Thumb $bad
    Check 'Corrupt file: handled error' (-not $r.ok)

    $tail = Get-Content $history -Tail 20
    $okLine = $tail | Where-Object { $_ -match "`tok`t\d+`t$([regex]::Escape($svg))$" } | Select-Object -Last 1
    $errLine = $tail | Where-Object { $_ -match "`terror`t\d+`t$([regex]::Escape($bad))`t.+" } | Select-Object -Last 1
    Check 'history.log: "ok" line with the full path' ($null -ne $okLine)
    Check 'history.log: "error" line with a detail' ($null -ne $errLine) ($tail -join "`n")
    Check 'history.log: ISO 8601 UTC time' ($okLine -match '^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ\t')
    $blockedLogged = $tail | Where-Object { $_ -like "*$fixtures*" -and $_ -notlike "*half.svg" -and $_ -notlike '*broken.svg*' }
    Check 'Excluded folders leave no trace in the history' ($null -eq $blockedLogged)

    # Frame and overlay icon, as Explorer resolves them (not just what the engine writes): Windows sets
    # TypeOverlay="" in SystemFileAssociations\image and in Unknown, which an absent value inherits.
    $assoc = (Get-ItemProperty 'HKCU:\Software\Glancy\Engine' -ErrorAction SilentlyContinue).DllPath
    if (-not $assoc) {
        "SKIP  Overlay icon: the engine is not registered"
    } else {
        $shell32 = Join-Path $env:windir 'System32\shell32.dll'
        Apply @{ adornment = 0 }
        Sync
        Check 'Frame off: Treatment 0' ([Assoc]::Resolved($svg, 'Treatment') -eq '0') ([Assoc]::Resolved($svg, 'Treatment'))
        Apply @{ adornment = 1; showAssocIcon = 0 }
        Sync
        Check 'Frame on: Treatment 1' ([Assoc]::Resolved($svg, 'Treatment') -eq '1')
        Check 'App icon off: no overlay' ([Assoc]::Resolved($svg, 'TypeOverlay') -eq '')
        Apply @{ showAssocIcon = 1 }
        Sync
        $overlay = [Assoc]::Resolved($svg, 'TypeOverlay')
        $progId = [Assoc]::ProgId('.svg')
        if (-not $progId -or $progId -eq 'Unknown') {
            Check 'App icon on, no app for .svg: no overlay' (-not $overlay) $overlay
        } else {
            Check "App icon on: the icon of the app that opens .svg ($progId)" ($overlay -match '^@\{|,-?\d+$') $overlay
        }
        Apply @{ customIcon = 1 }
        New-ItemProperty (Join-Path $psKey 'Formats\svg') customIconPath -PropertyType String -Value "$shell32,3" -Force | Out-Null
        Sync
        Check 'Custom icon' ([Assoc]::Resolved($svg, 'TypeOverlay') -eq "$shell32,3") ([Assoc]::Resolved($svg, 'TypeOverlay'))

        # A type no app opens resolves Unknown, which sets "": the custom icon goes in the extension's key.
        $loose = '.tga', '.dds', '.psb', '.kra' | Where-Object { [Assoc]::ProgId($_) -in $null, 'Unknown' } | Select-Object -First 1
        if (-not $loose) {
            "SKIP  Custom icon on a type no app opens: every candidate has an app"
        } else {
            $format = @{ '.tga' = 'tga'; '.dds' = 'dds'; '.psb' = 'psd'; '.kra' = 'krita' }[$loose]
            $file = Join-Path $fixtures "loose$loose"
            Set-Content $file ''
            Apply @{ customIcon = 1; adornment = 0 } "Formats\$format"
            New-ItemProperty (Join-Path $psKey "Formats\$format") customIconPath -PropertyType String -Value "$shell32,4" -Force | Out-Null
            Sync
            Check "Custom icon on a type no app opens ($loose)" ([Assoc]::Resolved($file, 'TypeOverlay') -eq "$shell32,4") ([Assoc]::Resolved($file, 'TypeOverlay'))
            Check "Frame on a type no app opens ($loose): Treatment 0" ([Assoc]::Resolved($file, 'Treatment') -eq '0') ([Assoc]::Resolved($file, 'Treatment'))
        }
    }
}
finally {
    Remove-Item $psKey -Recurse -Force -ErrorAction SilentlyContinue
    if ($hadKey) { cmd /c "reg import `"$backup`" >nul 2>&1" }
    # Puts the frame and overlay icon back as the restored settings ask.
    if ((Get-ItemProperty 'HKCU:\Software\Glancy\Engine' -ErrorAction SilentlyContinue).DllPath) { try { Sync } catch { "WARN  $_" } }
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
""
if ($script:fails -eq 0) { "All checks passed." } else { "$($script:fails) checks failed."; exit 1 }
