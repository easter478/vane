; Vane setup: after installing, offer the free tools Vane reads from and works with.
; They are installed by Windows Package Manager (winget) from their publishers, not bundled.
;   MSI Afterburner             - CPU temperature, CPU power and GPU voltage
;   RivaTuner Statistics Server - FPS, frame time and 1% low in games
;   OpenRGB                     - lighting for the motherboard, memory, graphics card and other RGB parts

!macro NSIS_HOOK_PREINSTALL
  ; Vane and L-Connect can't share the devices; stop a running Vane so its files can be replaced
  nsExec::Exec 'taskkill /f /im Vane.exe'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  IfSilent vane_deps_done
  MessageBox MB_YESNO|MB_ICONQUESTION "Vane reads CPU temperature from MSI Afterburner and game FPS from RivaTuner Statistics Server.$\r$\n$\r$\nInstall them now? Both are free. Windows Package Manager downloads them from their publishers, and ones you already have are skipped.$\r$\n$\r$\nWindows may ask for admin permission." IDNO vane_rgb
    DetailPrint "Installing MSI Afterburner with Windows Package Manager"
    nsExec::ExecToLog 'cmd /c winget install --id Guru3D.Afterburner -e --silent --accept-package-agreements --accept-source-agreements'
    Pop $0
    StrCmp $0 "9009" vane_no_winget
    StrCmp $0 "error" vane_no_winget
    DetailPrint "Installing RivaTuner Statistics Server with Windows Package Manager"
    nsExec::ExecToLog 'cmd /c winget install --id Guru3D.RTSS -e --silent --accept-package-agreements --accept-source-agreements'
    Pop $1
  vane_rgb:
  MessageBox MB_YESNO|MB_ICONQUESTION "Light the rest of the PC too?$\r$\n$\r$\nOpenRGB lets Vane drive the lighting on the motherboard, memory, graphics card and other RGB parts with the same effects as the fans.$\r$\n$\r$\nInstall OpenRGB now? It's free and open source." IDNO vane_tip
    DetailPrint "Installing OpenRGB with Windows Package Manager"
    nsExec::ExecToLog 'cmd /c winget install --id OpenRGB.OpenRGB -e --silent --accept-package-agreements --accept-source-agreements'
    Pop $2
    StrCmp $2 "9009" vane_no_winget
    StrCmp $2 "error" vane_no_winget
  vane_tip:
    MessageBox MB_OK|MB_ICONINFORMATION "Leave MSI Afterburner running in the tray: Vane reads the CPU temperature from it.$\r$\n$\r$\nVane's Settings page shows which readings are coming through and how to turn on any that are missing."
    Goto vane_deps_done
  vane_no_winget:
    MessageBox MB_OK|MB_ICONEXCLAMATION "Windows Package Manager (winget) isn't available on this PC, so nothing extra was installed.$\r$\n$\r$\nGet MSI Afterburner and RivaTuner from guru3d.com, and OpenRGB from openrgb.org. Vane works without them, with fewer readings."
  vane_deps_done:
!macroend
