; Shared fallback for direct manual makensis runs.
; installer/build.ps1 always overrides both values from cargo metadata.
!ifndef APP_VERSION
  !define APP_VERSION "0.1.0-beta.2"
!endif
!ifndef APP_VERSION_NUMERIC
  !define APP_VERSION_NUMERIC "0.1.0.0"
!endif
