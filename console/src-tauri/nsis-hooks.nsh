!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\windows-path.ps1" -Action Add -InstallDir "$INSTDIR"'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Rua PATH integration failed with exit code $0"
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\windows-path.ps1" -Action Remove -InstallDir "$INSTDIR"'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Rua PATH cleanup failed with exit code $0"
  ${EndIf}
!macroend
