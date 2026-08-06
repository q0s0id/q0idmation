; q0player-setup.nsi
; Per-user installer for q0player. No admin rights, no q0editor mention.
; Build with: makensis q0player-setup.nsi (uses version.nsh fallback)
; Requires q0player to be built first: cargo build --release -p q0player

Unicode true
SetCompressor /SOLID lzma

;--------------------------------
; Application metadata

!include "version.nsh"
!define APP_NAME      "q0player"
!define APP_PUBLISHER "q0s"
!define APP_EXE       "q0player.exe"
!define Q0S_PROG_ID       "q0player.Movie"
!define Q0S_PROG_FRIENDLY "q0s Movie"
!define Q0S_EXT           ".q0s"
!define Q0V_PROG_ID       "q0player.Q0v"
!define Q0V_PROG_FRIENDLY "q0v Media"
!define Q0V_EXT           ".q0v"
!define PREVIOUS_PROG_ID_VALUE "PreviousDefaultProgId"
!define UNINST_KEY    "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
!define APP_REG_KEY   "Software\q0s\${APP_NAME}"

Name "${APP_NAME} ${APP_VERSION}"
!ifndef OUTPUT_FILE
  !define OUTPUT_FILE "dist\q0player-${APP_VERSION}-windows-x64-setup.exe"
!endif
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\q0s\${APP_NAME}"
InstallDirRegKey HKCU "${APP_REG_KEY}" "InstallDir"
RequestExecutionLevel user
ShowInstDetails show
ShowUninstDetails show
BrandingText "q0s :: black & red"

VIProductVersion "${APP_VERSION_NUMERIC}"
VIAddVersionKey "ProductName"     "${APP_NAME}"
VIAddVersionKey "ProductVersion"  "${APP_VERSION}"
VIAddVersionKey "FileDescription" "q0s movie player installer"
VIAddVersionKey "FileVersion"     "${APP_VERSION}"
VIAddVersionKey "CompanyName"     "${APP_PUBLISHER}"
VIAddVersionKey "LegalCopyright"  "(C) 2026 q0s"

;--------------------------------
; Modern UI 2

!include "MUI2.nsh"

!define MUI_ICON   "assets\q0player.ico"
!define MUI_UNICON "assets\q0player.ico"

!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP   "assets\header-player.bmp"
!define MUI_HEADERIMAGE_UNBITMAP "assets\header-player.bmp"
!define MUI_HEADERIMAGE_RIGHT

!define MUI_WELCOMEFINISHPAGE_BITMAP   "assets\welcome-player.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "assets\welcome-player.bmp"

!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Запустить ${APP_NAME}"

;--------------------------------
; Pages

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

!insertmacro MUI_LANGUAGE "Russian"

;--------------------------------
; Sections

Section "q0player (обязательно)" SecPlayer
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "..\target\release\${APP_EXE}"
  File "assets\q0player.ico"
  File "assets\q0s.ico"
  File "LICENSE.txt"

  ; Preserve and register .q0s -> q0player.Movie.
  ReadRegStr $0 HKCU "Software\Classes\${Q0S_EXT}" ""
  StrCmp $0 "${Q0S_PROG_ID}" q0player_install_q0s_validate_saved
  StrCmp $0 "" q0player_install_q0s_clear_saved
  StrCpy $1 $0 1
  StrCmp $1 " " q0player_install_q0s_clear_saved
  StrCmp $1 "$\t" q0player_install_q0s_clear_saved
  StrCpy $1 $0 1 -1
  StrCmp $1 " " q0player_install_q0s_clear_saved
  StrCmp $1 "$\t" q0player_install_q0s_clear_saved
  WriteRegStr HKCU "Software\Classes\${Q0S_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}" "$0"
  Goto q0player_install_q0s_backup_done

