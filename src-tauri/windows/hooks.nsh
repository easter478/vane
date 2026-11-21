; Vane setup: after installing, offer the free tools Vane reads its readings from.
; They are installed by Windows Package Manager (winget) from their publishers, not bundled.
;   HWiNFO                      - CPU temperature, GPU voltage and every other sensor
;   RivaTuner Statistics Server - FPS, frame time and 1% low in games

!macro NSIS_HOOK_PREINSTALL
  ; Vane and L-Connect can't share the devices; stop a running Vane so its files can be replaced
  nsExec::Exec 'taskkill /f /im Vane.exe'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  IfSilent vane_deps_done
  MessageBox MB_YESNO|MB_ICONQUESTION "Vane reads temperatures from HWiNFO and game FPS from RivaTuner Statistics Server.$\r$\n$\r$\nInstall them now? Both are free. Windows Package Manager downloads them from their publishers, and ones you already have are skipped.$\r$\n$\r$\nWindows may ask for admin permission." IDNO vane_deps_done
    DetailPrint "Installing HWiNFO with Windows Package Manager"
    nsExec::ExecToLog 'cmd /c winget install --id REALiX.HWiNFO -e --silent --accept-package-agreements --accept-source-agreements'
    Pop $0
    DetailPrint "Installing RivaTuner Statistics Server with Windows Package Manager"
    nsExec::ExecToLog 'cmd /c winget install --id Guru3D.RTSS -e --silent --accept-package-agreements --accept-source-agreements'
    Pop $1
    StrCmp $0 "9009" vane_no_winget
    StrCmp $0 "error" vane_no_winget
    MessageBox MB_OK|MB_ICONINFORMATION "Done. One more step for CPU temperature and GPU voltage:$\r$\n$\r$\nIn HWiNFO's Sensors window, right-click CPU (Tctl/Tdie) and GPU Core Voltage, choose Customize values and tick Report value in Gadget.$\r$\n$\r$\nVane's Settings page shows these steps too."
    Goto vane_deps_done
  vane_no_winget:
    MessageBox MB_OK|MB_ICONEXCLAMATION "Windows Package Manager (winget) isn't available on this PC, so HWiNFO and RivaTuner weren't installed.$\r$\n$\r$\nGet them from hwinfo.com and guru3d.com. Vane works without them, with fewer readings."
  vane_deps_done:
!macroend
