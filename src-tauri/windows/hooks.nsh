; Registers ThreeStrands as a handler for mailto: links and .ics files, so it
; is listed in Windows Settings > Default apps and in Open with. It never makes
; itself the default: Windows leaves that choice to the user, and
; Settings > Default Apps in ThreeStrands opens the page where they make it.
; The ProgIDs and the RegisteredApplications name must match
; src-tauri/src/default_apps.rs. Both open commands pass the link or file as
; an argument; see src-tauri/src/launch_args.rs.

!define THREESTRANDS_CAPABILITIES "Software\Clients\Mail\ThreeStrands\Capabilities"

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\ThreeStrands.mailto" "" "ThreeStrands Email Link"
  WriteRegStr SHCTX "Software\Classes\ThreeStrands.mailto\DefaultIcon" "" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr SHCTX "Software\Classes\ThreeStrands.mailto\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'

  WriteRegStr SHCTX "Software\Classes\ThreeStrands.ics" "" "Calendar Invitation"
  WriteRegStr SHCTX "Software\Classes\ThreeStrands.ics\DefaultIcon" "" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr SHCTX "Software\Classes\ThreeStrands.ics\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  WriteRegStr SHCTX "Software\Classes\.ics\OpenWithProgids" "ThreeStrands.ics" ""

  WriteRegStr SHCTX "Software\Clients\Mail\ThreeStrands" "" "ThreeStrands"
  WriteRegStr SHCTX "${THREESTRANDS_CAPABILITIES}" "ApplicationName" "ThreeStrands"
  WriteRegStr SHCTX "${THREESTRANDS_CAPABILITIES}" "ApplicationDescription" "Email, calendar, and contacts."
  WriteRegStr SHCTX "${THREESTRANDS_CAPABILITIES}\URLAssociations" "mailto" "ThreeStrands.mailto"
  WriteRegStr SHCTX "${THREESTRANDS_CAPABILITIES}\FileAssociations" ".ics" "ThreeStrands.ics"
  WriteRegStr SHCTX "Software\RegisteredApplications" "ThreeStrands" "${THREESTRANDS_CAPABILITIES}"

  ; SHCNE_ASSOCCHANGED, so Explorer and Settings see the new handlers.
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue SHCTX "Software\RegisteredApplications" "ThreeStrands"
  DeleteRegKey SHCTX "Software\Clients\Mail\ThreeStrands"
  DeleteRegValue SHCTX "Software\Classes\.ics\OpenWithProgids" "ThreeStrands.ics"
  DeleteRegKey SHCTX "Software\Classes\ThreeStrands.ics"
  DeleteRegKey SHCTX "Software\Classes\ThreeStrands.mailto"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend
