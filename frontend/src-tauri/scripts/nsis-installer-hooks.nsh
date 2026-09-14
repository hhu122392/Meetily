; Tauri 2.11.1's stock NSIS template initializes its downgrade comparison on a
; custom page. NSIS skips that page in /S mode, so the stock EarlyChecks section
; can observe an empty comparison result and allow a silent downgrade.
;
; Re-check the installed DisplayVersion immediately before application files are
; written. This is intentionally limited to blocking downgrades; upgrades and
; same-version repair installs continue through the stock Tauri flow.
!define MEETILY_NSIS_HOOK_DIR "${__FILEDIR__}"

!macro MEETILY_CLEAR_UPGRADE_ENV
  System::Call 'kernel32::SetEnvironmentVariableW(w "MEETILY_INSTALLER_SOURCE_VERSION", p 0)i.r17'
  System::Call 'kernel32::SetEnvironmentVariableW(w "MEETILY_INSTALLER_TARGET_VERSION", p 0)i.r17'
  System::Call 'kernel32::SetEnvironmentVariableW(w "MEETILY_INSTALLER_DATA_ROOT", p 0)i.r17'
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; The custom installer template captured this value in .onInit, before an
  ; interactive page could run the old uninstaller and remove its registry key.
  StrCpy $R8 "$MeetilyInstalledVersion"
  ${If} $R8 != ""
    nsis_tauri_utils::SemverCompare "${VERSION}" $R8
    Pop $R9
    ${If} $R9 = -1
      ${IfNot} ${Silent}
      ${AndIf} $PassiveMode != 1
        MessageBox MB_ICONSTOP|MB_OK "$(newerVersionInstalled)"
      ${EndIf}
      SetErrorLevel 3
      Quit
    ${ElseIf} $R9 = 1
      ; A stable snapshot requires all app and SQLite writers to be stopped.
      ; The stock template performs the same check again after this hook; that
      ; second check is harmless and protects the later file-copy stage too.
      !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

      InitPluginsDir
      File "/oname=$PLUGINSDIR\meetily-versioned-data.ps1" "${MEETILY_NSIS_HOOK_DIR}\meetily-versioned-data.ps1"
      SetShellVarContext current
      StrCpy $R6 "${VERSION}"
      StrCpy $R5 "$APPDATA\${BUNDLEID}"
      System::Call 'kernel32::SetEnvironmentVariableW(w "MEETILY_INSTALLER_SOURCE_VERSION", w r18)i.r17'
      System::Call 'kernel32::SetEnvironmentVariableW(w "MEETILY_INSTALLER_TARGET_VERSION", w r16)i.r17'
      System::Call 'kernel32::SetEnvironmentVariableW(w "MEETILY_INSTALLER_DATA_ROOT", w r15)i.r17'
      nsExec::ExecToLog /TIMEOUT=1200000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "$PLUGINSDIR\meetily-versioned-data.ps1" -Mode Backup'
      Pop $R7
      ${If} $R7 == "error"
        StrCpy $R4 22
      ${ElseIf} $R7 == "timeout"
        StrCpy $R4 23
      ${ElseIf} $R7 != 0
        StrCpy $R4 24
      ${Else}
        StrCpy $R4 0
      ${EndIf}
      !insertmacro MEETILY_CLEAR_UPGRADE_ENV
      Delete "$PLUGINSDIR\meetily-versioned-data.ps1"

      ${If} $R4 != 0
        DetailPrint "Upgrade backup failed with code $R4; installation stopped before application files were copied."
        ${IfNot} ${Silent}
        ${AndIf} $PassiveMode != 1
          ${If} $LANGUAGE == ${LANG_SIMPCHINESE}
            MessageBox MB_ICONSTOP|MB_OK "升级前数据备份失败（代码 $R4）。安装已在修改程序文件前停止，原数据保持不变。"
          ${Else}
            MessageBox MB_ICONSTOP|MB_OK "The pre-upgrade data backup failed (code $R4). Installation stopped before application files were changed; the original data is unchanged."
          ${EndIf}
        ${EndIf}
        SetErrorLevel $R4
        Quit
      ${EndIf}
    ${EndIf}
  ${EndIf}

  ; Tauri's WiX bundler automatically includes the DirectML DLL emitted beside
  ; the Rust executable, but its NSIS bundler does not. Package the same signed
  ; DLL explicitly so clean Windows installations can load meetily.exe.
  File /a "/oname=DirectML.dll" "${MEETILY_NSIS_HOOK_DIR}\..\runtime\windows-x64\nsis\DirectML.dll"
