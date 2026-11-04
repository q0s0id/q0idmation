; q0editor-setup.nsi
; Per-user installer for q0editor. q0player has its own installer.
; Build with: makensis q0editor-setup.nsi (uses version.nsh fallback)
; Requires q0editor to be built first: cargo build --release -p q0editor

Unicode true
SetCompressor /SOLID lzma

;--------------------------------
; Application metadata

!include "version.nsh"
!define APP_NAME       "q0editor"
!define APP_PUBLISHER  "q0idmation"
!define APP_EXE        "q0editor.exe"
!define PROG_ID        "q0editor.Project"
!define PROG_FRIENDLY  "q0editor Project"
!define EXT             ".q1s"
!define PREVIOUS_PROG_ID_VALUE "PreviousDefaultProgId"
; .q0l and .q0lang: one format, two spellings. The ProgID and the names of
; the saved previous defaults are the ones q0editor's own settings use.
!define Q0L_PROG_ID       "q0editor.Q0langSource"
!define Q0L_PROG_FRIENDLY "q0lang source"

; Before taking extension EXT for PROG, keep a valid foreign default in
; PROG's value SAVED. On an upgrade the original backup is kept instead of
; being replaced with ourselves. ID keeps each copy's labels apart.
!macro SaveForeignDefault EXT PROG SAVED ID
  ReadRegStr $0 HKCU "Software\Classes\${EXT}" ""
  StrCmp $0 "${PROG}" save_${ID}_validate
  StrCmp $0 "" save_${ID}_clear
  StrCpy $1 $0 1
  StrCmp $1 " " save_${ID}_clear
  StrCmp $1 "$\t" save_${ID}_clear
  StrCpy $1 $0 1 -1
  StrCmp $1 " " save_${ID}_clear
  StrCmp $1 "$\t" save_${ID}_clear
  WriteRegStr HKCU "Software\Classes\${PROG}" "${SAVED}" "$0"
  Goto save_${ID}_done
save_${ID}_validate:
  ReadRegStr $1 HKCU "Software\Classes\${PROG}" "${SAVED}"
  StrCmp $1 "" save_${ID}_clear
  StrCmp $1 "${PROG}" save_${ID}_clear
  StrCpy $2 $1 1
  StrCmp $2 " " save_${ID}_clear
  StrCmp $2 "$\t" save_${ID}_clear
  StrCpy $2 $1 1 -1
  StrCmp $2 " " save_${ID}_clear
  StrCmp $2 "$\t" save_${ID}_clear save_${ID}_done
save_${ID}_clear:
  DeleteRegValue HKCU "Software\Classes\${PROG}" "${SAVED}"
save_${ID}_done:
!macroend

; On uninstall, give EXT back to what it had before, but only while it is
; still ours: a later choice of the user's is never changed.
!macro RestoreForeignDefault EXT PROG SAVED ID
  ReadRegStr $0 HKCU "Software\Classes\${EXT}" ""
  StrCmp $0 "${PROG}" restore_${ID}_current restore_${ID}_done
restore_${ID}_current:
  ReadRegStr $1 HKCU "Software\Classes\${PROG}" "${SAVED}"
  StrCmp $1 "" restore_${ID}_remove
  StrCmp $1 "${PROG}" restore_${ID}_remove
  StrCpy $2 $1 1
  StrCmp $2 " " restore_${ID}_remove
  StrCmp $2 "$\t" restore_${ID}_remove
  StrCpy $2 $1 1 -1
  StrCmp $2 " " restore_${ID}_remove
  StrCmp $2 "$\t" restore_${ID}_remove
  WriteRegStr HKCU "Software\Classes\${EXT}" "" "$1"
  Goto restore_${ID}_done
restore_${ID}_remove:
  DeleteRegValue HKCU "Software\Classes\${EXT}" ""
  DeleteRegKey /ifempty HKCU "Software\Classes\${EXT}"
restore_${ID}_done:
!macroend
!define UNINST_KEY      "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
!define APP_REG_KEY     "Software\q0idmation\${APP_NAME}"
!ifndef APP_BINARY_DIR
  !define APP_BINARY_DIR "..\target\release"
