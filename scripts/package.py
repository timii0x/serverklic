#!/usr/bin/env python3
"""Package binaries built on the current OS; no cross-OS binaries are fabricated."""
import os, platform, plistlib, shutil, subprocess, pathlib, zipfile
root = pathlib.Path(__file__).resolve().parents[1]
system = platform.system()
arch = platform.machine().lower()
label = {'Darwin': 'macos', 'Windows': 'windows', 'Linux': 'linux'}[system] + '-' + arch
out = root / 'dist' / ('ServerKlic-' + label)
out.mkdir(parents=True, exist_ok=True)
target = pathlib.Path(os.environ.get('CARGO_TARGET_DIR', str(root / 'target'))) / 'release'
ext = '.exe' if system == 'Windows' else ''
shutil.copy2(target / ('serverklic-server' + ext), out)
shutil.copy2(root / 'README.md', out / 'PREBERI.md')
if system == 'Darwin':
    app = out / 'ServerKlic.app'
    macos = app / 'Contents' / 'MacOS'
    macos.mkdir(parents=True, exist_ok=True)
    shutil.copy2(target / 'serverklic', macos / 'serverklic')
    info = dict(CFBundleName='ServerKlic', CFBundleDisplayName='ServerKlic', CFBundleIdentifier='si.serverklic.desktop', CFBundleVersion='1', CFBundleShortVersionString='0.1.0', CFBundleExecutable='serverklic', CFBundlePackageType='APPL', NSHighResolutionCapable=True, LSMinimumSystemVersion='11.0', NSMicrophoneUsageDescription='ServerKlic uporablja mikrofon samo za glasovni klic na tvojem strežniku.')
    with (app / 'Contents' / 'Info.plist').open('wb') as f: plistlib.dump(info, f)
    for attr in ('com.apple.FinderInfo', 'com.apple.ResourceFork'):
        subprocess.run(['xattr', '-dr', attr, str(app)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(app)], check=True)
    launcher = out / 'Zazeni-lokalni-test.command'
    launcher.write_text('''#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
DATA="$HOME/Library/Application Support/ServerKlic/local-server"
mkdir -p "$DATA"
./serverklic-server --bind 127.0.0.1:5000 --data-dir "$DATA" &
PID=$!
trap 'kill "$PID" 2>/dev/null || true' EXIT INT TERM
sleep 1
kill -0 "$PID" || { echo "Strežnik se ni zagnal (je port 5000 že zaseden?)"; read -r; exit 1; }
open -n ServerKlic.app
echo "V aplikaciji vpiši ime, naslov 127.0.0.1:5000 in potrdi odtis."
echo "ENTER ustavi lokalni strežnik. Za dva odjemalca ponovno odpri ServerKlic.app."
read -r
''')
    launcher.chmod(0o755)
elif system == 'Windows':
    shutil.copy2(target / 'serverklic.exe', out)
    (out / 'Zazeni-lokalni-test.bat').write_text('''@echo off
cd /d "%~dp0"
start "ServerKlic server - close to stop" serverklic-server.exe --bind 127.0.0.1:5000 --data-dir "%LOCALAPPDATA%\\ServerKlic\\local-server"
timeout /t 2 >nul
start "ServerKlic" serverklic.exe
''')
else:
    shutil.copy2(target / 'serverklic', out)
    shutil.copy2(root / 'serverklic.sh', out)
    launcher = out / 'zazeni-lokalni-test.sh'
    launcher.write_text('''#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
./serverklic-server --bind 127.0.0.1:5000 --data-dir "${XDG_DATA_HOME:-$HOME/.local/share}/serverklic/local-server" &
PID=$!
trap 'kill "$PID" 2>/dev/null || true' EXIT INT TERM
sleep 1
kill -0 "$PID"
./serverklic
''')
    launcher.chmod(0o755)
archive = out.with_suffix('.zip')
with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as z:
    for f in sorted(out.rglob('*')): z.write(f, f.relative_to(out.parent))
print(archive)
