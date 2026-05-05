param(
    [string]$SourceDir,
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\Lazyterm",
    [switch]$NoDesktopShortcut,
    [switch]$SkipPath,
    [switch]$Launch
)

$ErrorActionPreference = "Stop"

function Resolve-LazytermSource {
    param([string]$RequestedSource)

    $scriptDir = $PSScriptRoot
    $candidates = @()
    if ($RequestedSource) {
        $candidates += $RequestedSource
    } else {
        $candidates += $scriptDir
        $candidates += Join-Path $scriptDir "..\target\release"
        $candidates += Join-Path $scriptDir "..\target\debug"
    }

    foreach ($candidate in $candidates) {
        $resolved = Resolve-Path $candidate -ErrorAction SilentlyContinue
        if (-not $resolved) {
            continue
        }

        $path = $resolved.Path
        if ((Test-Path (Join-Path $path "lazyterm.exe")) -and (Test-Path (Join-Path $path "lazytermctl.exe"))) {
            return $path
        }
    }

    throw "Could not find lazyterm.exe and lazytermctl.exe. Build first or pass -SourceDir."
}

function New-Shortcut {
    param(
        [string]$Path,
        [string]$TargetPath,
        [string]$WorkingDirectory,
        [string]$Description
    )

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($Path)
    $shortcut.TargetPath = $TargetPath
    $shortcut.WorkingDirectory = $WorkingDirectory
    $shortcut.Description = $Description
    $shortcut.IconLocation = "$TargetPath,0"
    $shortcut.Save()
}

function Add-UserPath {
    param([string]$PathToAdd)

    $environmentKey = "HKCU:\Environment"
    $currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if ($currentPath) {
        $parts = $currentPath -split ";" | Where-Object { $_ }
    }

    if ($parts -contains $PathToAdd) {
        return
    }

    $nextPath = (@($parts) + $PathToAdd) -join ";"
    Set-ItemProperty -Path $environmentKey -Name Path -Value $nextPath
    [Environment]::SetEnvironmentVariable("Path", $nextPath, "User")
    $env:Path = "$env:Path;$PathToAdd"
}

$source = Resolve-LazytermSource -RequestedSource $SourceDir
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..") -ErrorAction SilentlyContinue
$install = New-Item -ItemType Directory -Force -Path $InstallDir

Copy-Item -Path (Join-Path $source "lazyterm.exe") -Destination $install.FullName -Force
Copy-Item -Path (Join-Path $source "lazytermctl.exe") -Destination $install.FullName -Force

foreach ($file in @("README.md", "LICENSE")) {
    $candidate = Join-Path $source $file
    if ((-not (Test-Path $candidate)) -and $repoRoot) {
        $candidate = Join-Path $repoRoot.Path $file
    }
    if (Test-Path $candidate) {
        Copy-Item -Path $candidate -Destination $install.FullName -Force
    }
}

$appExe = Join-Path $install.FullName "lazyterm.exe"
$startMenuDir = New-Item -ItemType Directory -Force -Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Lazyterm"
New-Shortcut `
    -Path (Join-Path $startMenuDir.FullName "Lazyterm.lnk") `
    -TargetPath $appExe `
    -WorkingDirectory $install.FullName `
    -Description "Lazyterm"

if (-not $NoDesktopShortcut) {
    New-Shortcut `
        -Path (Join-Path ([Environment]::GetFolderPath("Desktop")) "Lazyterm.lnk") `
        -TargetPath $appExe `
        -WorkingDirectory $install.FullName `
        -Description "Lazyterm"
}

if (-not $SkipPath) {
    Add-UserPath -PathToAdd $install.FullName
}

if ($Launch) {
    Start-Process -FilePath $appExe -WorkingDirectory $install.FullName -WindowStyle Hidden
}

Write-Output "Installed Lazyterm to $($install.FullName)"