!endif

Name "${APP_NAME} ${APP_VERSION}"
!ifndef OUTPUT_FILE
  !define OUTPUT_FILE "dist\q0editor-${APP_VERSION}-windows-x64-setup.exe"
!endif
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\q0idmation\${APP_NAME}"
InstallDirRegKey HKCU "${APP_REG_KEY}" "InstallDir"
RequestExecutionLevel user
ShowInstDetails show
ShowUninstDetails show
BrandingText "q0idmation :: black & red"

VIProductVersion "${APP_VERSION_NUMERIC}"
VIAddVersionKey "ProductName"     "${APP_NAME}"
VIAddVersionKey "ProductVersion"  "${APP_VERSION}"
VIAddVersionKey "FileDescription" "q0editor installer for q0idmation"
VIAddVersionKey "FileVersion"     "${APP_VERSION}"
VIAddVersionKey "CompanyName"     "${APP_PUBLISHER}"
VIAddVersionKey "LegalCopyright"  "(C) 2026 q0s"

;--------------------------------
; Modern UI 2

!include "MUI2.nsh"

!define MUI_ICON   "assets\q0editor.ico"
!define MUI_UNICON "assets\q0editor.ico"

!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP   "assets\header-editor.bmp"
!define MUI_HEADERIMAGE_UNBITMAP "assets\header-editor.bmp"
!define MUI_HEADERIMAGE_RIGHT

!define MUI_WELCOMEFINISHPAGE_BITMAP   "assets\welcome-editor.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "assets\welcome-editor.bmp"

!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_SMALLDESC
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Run ${APP_NAME}"

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

!insertmacro MUI_LANGUAGE "English"

;--------------------------------
; Sections

Section "q0editor (required)" SecEditor
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${APP_BINARY_DIR}\${APP_EXE}"
  File "assets\q0editor.ico"
  File "assets\q1s.ico"
  File "assets\q0l.ico"
  File "LICENSE.txt"
  File "..\q0fla\THIRD_PARTY_LICENSES.md"

  ; Preserve a valid foreign per-user default before taking ownership. On an
  ; upgrade, keep the original backup instead of replacing it with ourselves.
  ReadRegStr $0 HKCU "Software\Classes\${EXT}" ""
  StrCmp $0 "${PROG_ID}" q0editor_install_validate_saved
  StrCmp $0 "" q0editor_install_clear_saved
  StrCpy $1 $0 1
  StrCmp $1 " " q0editor_install_clear_saved
  StrCmp $1 "$\t" q0editor_install_clear_saved
  StrCpy $1 $0 1 -1
  StrCmp $1 " " q0editor_install_clear_saved
  StrCmp $1 "$\t" q0editor_install_clear_saved
  WriteRegStr HKCU "Software\Classes\${PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}" "$0"
  Goto q0editor_install_backup_done

q0editor_install_validate_saved:
  ReadRegStr $1 HKCU "Software\Classes\${PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"
  StrCmp $1 "" q0editor_install_clear_saved
  StrCmp $1 "${PROG_ID}" q0editor_install_clear_saved
  StrCpy $2 $1 1
  StrCmp $2 " " q0editor_install_clear_saved
  StrCmp $2 "$\t" q0editor_install_clear_saved
  StrCpy $2 $1 1 -1
  StrCmp $2 " " q0editor_install_clear_saved
  StrCmp $2 "$\t" q0editor_install_clear_saved q0editor_install_backup_done

q0editor_install_clear_saved:
  DeleteRegValue HKCU "Software\Classes\${PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"

