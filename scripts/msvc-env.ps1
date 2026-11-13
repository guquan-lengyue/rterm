# Load MSVC build environment for the current PowerShell session (Windows + VS Build Tools).
# Usage: . scripts/msvc-env.ps1

$vsInstallDir = Get-ChildItem "C:\Program Files*\Microsoft Visual Studio\*\*\VC\Tools\MSVC" -Directory -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName

if (-not $vsInstallDir) { Write-Error "MSVC not found"; return }

$vsRoot     = Split-Path (Split-Path (Split-Path $vsInstallDir)) -Parent
$toolBin    = Join-Path $vsInstallDir "bin\Hostx64\x64"
$toolInclude = Join-Path $vsInstallDir "include"
$toolLib    = Join-Path $vsInstallDir "lib\x64"

$cmakeBin = (Get-ChildItem "$vsRoot\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty DirectoryName)

# SDK
$ucrtInclude = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\Include" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
$ucrtLib = (Join-Path $ucrtInclude "..\Lib")

$env:PATH = @($toolBin, $cmakeBin, $env:PATH) -join ";"
$env:INCLUDE = "$toolInclude;$ucrtInclude\ucrt;$ucrtInclude\um;$ucrtInclude\shared"
$env:LIB     = "$toolLib;$ucrtLib\$((Split-Path $ucrtInclude -Leaf))\ucrt\x64;$ucrtLib\$((Split-Path $ucrtInclude -Leaf))\um\x64"

Write-Host "[msvc-env] cl.exe   =" (Get-Command cl -ErrorAction SilentlyContinue).Source
Write-Host "[msvc-env] cmake   =" (Get-Command cmake -ErrorAction SilentlyContinue).Source
