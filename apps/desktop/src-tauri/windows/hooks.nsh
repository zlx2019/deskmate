; Deskmate NSIS installer hooks for Windows.
;
; Windows prompts for firewall access when the app first listens on TCP/UDP.
; Denying that prompt causes device discovery to fail silently. Register an
; inbound rule during installation and remove it during uninstallation. Only
; private and domain networks are allowed to reduce public-network exposure.

!macro NSIS_HOOK_POSTINSTALL
  ; Remove an existing rule first for reinstall and upgrade scenarios.
  ; Keep the executable name synchronized with productName in Tauri.toml.
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Deskmate"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Deskmate" dir=in action=allow program="$INSTDIR\Deskmate.exe" enable=yes profile=private,domain'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Deskmate"'
!macroend
