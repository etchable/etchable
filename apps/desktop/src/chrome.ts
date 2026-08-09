// The native title bar is hidden (titleBarStyle: Overlay), so the macOS
// traffic lights float over the shell titlebar's left edge; the Shell
// reserves room for them, but only when actually running in the Tauri
// webview on a Mac.
export const macOverlayChrome =
  navigator.userAgent.includes("Mac") && "__TAURI_INTERNALS__" in window;