q0player_install_q0s_validate_saved:
  ReadRegStr $1 HKCU "Software\Classes\${Q0S_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"
  StrCmp $1 "" q0player_install_q0s_clear_saved
  StrCmp $1 "${Q0S_PROG_ID}" q0player_install_q0s_clear_saved
  StrCpy $2 $1 1
  StrCmp $2 " " q0player_install_q0s_clear_saved
  StrCmp $2 "$\t" q0player_install_q0s_clear_saved
  StrCpy $2 $1 1 -1
  StrCmp $2 " " q0player_install_q0s_clear_saved
  StrCmp $2 "$\t" q0player_install_q0s_clear_saved q0player_install_q0s_backup_done

q0player_install_q0s_clear_saved:
  DeleteRegValue HKCU "Software\Classes\${Q0S_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"

q0player_install_q0s_backup_done:
  WriteRegStr HKCU "Software\Classes\${Q0S_PROG_ID}" "" "${Q0S_PROG_FRIENDLY}"
  WriteRegStr HKCU "Software\Classes\${Q0S_PROG_ID}\DefaultIcon" "" '"$INSTDIR\q0s.ico"'
  WriteRegStr HKCU "Software\Classes\${Q0S_PROG_ID}\shell\open\command" "" '"$INSTDIR\${APP_EXE}" "%1"'
  WriteRegStr HKCU "Software\Classes\${Q0S_EXT}" "" "${Q0S_PROG_ID}"

  ; Preserve and register .q0v -> q0player.Q0v. q0v uses the player exe icon.
  ReadRegStr $0 HKCU "Software\Classes\${Q0V_EXT}" ""
  StrCmp $0 "${Q0V_PROG_ID}" q0player_install_q0v_validate_saved
  StrCmp $0 "" q0player_install_q0v_clear_saved
  StrCpy $1 $0 1
  StrCmp $1 " " q0player_install_q0v_clear_saved
  StrCmp $1 "$\t" q0player_install_q0v_clear_saved
  StrCpy $1 $0 1 -1
  StrCmp $1 " " q0player_install_q0v_clear_saved
  StrCmp $1 "$\t" q0player_install_q0v_clear_saved
  WriteRegStr HKCU "Software\Classes\${Q0V_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}" "$0"
  Goto q0player_install_q0v_backup_done

q0player_install_q0v_validate_saved:
  ReadRegStr $1 HKCU "Software\Classes\${Q0V_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"
  StrCmp $1 "" q0player_install_q0v_clear_saved
  StrCmp $1 "${Q0V_PROG_ID}" q0player_install_q0v_clear_saved
  StrCpy $2 $1 1
  StrCmp $2 " " q0player_install_q0v_clear_saved
  StrCmp $2 "$\t" q0player_install_q0v_clear_saved
  StrCpy $2 $1 1 -1
  StrCmp $2 " " q0player_install_q0v_clear_saved
  StrCmp $2 "$\t" q0player_install_q0v_clear_saved q0player_install_q0v_backup_done

q0player_install_q0v_clear_saved:
  DeleteRegValue HKCU "Software\Classes\${Q0V_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"

q0player_install_q0v_backup_done:
  WriteRegStr HKCU "Software\Classes\${Q0V_PROG_ID}" "" "${Q0V_PROG_FRIENDLY}"
  WriteRegStr HKCU "Software\Classes\${Q0V_PROG_ID}\DefaultIcon" "" '"$INSTDIR\${APP_EXE}",0'
  WriteRegStr HKCU "Software\Classes\${Q0V_PROG_ID}\shell\open\command" "" '"$INSTDIR\${APP_EXE}" "%1"'
  WriteRegStr HKCU "Software\Classes\${Q0V_EXT}" "" "${Q0V_PROG_ID}"

  ; Tell Explorer the association map changed so the new icon shows up
  ; without a logoff. SHCNE_ASSOCCHANGED = 0x08000000.
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'

  ; Start menu shortcut
  CreateDirectory "$SMPROGRAMS\q0s"
  CreateShortcut "$SMPROGRAMS\q0s\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0

  ; Install / uninstall registry
  WriteRegStr   HKCU "${APP_REG_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr   HKCU "${APP_REG_KEY}" "Version"    "${APP_VERSION}"

  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayName"     "${APP_NAME}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayVersion"  "${APP_VERSION}"
  WriteRegStr   HKCU "${UNINST_KEY}" "Publisher"       "${APP_PUBLISHER}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\${APP_EXE},0"
  WriteRegStr   HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr   HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Ярлык на рабочем столе" SecDesktop
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
SectionEnd