!macroend

; Tauri's confirmation page is the first confirmation. If the user checks its
; delete-data option, validate the resolved target and require a second explicit
; confirmation that lists both exact absolute paths. Only the bundle-specific
; roaming core-data directory and local WebView profile are eligible; recordings
; and the shared template repository are deliberately outside this flow.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
    SetShellVarContext current
    GetFullPathName $MeetilyDeleteDataPath "$APPDATA\${BUNDLEID}"
    GetFullPathName $MeetilyDeleteWebViewPath "$LOCALAPPDATA\${BUNDLEID}"

    InitPluginsDir
    File "/oname=$PLUGINSDIR\meetily-uninstall-data-guard.ps1" "${MEETILY_NSIS_HOOK_DIR}\meetily-uninstall-data-guard.ps1"
    nsExec::ExecToStack /TIMEOUT=120000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "$PLUGINSDIR\meetily-uninstall-data-guard.ps1" -AppDataRoot "$APPDATA" -LocalAppDataRoot "$LOCALAPPDATA" -BundleId "${BUNDLEID}"'
    Pop $R7
    Pop $R6
    Delete "$PLUGINSDIR\meetily-uninstall-data-guard.ps1"
    ${If} $R7 != 0
      Goto p2200_unsafe_delete
    ${EndIf}

    ; Keep the confirmation text inside the uninstaller code instead of a custom
    ; LangString. Custom installer-language strings can compile into an empty
    ; uninstaller string table in multilingual NSIS builds.
    StrCmp $LANGUAGE ${LANG_SIMPCHINESE} p2200_delete_confirm_zh p2200_delete_confirm_en

    p2200_delete_confirm_zh:
      MessageBox MB_ICONEXCLAMATION|MB_YESNO|MB_DEFBUTTON2 "是否永久删除以下两个目录？$\r$\n$\r$\n核心数据：$MeetilyDeleteDataPath$\r$\nWebView 本地数据：$MeetilyDeleteWebViewPath$\r$\n$\r$\n会议录音目录和共享的 Meetily 模板仓储不在删除范围内。此操作无法撤销。" IDYES p2200_delete_confirmed
      Goto p2200_delete_declined

    p2200_delete_confirm_en:
      MessageBox MB_ICONEXCLAMATION|MB_YESNO|MB_DEFBUTTON2 "Permanently delete both directories below?$\r$\n$\r$\nCore data: $MeetilyDeleteDataPath$\r$\nLocal WebView data: $MeetilyDeleteWebViewPath$\r$\n$\r$\nMeeting recording directories and the shared Meetily template repository are not included. This action cannot be undone." IDYES p2200_delete_confirmed

    p2200_delete_declined:
    StrCpy $DeleteAppDataCheckboxState 0
    Goto p2200_delete_check_done

    p2200_unsafe_delete:
      StrCpy $DeleteAppDataCheckboxState 0
      StrCmp $LANGUAGE ${LANG_SIMPCHINESE} p2200_unsafe_delete_zh p2200_unsafe_delete_en

    p2200_unsafe_delete_zh:
      MessageBox MB_ICONSTOP|MB_OK "应用数据删除路径未通过完整安全检查，系统不会删除任何应用数据。"
      Goto p2200_delete_check_done

    p2200_unsafe_delete_en:
      MessageBox MB_ICONSTOP|MB_OK "The application-data delete targets failed the complete safety check. No application data will be deleted."
      Goto p2200_delete_check_done

    p2200_delete_confirmed:
    p2200_delete_check_done:
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete /REBOOTOK "$INSTDIR\DirectML.dll"
  RmDir /REBOOTOK "$INSTDIR"
!macroend
