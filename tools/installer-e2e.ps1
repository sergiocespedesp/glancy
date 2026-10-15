param([Parameter(Mandatory)] [string]$Setup)
# End-to-end test of the setup on the real system: silent install, upgrade while a process holds the engine DLL
# (what Explorer does), silent uninstall with the DLL still loaded, and a final install that leaves Glancy installed.
# Run it in a process that sees the user's real registry (in the Claude desktop app: a .cmd started by explorer.exe).
$ErrorActionPreference = 'Stop'
$Setup = (Resolve-Path $Setup).Path
$app = Join-Path $env:LOCALAPPDATA 'Programs\Glancy'
$dll = Join-Path $app 'Engine\Glancy.Thumbnails.dll'
$clsid = '{6C3A8B2E-5F1D-4E7A-9B0C-2D8E4F61A7C5}'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{57BE6CE7-E3A9-409E-9D9B-3C6BE3EAB064}_is1'
$runOnce = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\RunOnce'
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Glancy.lnk'
$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Glancy.lnk'
$logs = Join-Path ([IO.Path]::GetTempPath()) 'glancy-installer-e2e'
New-Item -ItemType Directory -Force $logs | Out-Null
$script:fails = 0
function Check($name, [bool]$ok, $detail = '') {
    if ($ok) { "PASS  $name" } else { "FAIL  $name  $detail"; $script:fails++ }
}

Add-Type -Namespace GlancyE2E -Name Shell -MemberDefinition @'
[DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
public static extern int AssocQueryStringW(int flags, int str, string assoc, string extra, System.Text.StringBuilder output, ref uint length);
'@
# The thumbnail provider Explorer resolves for an extension (ASSOCSTR_SHELLEXTENSION with IThumbnailProvider).
function ThumbnailProvider([string]$ext) {
    $sb = New-Object System.Text.StringBuilder 128
    [uint32]$len = 128
    $hr = [GlancyE2E.Shell]::AssocQueryStringW(0, 16, $ext, '{e357fccd-a995-4576-b01f-234630154e96}', $sb, [ref]$len)
    if ($hr -eq 0) { $sb.ToString() } else { '' }
}
function RegisteredDll {
    $k = Get-Item "HKCU:\Software\Classes\CLSID\$clsid\InprocServer32" -ErrorAction SilentlyContinue
    if ($k) { $k.GetValue('') } else { $null }
}
function RunSetup([string]$file, [string]$log) {
    $p = Start-Process $file -ArgumentList "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$(Join-Path $logs $log)`"" -Wait -PassThru
    $p.ExitCode
}
# A process that keeps the engine DLL loaded, as Explorer does after showing a thumbnail.
function Start-Holder([string]$path) {
    $script = "Add-Type -Namespace H -Name K -MemberDefinition '[DllImport(""kernel32.dll"", CharSet = CharSet.Unicode)] public static extern IntPtr LoadLibraryW(string p);'; " +
        "if ([H.K]::LoadLibraryW('$path') -eq [IntPtr]::Zero) { exit 1 }; Start-Sleep 600"
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
    $p = Start-Process powershell -ArgumentList "-NoProfile -EncodedCommand $encoded" -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 4
    $p
}

Get-Process Glancy -ErrorAction SilentlyContinue | Stop-Process -Force

# 1. Install
"== install"
Check 'setup exit code 0' ((RunSetup $Setup 'install.log') -eq 0)
Check 'app files' (Test-Path (Join-Path $app 'Glancy.exe'))
Check 'engine files' ((Test-Path $dll) -and (Test-Path (Join-Path $app 'Engine\pdfium.dll')))
Check 'Start menu shortcut' (Test-Path $startMenu)
Check 'no desktop shortcut by default' (-not (Test-Path $desktop))
Check 'uninstall entry' (Test-Path $uninstallKey)
Check 'CLSID points to the installed DLL' ((RegisteredDll) -eq $dll) (RegisteredDll)
$notResolved = (Get-ItemProperty 'HKCU:\Software\Glancy\Engine' -ErrorAction SilentlyContinue).NotResolved
Check 'every extension resolves to Glancy (NotResolved empty)' ([string]::IsNullOrEmpty($notResolved)) $notResolved
Check 'Explorer resolves .psd to Glancy' ((ThumbnailProvider '.psd') -eq $clsid) (ThumbnailProvider '.psd')
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'engine-e2e.ps1') -Engine (Join-Path $app 'Engine') | Out-Host
Check 'engine-e2e against the installed engine' ($LASTEXITCODE -eq 0)

# 2. Upgrade while the DLL is loaded
"== upgrade with the engine loaded"
$holder = Start-Holder $dll
Check 'holder loaded the DLL' (-not $holder.HasExited)
Check 'setup exit code 0' ((RunSetup $Setup 'upgrade.log') -eq 0)
Check 'in-use DLL renamed to *.old-*' ((Get-ChildItem (Join-Path $app 'Engine') -Filter 'Glancy.Thumbnails.dll.old-*').Count -ge 1)
Check 'new DLL in place and registered' ((Test-Path $dll) -and (RegisteredDll) -eq $dll)
Stop-Process -Id $holder.Id -Force -ErrorAction SilentlyContinue

# 3. Uninstall while the DLL is loaded
"== uninstall with the engine loaded"
New-Item -Force 'HKCU:\Software\Glancy\App' | Out-Null
New-ItemProperty 'HKCU:\Software\Glancy\App' E2EMarker -Value 1 -PropertyType DWord -Force | Out-Null
New-Item -Force 'HKCU:\Software\Classes\AppUserModelId\Glancy' | Out-Null
$holder = Start-Holder $dll
Start-Process (Join-Path $app 'unins000.exe') -ArgumentList "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$(Join-Path $logs 'uninstall.log')`"" | Out-Null
# The uninstaller relaunches itself from the temp folder, so wait for its entry to disappear instead of the process.
for ($i = 0; $i -lt 120 -and (Test-Path $uninstallKey); $i++) { Start-Sleep -Seconds 1 }
Start-Sleep -Seconds 3
Check 'uninstall entry removed' (-not (Test-Path $uninstallKey))
Check 'CLSID removed' ($null -eq (RegisteredDll))
Check 'engine backups key removed' (-not (Test-Path 'HKCU:\Software\Glancy\Engine'))
Check 'Explorer no longer resolves .psd to Glancy' ((ThumbnailProvider '.psd') -ne $clsid) (ThumbnailProvider '.psd')
Check 'settings kept (silent uninstall)' ((Get-ItemProperty 'HKCU:\Software\Glancy\App' -ErrorAction SilentlyContinue).E2EMarker -eq 1)
Check 'notification identity removed' (-not (Test-Path 'HKCU:\Software\Classes\AppUserModelId\Glancy'))
Check 'Start menu shortcut removed' (-not (Test-Path $startMenu))
Check 'app folder removed' (-not (Test-Path $app)) ((Get-ChildItem $app -Recurse -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName) -join ', ')
$cleanup = (Get-ItemProperty $runOnce -ErrorAction SilentlyContinue).GlancyCleanup
$leftovers = if ($cleanup -match '"([^"]+)"') { $Matches[1] } else { '' }
Check 'locked DLL moved aside and scheduled for deletion' ($leftovers -and (Test-Path (Join-Path $leftovers 'Glancy.Thumbnails.dll'))) $cleanup
Stop-Process -Id $holder.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
if ($leftovers) { Remove-Item $leftovers -Recurse -Force -ErrorAction SilentlyContinue }
Remove-ItemProperty $runOnce GlancyCleanup -ErrorAction SilentlyContinue
Remove-ItemProperty 'HKCU:\Software\Glancy\App' E2EMarker -ErrorAction SilentlyContinue

# 4. Install again, leaving the machine with Glancy installed
"== install again"
Check 'setup exit code 0' ((RunSetup $Setup 'reinstall.log') -eq 0)
Check 'CLSID points to the installed DLL' ((RegisteredDll) -eq $dll) (RegisteredDll)
Check 'Explorer resolves .psd to Glancy' ((ThumbnailProvider '.psd') -eq $clsid) (ThumbnailProvider '.psd')

""
"Setup logs: $logs"
if ($script:fails -eq 0) { "All checks passed." } else { "$($script:fails) checks failed."; exit 1 }
