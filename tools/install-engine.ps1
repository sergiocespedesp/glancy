param([switch]$Uninstall, [string]$Source)
# Installs the engine for the current user (no administrator) in %LOCALAPPDATA%\Programs\Glancy\Engine and registers
# it with regsvr32. -Uninstall unregisters it (restoring the previous thumbnail providers) and deletes the files.
# Default source: dist\Engine (see tools/build-engine.ps1).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Source) { $Source = Join-Path $root 'dist\Engine' }
$target = Join-Path $env:LOCALAPPDATA 'Programs\Glancy\Engine'
$dll = Join-Path $target 'Glancy.Thumbnails.dll'
$regsvr = Join-Path $env:SystemRoot 'System32\regsvr32.exe'

function Invoke-Regsvr([string[]]$arguments) {
    $p = Start-Process $regsvr -ArgumentList $arguments -Wait -PassThru -WindowStyle Hidden
    if ($p.ExitCode -ne 0) { throw "regsvr32 $($arguments -join ' ') exited with code $($p.ExitCode)." }
}

# Explorer and its thumbnail processes (dllhost) keep the DLL loaded: it cannot be overwritten, but it can be
# renamed. The leftovers are deleted on the next install.
function Move-Aside([string]$path) {
    if (-not (Test-Path $path)) { return }
    try { Remove-Item $path -Force } catch { Rename-Item $path ("{0}.old-{1:yyyyMMddHHmmss}" -f (Split-Path -Leaf $path), (Get-Date)) -Force }
}

if ($Uninstall) {
    if (Test-Path $dll) { Invoke-Regsvr @('/s', '/u', "`"$dll`"") }
    Get-ChildItem $target -File -ErrorAction SilentlyContinue | ForEach-Object { Move-Aside $_.FullName }
    "Engine unregistered. Any remaining .old-* files can be deleted after restarting Explorer."
    return
}

if (-not (Test-Path (Join-Path $Source 'Glancy.Thumbnails.dll'))) { throw "No build in $Source (run tools/build-engine.ps1)." }
New-Item -ItemType Directory -Force $target | Out-Null
Get-ChildItem $target -Filter '*.old-*' -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
foreach ($name in 'Glancy.Thumbnails.dll', 'pdfium.dll', 'glancy-thumb.exe') {
    $dest = Join-Path $target $name
    Move-Aside $dest
    Copy-Item (Join-Path $Source $name) $dest
}
Copy-Item (Join-Path $Source 'licenses') $target -Recurse -Force
Invoke-Regsvr @('/s', "`"$dll`"")
"Engine installed and registered in $target"
