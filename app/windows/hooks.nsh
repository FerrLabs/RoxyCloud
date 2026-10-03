!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $R8 SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\RoxyCloud" "UninstallString"
  ReadRegStr $R9 SHCTX "${MANUKEY}\RoxyCloud" ""
  ${If} $R8 != ""
  ${AndIf} $R9 != ""
    DetailPrint "Removing the RoxyCloud install this replaces, keeping its data"
    ExecWait '$R8 /S _?=$R9'
    Delete "$R9\uninstall.exe"
    RMDir "$R9"
    DeleteRegKey SHCTX "${MANUKEY}\RoxyCloud"
  ${EndIf}
!macroend
