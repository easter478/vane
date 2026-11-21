#!/bin/zsh
# Build Vane on a Windows PC over SSH and install it for the signed-in user.
#   tools/deploy.sh          build + install + restart Vane
#   tools/deploy.sh --build  build only
#   tools/deploy.sh --shot   capture the Vane window to tools/last-window.png
set -e
cd "${0:A:h}/.."
# tools/pc.env (not committed) sets VANE_PC=user@host and VANE_KEY=path-to-ssh-key
[[ -f tools/pc.env ]] && source tools/pc.env
PC=${VANE_PC:?set VANE_PC=user@host in tools/pc.env}
KEY=${VANE_KEY:-~/.ssh/id_ed25519}
KEY=${KEY/#\~/$HOME}
pc() { ssh -i $KEY -o BatchMode=yes $PC "$@"; }

if [[ "$1" == "--shot" ]]; then
  scp -q -i $KEY tools/winshot.ps1 $PC:C:/vane-tools/winshot.ps1
  pc 'schtasks /create /f /tn VaneWinShot /sc once /sd 01/01/2000 /st 00:00 /it /tr "conhost.exe --headless powershell -NoProfile -ExecutionPolicy Bypass -File C:\vane-tools\winshot.ps1" >nul & del C:\vane-tools\win.png 2>nul & schtasks /run /tn VaneWinShot >nul'
  sleep 6
  scp -q -i $KEY $PC:C:/vane-tools/win.png tools/last-window.png && echo "saved tools/last-window.png"
  exit 0
fi

echo "Uploading source to C:\\vane-build"
COPYFILE_DISABLE=1 tar czf - src-tauri/src src-tauri/Cargo.toml src-tauri/build.rs src-tauri/tauri.conf.json src-tauri/capabilities src-tauri/icons ui | pc 'tar xzf - -C C:/vane-build'
echo "Building (about 1-2 minutes)"
pc 'C:\vane-tools\cargo.cmd C:\vane-build\src-tauri build --release' | grep -E '^(error|warning)|Finished|exit=' 
[[ "$1" == "--build" ]] && exit 0

echo "Installing to %LOCALAPPDATA%\\Programs\\Vane and restarting it"
pc 'taskkill /f /im Vane.exe >nul 2>&1 & ping -n 2 127.0.0.1 >nul & copy /y C:\vane-build\src-tauri\target\release\vane.exe "%LOCALAPPDATA%\Programs\Vane\Vane.exe" >nul && schtasks /create /f /tn VaneLaunch /sc once /sd 01/01/2000 /st 00:00 /it /tr "\"%LOCALAPPDATA%\Programs\Vane\Vane.exe\" --tray" >nul && schtasks /run /tn VaneLaunch >nul && echo installed'
