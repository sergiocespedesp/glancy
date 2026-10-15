param([switch]$SkipTests)
# Builds the thumbnail engine (src/Glancy.Thumbnails) and puts everything to install in dist/Engine:
# Glancy.Thumbnails.dll, pdfium.dll, glancy-thumb.exe (diagnostics) and the third-party licenses.
# Needs Rust (x86_64-pc-windows-gnu toolchain, per user). Visual Studio is not needed.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$crate = Join-Path $root 'src\Glancy.Thumbnails'
$build = Join-Path $env:LOCALAPPDATA 'Temp\lmb'
$dist = Join-Path $root 'dist\Engine'

$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { throw 'Rust not found: install rustup with --default-host x86_64-pc-windows-gnu.' }
# Output goes outside the repository and under the 260-character path limit, like the rest of the build.
$env:CARGO_TARGET_DIR = Join-Path $build 'cargo'

# PDFium: a pinned release of https://github.com/bblanchon/pdfium-binaries (BSD/Apache), checked by SHA-256.
$pdfiumTag = 'chromium/8100'
$pdfiumSha = '51621BCC1D9BE9EA448F4B84B0516862B928E3968D3FCA899AF1A6D6E2848D6D'
$pdfiumDir = Join-Path $build 'deps\pdfium'
$tgz = Join-Path $pdfiumDir 'pdfium-win-x64.tgz'
if (-not (Test-Path (Join-Path $pdfiumDir 'bin\pdfium.dll'))) {
    New-Item -ItemType Directory -Force $pdfiumDir | Out-Null
    Invoke-WebRequest -UseBasicParsing -OutFile $tgz "https://github.com/bblanchon/pdfium-binaries/releases/download/$pdfiumTag/pdfium-win-x64.tgz"
    if ((Get-FileHash $tgz -Algorithm SHA256).Hash -ne $pdfiumSha) { Remove-Item $tgz; throw 'The PDFium SHA-256 does not match.' }
    tar -xzf $tgz -C $pdfiumDir
    if ($LASTEXITCODE -ne 0) { throw 'Could not extract PDFium.' }
}
$env:GLANCY_PDFIUM = Join-Path $pdfiumDir 'bin\pdfium.dll'

Push-Location $crate
try {
    if (-not $SkipTests) {
        & $cargo test --release
        if ($LASTEXITCODE -ne 0) { throw 'The engine tests failed.' }
    }
    & $cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'The engine build failed.' }
} finally { Pop-Location }

New-Item -ItemType Directory -Force $dist, (Join-Path $dist 'licenses') | Out-Null
$out = Join-Path $env:CARGO_TARGET_DIR 'release'
Copy-Item (Join-Path $out 'glancy_thumbnails.dll') (Join-Path $dist 'Glancy.Thumbnails.dll') -Force
Copy-Item (Join-Path $out 'glancy-thumb.exe') $dist -Force
Copy-Item $env:GLANCY_PDFIUM $dist -Force
Copy-Item (Join-Path $pdfiumDir 'licenses\*') (Join-Path $dist 'licenses') -Force

# License texts of the Rust crates compiled into the engine: the normal (not build or dev) dependencies for this
# target, walked from the engine crate. A crate without license files gets a note with its license expression.
$meta = & $cargo metadata --format-version 1 --filter-platform x86_64-pc-windows-gnu --manifest-path (Join-Path $crate 'Cargo.toml') | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed.' }
$nodes = @{}; foreach ($n in $meta.resolve.nodes) { $nodes[$n.id] = $n }
$byId = @{}; foreach ($p in $meta.packages) { $byId[$p.id] = $p }
$seen = @{}; $queue = New-Object System.Collections.Queue
$queue.Enqueue($meta.resolve.root)
while ($queue.Count) {
    foreach ($dep in $nodes[$queue.Dequeue()].deps) {
        if (-not ($dep.dep_kinds | Where-Object { $null -eq $_.kind })) { continue }
        if (-not $seen.ContainsKey($dep.pkg)) { $seen[$dep.pkg] = $true; $queue.Enqueue($dep.pkg) }
    }
}
$rust = Join-Path $dist 'licenses\rust'
if (Test-Path $rust) { Remove-Item $rust -Recurse -Force }
$summary = foreach ($id in $seen.Keys) {
    $p = $byId[$id]
    $dest = Join-Path $rust "$($p.name)-$($p.version)"
    New-Item -ItemType Directory -Force $dest | Out-Null
    $files = Get-ChildItem (Split-Path $p.manifest_path) -File | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)' }
    if ($files) { $files | Copy-Item -Destination $dest }
    else {
        Set-Content (Join-Path $dest 'LICENSE-NOTE.txt') -Encoding utf8 ("$($p.name) $($p.version)`r`nLicense: $($p.license)`r`n" +
            "Authors: $($p.authors -join ', ')`r`nSource: $($p.repository)`r`nThe crate ships no license file; the license above applies.")
    }
    '{0} {1}: {2}' -f $p.name, $p.version, $p.license
}
$summary | Sort-Object | Set-Content (Join-Path $rust 'crates.txt') -Encoding utf8
"Rust crates in the engine: $($seen.Count)"
Get-ChildItem $dist -File | Select-Object Name, @{ n = 'KB'; e = { [int]($_.Length / 1kb) } } | Format-Table -AutoSize
