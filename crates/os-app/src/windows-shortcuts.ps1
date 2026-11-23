function Repair-MLStudioShortcuts($install, $target, $programs, $desktop) {
    $old = [IO.Path]::Combine($install, 'lemonstudio.exe')
    $new = [IO.Path]::Combine($install, 'ML Studio.exe')
    if ($target -notin @($old, $new) -or !(Test-Path -LiteralPath $target -PathType Leaf)) { return }

    $shell = New-Object -ComObject WScript.Shell
    $startLink = Join-Path $programs 'ML Studio\ML Studio.lnk'
    $desktopLink = Join-Path $desktop 'ML Studio.lnk'
    $legacyLink = Join-Path $desktop 'Lemon Studio.lnk'
    if (!(Test-Path -LiteralPath $desktopLink) -and (Test-Path -LiteralPath $legacyLink -PathType Leaf)) {
        $previous = $shell.CreateShortcut($legacyLink)
        if ($previous.TargetPath -in @($old, $new)) { $desktopLink = $legacyLink }
    }

    $changed = @()
    foreach ($path in @($startLink, $desktopLink)) {
        $exists = Test-Path -LiteralPath $path
        if ($exists -and !(Test-Path -LiteralPath $path -PathType Leaf)) { continue }
        $link = $shell.CreateShortcut($path)
        if ($exists -and $link.TargetPath -notin @($old, $new)) { continue }
        $touched = !$exists -or $link.TargetPath -ne $target
        if ($touched) {
            $link.TargetPath = $target
            $link.WorkingDirectory = $install
        }
        $iconPath = ($link.IconLocation -replace ',\s*-?\d+\s*$', '').Trim(' ', '"')
        if (!$iconPath -or $iconPath -eq $old) {
            $link.IconLocation = "$target,0"
            $touched = $true
        }
        if (!$touched) { continue }
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path)) | Out-Null
        $link.Save()
        $changed += $path
    }
    return $changed
}

function Update-MLStudioShortcutIcons($paths) {
    if (!$paths) { return }
    Add-Type -Namespace MLStudio -Name Shell -MemberDefinition @"
[System.Runtime.InteropServices.DllImport("shell32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern void SHChangeNotify(int change, uint flags, string path, System.IntPtr item2);
"@
    # SHCNE_UPDATEITEM, SHCNF_PATHW. Refresh only the shortcuts we changed.
    foreach ($path in $paths) { [MLStudio.Shell]::SHChangeNotify(0x2000, 5, $path, [IntPtr]::Zero) }
}

function Repair-RegisteredMLStudioShortcuts {
    $ErrorActionPreference = 'Stop'
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\LemonStudio-MichaelLane'
    $install = (Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue).InstallLocation
    if (!$install) { return }
    $install = [IO.Path]::GetFullPath($install)
    $target = [IO.Path]::GetFullPath($env:ML_SHORTCUT_TARGET)
    $old = [IO.Path]::Combine($install, 'lemonstudio.exe')
    $new = [IO.Path]::Combine($install, 'ML Studio.exe')
    if ($target -notin @($old, $new) -or !(Test-Path -LiteralPath $target -PathType Leaf)) { return }
    $changed = Repair-MLStudioShortcuts $install $target ([Environment]::GetFolderPath('Programs')) ([Environment]::GetFolderPath('DesktopDirectory'))
    Update-MLStudioShortcutIcons $changed
    if ($target -eq $new) {
        Set-ItemProperty -LiteralPath $key -Name DisplayVersion -Value $env:ML_SHORTCUT_VERSION
        Set-ItemProperty -LiteralPath $key -Name ShortcutRepairBuild -Value $env:ML_SHORTCUT_BUILD
    }
}
