; Removing the app also removes what it told the browsers about its helper
; (the native messaging host of the SNTH Vault extension). Per-user keys:
; no administrator rights. The app writes them again at its next start, so
; nothing is lost when this also runs during an update.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\ai.snth.connect"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\ai.snth.connect"
  DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\ai.snth.connect"
  DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\ai.snth.connect"
!macroend
