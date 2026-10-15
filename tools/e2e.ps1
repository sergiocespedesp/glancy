param([Parameter(Mandatory)] [string]$Exe)
# End-to-end test of Glancy through UI Automation patterns (no mouse/keyboard injection).
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$script:fails = 0
function Check($name, [bool]$ok, $detail = '') {
    if ($ok) { "PASS  $name" } else { "FAIL  $name  $detail"; $script:fails++ }
}
function Find($root, $name, $timeoutMs = 6000, $type = $null) {
    # Matches the exact name or the name plus the app's " (pending change)" suffix.
    $cond = if ($type) { New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $type) } else { [System.Windows.Automation.Condition]::TrueCondition }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        foreach ($el in $root.FindAll($TS::Descendants, $cond)) {
            $n = $el.Current.Name
            if ($n -eq $name -or $n.StartsWith("$name (")) { return $el }
        }
        Start-Sleep -Milliseconds 200
    }
    return $null
}
function Invoke($el) { $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke() }
function SelectComboItem($root, $comboName, $itemName) {
    $combo = Find $root $comboName 6000 ([System.Windows.Automation.ControlType]::ComboBox)
    if (-not $combo) { throw "combo '$comboName' not found" }
    $combo.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand()
    Start-Sleep -Milliseconds 500
    $item = Find $AE::RootElement $itemName 4000 ([System.Windows.Automation.ControlType]::ListItem)
    if (-not $item) { throw "item '$itemName' not found" }
    $item.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
    Start-Sleep -Milliseconds 300
    try { $combo.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Collapse() } catch {}
    Start-Sleep -Milliseconds 400
}
function ComboValue($root, $comboName) {
    $combo = Find $root $comboName 6000 ([System.Windows.Automation.ControlType]::ComboBox)
    $sel = $combo.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern).Current.GetSelection()
    if ($sel.Count -gt 0) { $sel[0].Current.Name } else { '' }
}
function Reg($sub, $name) {
    $k = Get-Item "HKCU:\Software\Glancy\Thumbnails\$sub" -ErrorAction SilentlyContinue
    if ($k) { $k.GetValue($name) } else { $null }
}
function WaitWindow($proc) {
    for ($i = 0; $i -lt 160; $i++) { Start-Sleep -Milliseconds 250; $proc.Refresh(); if ($proc.MainWindowHandle -ne 0) { break } }
    Start-Sleep -Milliseconds 2500
    $AE::FromHandle($proc.MainWindowHandle)
}

Get-Process Glancy -ErrorAction SilentlyContinue | ForEach-Object { $_.CloseMainWindow() | Out-Null; Start-Sleep 1; if (-not $_.HasExited) { $_.Kill() } }
# English UI whatever the Windows language: the control names below are the English ones.
$p = Start-Process $Exe -ArgumentList '--page maintenance --lang en' -PassThru
$win = WaitWindow $p
Check 'window open' ($win -ne $null)

# 0. Known starting point: factory defaults, through the app's own "Restore" + "Apply".
Invoke (Find $win 'Restore defaults' 6000 ([System.Windows.Automation.ControlType]::Button))
Start-Sleep -Milliseconds 1200
Invoke (Find $win 'Restore' 4000 ([System.Windows.Automation.ControlType]::Button))
Start-Sleep -Milliseconds 800
$apply0 = Find $win 'Apply changes' 2000 ([System.Windows.Automation.ControlType]::Button)
if ($apply0 -and -not $apply0.Current.IsOffscreen) { Invoke $apply0; Start-Sleep -Milliseconds 2500 }
$tip = Find $win 'Got it' 1000 ([System.Windows.Automation.ControlType]::Button)
if ($tip) { Invoke $tip }
# Formats are nested in category items: expand "Design and editing", then select Photoshop.
$category = Find $win 'Design and editing' 4000 ([System.Windows.Automation.ControlType]::ListItem)
$category.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand()
Start-Sleep -Milliseconds 600
$nav = Find $win 'Adobe Photoshop' 4000 ([System.Windows.Automation.ControlType]::ListItem)
Check 'navigation: Photoshop inside its category' ($nav -ne $null)
$nav.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
Start-Sleep -Milliseconds 1500
$revision0 = [int](Reg '' 'Revision')

# 1. Change a setting -> pending bar
$transparency = 'Background of transparent areas'
SelectComboItem $win $transparency 'Checkerboard'
Check 'transparency = Checkerboard' ((ComboValue $win $transparency) -eq 'Checkerboard') (ComboValue $win $transparency)
Check 'bar: 1 pending change' ((Find $win '1 pending change' 3000) -ne $null)
Check 'registry untouched before applying' ([int](Reg '' 'Revision') -eq $revision0)

