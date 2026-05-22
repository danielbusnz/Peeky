// Onboarding window. Dismisses itself when the user either clicks the visual
// keycap OR presses the platform's actual hotkey. The platform-specific HTML
// page is picked in launcher/src-tauri/src/main.rs; this JS works for both
// because the dismiss button shares one ID across pages and we listen for
// both Insert and Ctrl+Space.

function dismiss() {
  window.__TAURI__.window.getCurrentWindow().close();
}

// Click the visual key.
document.getElementById("dismiss-button").addEventListener("click", dismiss);

// Top-right X to close without going through the hotkey flow.
document.getElementById("close-button").addEventListener("click", dismiss);

// Or press the actual hotkey. Insert on Linux/Windows, Ctrl+Space on macOS.
// Both branches are safe on every platform: Macs don't have an Insert key
// and Linux users won't normally press Ctrl+Space in this window.
window.addEventListener("keydown", (e) => {
  if (e.key === "Insert") dismiss();
  if (e.key === " " && e.ctrlKey) dismiss();
});
