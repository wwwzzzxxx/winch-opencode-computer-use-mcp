# Build winch-mcp with the rustup GNU toolchain (no MSVC SDK on this machine).
$ErrorActionPreference = "Stop"
$tc = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained"
if (-not (Test-Path $tc)) { throw "GNU toolchain not found: $tc" }
$env:PATH = "$tc;$env:PATH"
$env:RUSTFLAGS = "-C link-self-contained=yes"
Push-Location $PSScriptRoot
try {
    cargo +stable-x86_64-pc-windows-gnu build --release $args
    Write-Host "built: $PSScriptRoot\target\release\winch-mcp.exe"
} finally {
    Pop-Location
}