;--------------------------------
; Component descriptions

LangString DESC_SecPlayer  ${LANG_RUSSIAN} "Установить q0player и зарегистрировать .q0s в системе."
LangString DESC_SecDesktop ${LANG_RUSSIAN} "Создать ярлык q0player на рабочем столе."

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecPlayer}  $(DESC_SecPlayer)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_SecDesktop)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

;--------------------------------
; Uninstaller

Section "Uninstall"
  ; Restore .q0s only if q0player is still current.
  ReadRegStr $0 HKCU "Software\Classes\${Q0S_EXT}" ""
  StrCmp $0 "${Q0S_PROG_ID}" q0player_uninstall_q0s_is_current q0player_uninstall_q0s_keep_foreign

q0player_uninstall_q0s_is_current:
  ReadRegStr $1 HKCU "Software\Classes\${Q0S_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"
  StrCmp $1 "" q0player_uninstall_q0s_remove_default
  StrCmp $1 "${Q0S_PROG_ID}" q0player_uninstall_q0s_remove_default
  StrCpy $2 $1 1
  StrCmp $2 " " q0player_uninstall_q0s_remove_default
  StrCmp $2 "$\t" q0player_uninstall_q0s_remove_default
  StrCpy $2 $1 1 -1
  StrCmp $2 " " q0player_uninstall_q0s_remove_default
  StrCmp $2 "$\t" q0player_uninstall_q0s_remove_default
  WriteRegStr HKCU "Software\Classes\${Q0S_EXT}" "" "$1"
  Goto q0player_uninstall_q0s_done

q0player_uninstall_q0s_remove_default:
  DeleteRegValue HKCU "Software\Classes\${Q0S_EXT}" ""
  DeleteRegKey /ifempty HKCU "Software\Classes\${Q0S_EXT}"

q0player_uninstall_q0s_keep_foreign:
q0player_uninstall_q0s_done:
  DeleteRegKey HKCU "Software\Classes\${Q0S_PROG_ID}"

  ; Restore .q0v only if q0player is still current.
  ReadRegStr $0 HKCU "Software\Classes\${Q0V_EXT}" ""
  StrCmp $0 "${Q0V_PROG_ID}" q0player_uninstall_q0v_is_current q0player_uninstall_q0v_keep_foreign

q0player_uninstall_q0v_is_current:
  ReadRegStr $1 HKCU "Software\Classes\${Q0V_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"
  StrCmp $1 "" q0player_uninstall_q0v_remove_default
  StrCmp $1 "${Q0V_PROG_ID}" q0player_uninstall_q0v_remove_default
  StrCpy $2 $1 1
  StrCmp $2 " " q0player_uninstall_q0v_remove_default
  StrCmp $2 "$\t" q0player_uninstall_q0v_remove_default
  StrCpy $2 $1 1 -1
  StrCmp $2 " " q0player_uninstall_q0v_remove_default
  StrCmp $2 "$\t" q0player_uninstall_q0v_remove_default
  WriteRegStr HKCU "Software\Classes\${Q0V_EXT}" "" "$1"
  Goto q0player_uninstall_q0v_done

q0player_uninstall_q0v_remove_default:
  DeleteRegValue HKCU "Software\Classes\${Q0V_EXT}" ""
  DeleteRegKey /ifempty HKCU "Software\Classes\${Q0V_EXT}"

q0player_uninstall_q0v_keep_foreign:
q0player_uninstall_q0v_done:
  DeleteRegKey HKCU "Software\Classes\${Q0V_PROG_ID}"

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\q0player.ico"
  Delete "$INSTDIR\q0s.ico"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir  "$INSTDIR"
  RMDir  "$LOCALAPPDATA\Programs\q0s"

  Delete "$SMPROGRAMS\q0s\${APP_NAME}.lnk"
  RMDir  "$SMPROGRAMS\q0s"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "${APP_REG_KEY}"
  DeleteRegKey /ifempty HKCU "Software\q0s"

  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd
