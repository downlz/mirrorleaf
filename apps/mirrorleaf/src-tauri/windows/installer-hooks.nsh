; Mirrorleaf's Windows installer hooks (the Windows app). Tauri's installer
; runs them for the current user (no administrator rights needed).
;
; "Open with -> Mirrorleaf" for photos and text files, as the Mac's Dock icon
; takes them (M12): Mirrorleaf is listed under Open with, and never becomes any
; file type's default app (Tauri's own file associations would make it one).

!macro MIRRORLEAF_OPEN_WITH EXT
  WriteRegStr SHCTX "Software\Classes\.${EXT}\OpenWithProgids" "Mirrorleaf.File" ""
!macroend

!macro MIRRORLEAF_FORGET EXT
  DeleteRegValue SHCTX "Software\Classes\.${EXT}\OpenWithProgids" "Mirrorleaf.File"
!macroend

!macro MIRRORLEAF_EACH MACRO
  !insertmacro ${MACRO} jpg
  !insertmacro ${MACRO} jpeg
  !insertmacro ${MACRO} png
  !insertmacro ${MACRO} heic
  !insertmacro ${MACRO} heif
  !insertmacro ${MACRO} gif
  !insertmacro ${MACRO} webp
  !insertmacro ${MACRO} tif
  !insertmacro ${MACRO} tiff
  !insertmacro ${MACRO} bmp
  !insertmacro ${MACRO} avif
  !insertmacro ${MACRO} txt
  !insertmacro ${MACRO} md
!macroend

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\Mirrorleaf.File" "" "Mirrorleaf"
  WriteRegStr SHCTX "Software\Classes\Mirrorleaf.File" "FriendlyTypeName" "Mirrorleaf"
  WriteRegStr SHCTX "Software\Classes\Mirrorleaf.File\DefaultIcon" "" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr SHCTX "Software\Classes\Mirrorleaf.File\shell\open" "FriendlyAppName" "Mirrorleaf"
  WriteRegStr SHCTX "Software\Classes\Mirrorleaf.File\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  !insertmacro MIRRORLEAF_EACH MIRRORLEAF_OPEN_WITH
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro MIRRORLEAF_EACH MIRRORLEAF_FORGET
  DeleteRegKey SHCTX "Software\Classes\Mirrorleaf.File"
!macroend