q0editor_install_backup_done:
  ; .q1s -> q0editor.Project. Publish the shared extension default only after
  ; the private ProgID is ready.
  WriteRegStr HKCU "Software\Classes\${PROG_ID}" "" "${PROG_FRIENDLY}"
  WriteRegStr HKCU "Software\Classes\${PROG_ID}\DefaultIcon" "" '"$INSTDIR\q1s.ico"'
  WriteRegStr HKCU "Software\Classes\${PROG_ID}\shell\open\command" "" '"$INSTDIR\${APP_EXE}" "%1"'
  WriteRegStr HKCU "Software\Classes\${EXT}" "" "${PROG_ID}"

  ; .q0l and .q0lang -> q0editor.Q0langSource, the same way
  !insertmacro SaveForeignDefault ".q0l" "${Q0L_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}.q0l" q0l
  !insertmacro SaveForeignDefault ".q0lang" "${Q0L_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}.q0lang" q0lang
  WriteRegStr HKCU "Software\Classes\${Q0L_PROG_ID}" "" "${Q0L_PROG_FRIENDLY}"
  WriteRegStr HKCU "Software\Classes\${Q0L_PROG_ID}\DefaultIcon" "" '"$INSTDIR\q0l.ico"'
  WriteRegStr HKCU "Software\Classes\${Q0L_PROG_ID}\shell\open\command" "" '"$INSTDIR\${APP_EXE}" "%1"'
  WriteRegStr HKCU "Software\Classes\.q0l" "" "${Q0L_PROG_ID}"
  WriteRegStr HKCU "Software\Classes\.q0lang" "" "${Q0L_PROG_ID}"

  CreateDirectory "$SMPROGRAMS\q0s"
  CreateShortcut "$SMPROGRAMS\q0s\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0

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

  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
SectionEnd

;--------------------------------
; Component descriptions

LangString DESC_SecEditor  ${LANG_ENGLISH} "Install q0editor and open .q1s projects and .q0l scripts with it."
LangString DESC_SecDesktop ${LANG_ENGLISH} "Create a q0editor shortcut on the desktop."

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecEditor}  $(DESC_SecEditor)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_SecDesktop)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

;--------------------------------
; Uninstaller

Section "Uninstall"
  ; Restore the previous per-user ProgID only if q0editor is still current.
  ; A later association chosen by the user is never changed.
  ReadRegStr $0 HKCU "Software\Classes\${EXT}" ""
  StrCmp $0 "${PROG_ID}" q0editor_uninstall_editor_is_current q0editor_uninstall_keep_foreign

q0editor_uninstall_editor_is_current:
  ReadRegStr $1 HKCU "Software\Classes\${PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}"
  StrCmp $1 "" q0editor_uninstall_remove_default
  StrCmp $1 "${PROG_ID}" q0editor_uninstall_remove_default
  StrCpy $2 $1 1
  StrCmp $2 " " q0editor_uninstall_remove_default
  StrCmp $2 "$\t" q0editor_uninstall_remove_default
  StrCpy $2 $1 1 -1
  StrCmp $2 " " q0editor_uninstall_remove_default
  StrCmp $2 "$\t" q0editor_uninstall_remove_default
  WriteRegStr HKCU "Software\Classes\${EXT}" "" "$1"
  Goto q0editor_uninstall_association_done

q0editor_uninstall_remove_default:
  DeleteRegValue HKCU "Software\Classes\${EXT}" ""
  DeleteRegKey /ifempty HKCU "Software\Classes\${EXT}"

q0editor_uninstall_keep_foreign:
q0editor_uninstall_association_done:
  DeleteRegKey HKCU "Software\Classes\${PROG_ID}"

  !insertmacro RestoreForeignDefault ".q0l" "${Q0L_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}.q0l" q0l
  !insertmacro RestoreForeignDefault ".q0lang" "${Q0L_PROG_ID}" "${PREVIOUS_PROG_ID_VALUE}.q0lang" q0lang
  DeleteRegKey HKCU "Software\Classes\${Q0L_PROG_ID}"

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\q0editor.ico"
  Delete "$INSTDIR\q1s.ico"
  Delete "$INSTDIR\q0l.ico"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.md"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir  "$INSTDIR"
  RMDir  "$LOCALAPPDATA\Programs\q0idmation"

  Delete "$SMPROGRAMS\q0s\${APP_NAME}.lnk"
  RMDir  "$SMPROGRAMS\q0s"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "${APP_REG_KEY}"
  DeleteRegKey /ifempty HKCU "Software\q0idmation"

  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd
