param([switch]$SkipEngine)
# Builds dist/Installer/GlancySetup-<version>.exe: the engine (tools/build-engine.ps1, with its tests), the app
# published self-contained, and installer/Glancy.iss compiled with Inno Setup 6.
# Code signing goes here once there is a certificate: sign Glancy.exe, Glancy.Thumbnails.dll and glancy-thumb.exe
# before ISCC, and the setup after it (signtool sign /fd sha256 /tr <timestamp server> /td sha256 …).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$out = Join-Path $root 'dist\Installer'
$app = Join-Path $out 'app'
$engine = Join-Path $root 'dist\Engine'

$iscc = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found: install it with "winget install --id JRSoftware.InnoSetup --scope user".' }

$dotnet = Join-Path $env:LOCALAPPDATA 'Microsoft\dotnet\dotnet.exe'
if (-not (Test-Path $dotnet)) { $dotnet = 'dotnet' }

[xml]$props = Get-Content (Join-Path $root 'Directory.Build.props')
$version = ($props.Project.PropertyGroup | ForEach-Object { $_.Version } | Where-Object { $_ } | Select-Object -First 1)
if (-not $version) { throw 'No <Version> in Directory.Build.props.' }

if (-not $SkipEngine) { & (Join-Path $PSScriptRoot 'build-engine.ps1') }
if (-not (Test-Path (Join-Path $engine 'Glancy.Thumbnails.dll'))) { throw "No engine build in $engine (run tools/build-engine.ps1)." }

if (Test-Path $app) { Remove-Item $app -Recurse -Force }
& $dotnet publish (Join-Path $root 'src\Glancy.Settings\Glancy.Settings.csproj') -c Release -p:Platform=x64 -r win-x64 `
    --self-contained true -p:PublishReadyToRun=true -o $app
if ($LASTEXITCODE -ne 0) { throw 'Publishing the app failed.' }

# The license texts the self-contained app must carry: the .NET runtime it includes and the Windows App SDK packages.
$nuget = if ($env:NUGET_PACKAGES) { $env:NUGET_PACKAGES } else { Join-Path $env:USERPROFILE '.nuget\packages' }
$runtime = Get-Content (Join-Path $app 'Glancy.runtimeconfig.json') -Raw | ConvertFrom-Json
$packages = @($runtime.runtimeOptions.includedFrameworks | ForEach-Object { "$($_.name).Runtime.win-x64/$($_.version)" })
$assets = Get-Content (Join-Path $env:LOCALAPPDATA 'Temp\lmb\obj\Glancy.Settings\project.assets.json') -Raw | ConvertFrom-Json
$packages += @($assets.libraries.PSObject.Properties.Name | Where-Object { $_ -like 'Microsoft.WindowsAppSDK.*' })
foreach ($package in $packages) {
    $id, $packageVersion = $package -split '/'
    $files = Get-ChildItem (Join-Path $nuget "$($id.ToLowerInvariant())\$packageVersion") -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^(license|third-party-notices|notice)' }
    if (-not $files) { throw "No license files for $id $packageVersion in the NuGet cache." }
    $dest = Join-Path $app "licenses\$id $packageVersion"
    New-Item -ItemType Directory -Force $dest | Out-Null
    $files | Copy-Item -Destination $dest
}

& $iscc /Q "/DAppVersion=$version" "/DAppDir=$app" "/DEngineDir=$engine" "/O$out" (Join-Path $root 'installer\Glancy.iss')
if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed.' }

$setup = Get-Item (Join-Path $out "GlancySetup-$version.exe")
'{0} ({1:N1} MB)' -f $setup.FullName, ($setup.Length / 1MB)
