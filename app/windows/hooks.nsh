!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $R8 SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\RoxyCloud" "UninstallString"
  ReadRegStr $R9 SHCTX "${MANUKEY}\RoxyCloud" ""
  ${If} $R8 != ""
  ${AndIf} $R9 != ""
    DetailPrint "Removing the RoxyCloud install this replaces, keeping its data"
    ExecWait '$R8 /S _?=$R9' $R7
    ${If} $R7 = 0
      Delete "$R9\uninstall.exe"
      RMDir "$R9"
      DeleteRegKey SHCTX "${MANUKEY}\RoxyCloud"
    ${Else}
      DetailPrint "The RoxyCloud uninstaller exited with $R7, leaving it in place"
    ${EndIf}
  ${EndIf}
!macroend
