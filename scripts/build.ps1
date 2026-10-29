$ErrorActionPreference = 'Stop'
Set-Location "$PSScriptRoot\.."
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
cargo build --release --workspace --locked
if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
python scripts/package.py
if ($LASTEXITCODE -ne 0) { throw 'Packaging failed' }
