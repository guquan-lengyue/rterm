# Local quality gate: fmt check + clippy + tests (exit code based).
# Usage: powershell -ExecutionPolicy Bypass -File scripts/check.ps1
# PowerShell 5 compatible (ASCII only, no PS7 syntax).
# NOTE: clippy does not use -D warnings here; existing warnings are reported
# but only a non-zero cargo exit code fails the gate.

$ErrorActionPreference = 'Stop'

# workspace root = parent of this scripts/ directory
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 不自动 dot-source msvc-env.ps1：切换 VS 环境会让 vendored OpenSSL 全量重编译。
# 如果当前 shell 找不到 MSVC/链接器，请先手动执行：
#   . .\scripts\msvc-env.ps1
# 再运行本脚本。

$steps = @(
    @{ Name = 'cargo fmt --all -- --check';                            Args = @('fmt', '--all', '--', '--check') },
    @{ Name = 'cargo clippy --workspace --all-targets';               Args = @('clippy', '--workspace', '--all-targets') },
    @{ Name = 'cargo test --workspace';                                Args = @('test', '--workspace') }
)

foreach ($step in $steps) {
    Write-Host ''
    Write-Host "[check] running: $($step.Name)" -ForegroundColor Cyan
    $cargoArgs = $step.Args
    & cargo $cargoArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[check] FAILED step: $($step.Name) (exit code $LASTEXITCODE)" -ForegroundColor Red
        exit 1
    }
}

Write-Host ''
Write-Host '[check] all steps passed' -ForegroundColor Green
exit 0