# 2. Undo
Invoke (Find $win 'Undo the last change' 6000 ([System.Windows.Automation.ControlType]::Button))
Start-Sleep -Milliseconds 800
Check 'undo restores Transparent' ((ComboValue $win $transparency) -eq 'Transparent') (ComboValue $win $transparency)
Check 'undo hides the bar' ((Find $win '1 pending change' 1500) -eq $null)

# 3. Apply -> registry
SelectComboItem $win $transparency 'Checkerboard'
Invoke (Find $win 'Apply changes' 6000 ([System.Windows.Automation.ControlType]::Button))
Start-Sleep -Milliseconds 2500
Check 'registry: psd transparency = 3' ((Reg 'Formats\psd' 'transparency') -eq 3) (Reg 'Formats\psd' 'transparency')
Check 'registry: Revision incremented' ([int](Reg '' 'Revision') -gt $revision0)
Check 'bar hidden after applying' ((Find $win '1 pending change' 1500) -eq $null)

# 4. Extension scope (.psb) -> override only for .psb
$psb = Find $win '.psb' 6000 ([System.Windows.Automation.ControlType]::ListItem)
if (-not $psb) { $psb = Find $win '.psb' 6000 ([System.Windows.Automation.ControlType]::TabItem) }
$psb.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
Start-Sleep -Milliseconds 800
SelectComboItem $win 'Maximum file size' '1 GB'
Invoke (Find $win 'Apply changes' 6000 ([System.Windows.Automation.ControlType]::Button))
Start-Sleep -Milliseconds 2500
Check 'registry: .psb maxFileSizeMB = 1024' ((Reg 'Formats\psd\Extensions\.psb' 'maxFileSizeMB') -eq 1024) (Reg 'Formats\psd\Extensions\.psb' 'maxFileSizeMB')
Check 'registry: psd maxFileSizeMB still 0' ((Reg 'Formats\psd' 'maxFileSizeMB') -eq 0)
Check 'tab marked (customized)' ((Find $win '.psb (customized)' 2000) -ne $null)

# 5. Single instance: second launch navigates the running window
$p2 = Start-Process $Exe -ArgumentList '--page maintenance --lang en' -PassThru
$p2.WaitForExit(15000) | Out-Null
Start-Sleep -Milliseconds 1500
Check 'second instance redirects and exits' ($p2.HasExited -and (Get-Process Glancy).Count -eq 1)
Check 'existing window navigated to Maintenance' ((Find $win 'Clear the thumbnail cache' 5000) -ne $null)

# 6. Restore defaults (confirmation dialog) + apply
Invoke (Find $win 'Restore defaults' 6000 ([System.Windows.Automation.ControlType]::Button))
Start-Sleep -Milliseconds 1200
$dialogButton = Find $win 'Restore' 4000 ([System.Windows.Automation.ControlType]::Button)
Check 'confirmation dialog' ($dialogButton -ne $null)
Invoke $dialogButton
Start-Sleep -Milliseconds 800
Invoke (Find $win 'Apply changes' 6000 ([System.Windows.Automation.ControlType]::Button))
Start-Sleep -Milliseconds 2500
Check 'registry: psd transparency back to 2' ((Reg 'Formats\psd' 'transparency') -eq 2) (Reg 'Formats\psd' 'transparency')
Check 'registry: .psb override removed' ((Get-Item 'HKCU:\Software\Glancy\Thumbnails\Formats\psd\Extensions\.psb' -ErrorAction SilentlyContinue) -eq $null)

# 7. Close with pending changes -> dialog -> close without applying
(Find $win 'File Explorer' 6000 ([System.Windows.Automation.ControlType]::ListItem)).GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
Start-Sleep -Milliseconds 1200
SelectComboItem $win 'Glancy context menu' 'Off'
$win.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern).Close()
$closeWithout = Find $win 'Close without applying' 5000 ([System.Windows.Automation.ControlType]::Button)
Check 'dialog when closing with changes' ($closeWithout -ne $null)
if ($closeWithout) { Invoke $closeWithout }
$p.WaitForExit(10000) | Out-Null
Check 'the app closes' $p.HasExited
Check 'registry: unapplied change not saved' ((Reg 'Global' 'contextMenu') -eq 2) (Reg 'Global' 'contextMenu')

if (-not $p.HasExited) { $p.Kill() }
"---- failures: $script:fails"
