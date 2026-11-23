param([Parameter(Mandatory)][string]$Executable, [Parameter(Mandatory)][string]$Scratch)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/../../crates/os-app/src/windows-shortcuts.ps1"

$install = Split-Path -Parent $Executable
$legacy = Join-Path $install 'lemonstudio.exe'
$shell = New-Object -ComObject WScript.Shell

function Add-TestLink($path, $target, $icon) {
    [IO.Directory]::CreateDirectory((Split-Path -Parent $path)) | Out-Null
    $link = $shell.CreateShortcut($path)
    $link.TargetPath = $target
    $link.IconLocation = "$icon,0"
    $link.Save()
}

function Assert-Link($path, $target, $icon) {
    if (!(Test-Path -LiteralPath $path -PathType Leaf)) { throw "Shortcut was not created: $path" }
    $link = $shell.CreateShortcut($path)
    if ($link.TargetPath -ne $target -or ($link.IconLocation -replace ',\s*0$', '') -ne $icon) {
        throw "Unexpected target or icon in $path ($($link.TargetPath), $($link.IconLocation))"
    }
}

$start = Join-Path $Scratch 'Programs'
$desktop = Join-Path $Scratch 'Desktop'
$startLink = Join-Path $start 'ML Studio\ML Studio.lnk'
$desktopLink = Join-Path $desktop 'ML Studio.lnk'
Add-TestLink $startLink $Executable $legacy
$changed = Repair-MLStudioShortcuts $install $Executable $start $desktop
if (@($changed).Count -ne 2) { throw 'Expected to repair the stale Start icon and recreate the missing desktop shortcut' }
Update-MLStudioShortcutIcons $changed
Assert-Link $startLink $Executable $Executable
Assert-Link $desktopLink $Executable $Executable
if (Repair-MLStudioShortcuts $install $Executable $start $desktop) { throw 'Second repair rewrote healthy shortcuts' }

$previousDesktop = Join-Path $Scratch 'Legacy Desktop'
$oldName = Join-Path $previousDesktop 'Lemon Studio.lnk'
Add-TestLink $oldName $legacy $legacy
$null = Repair-MLStudioShortcuts $install $Executable $start $previousDesktop
Assert-Link $oldName $Executable $Executable
if (Test-Path -LiteralPath (Join-Path $previousDesktop 'ML Studio.lnk')) { throw 'Duplicated an existing legacy desktop shortcut' }

$custom = Join-Path $Scratch 'Custom Desktop'
$unrelated = Join-Path $custom 'ML Studio.lnk'
$unrelatedTarget = Join-Path $PSHOME 'powershell.exe'
Add-TestLink $unrelated $unrelatedTarget $legacy
$null = Repair-MLStudioShortcuts $install $Executable $start $custom
Assert-Link $unrelated $unrelatedTarget $legacy
Add-TestLink $startLink $Executable $unrelatedTarget
$null = Repair-MLStudioShortcuts $install $Executable $start $custom
Assert-Link $startLink $Executable $unrelatedTarget
Write-Output 'Verified missing desktop, stale Start icon, legacy desktop name, shell notification, idempotency, and custom shortcut preservation.'
