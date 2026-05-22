// Onboarding window. Dismisses itself when the user either clicks the visual
// keycap OR presses the platform's actual hotkey. The platform-specific HTML
// page is picked in launcher/src-tauri/src/main.rs; this JS works for both
// because the dismiss button shares one ID across pages and we listen for
// both Insert and Ctrl+Space.
//
// macos.html also includes an optional invite-code input. Detection is by
// element presence (#invite-section) rather than userAgent sniffing; index.html
// omits the section so the wiring below is a no-op on non-macOS.

const { invoke } = window.__TAURI__.core;

function dismiss() {
  window.__TAURI__.window.getCurrentWindow().close();
}

// Click the visual key.
document.getElementById("dismiss-button").addEventListener("click", dismiss);

// Top-right X to close without going through the hotkey flow.
document.getElementById("close-button").addEventListener("click", dismiss);

// Or press the actual hotkey. Insert on Linux/Windows, Ctrl+Space on macOS.
// Both branches are safe on every platform: Macs don't have an Insert key
// and Linux users won't normally press Ctrl+Space in this window. Skip
// dismissal when focus is in the invite-code field so the user can paste
// without the window closing on them.
window.addEventListener("keydown", (e) => {
  const target = e.target;
  if (target && target.id === "invite-code") return;
  if (e.key === "Insert") dismiss();
  if (e.key === " " && e.ctrlKey) dismiss();
});

// Wire up the invite-code section if present (macos.html only).
const inviteSection = document.getElementById("invite-section");
if (inviteSection) {
  const input = document.getElementById("invite-code");
  const button = document.getElementById("invite-save");
  const status = document.getElementById("invite-status");

  // Force uppercase as the user types so the rendered value matches the
  // placeholder style. The proxy normalizes too, but immediate feedback is
  // worth one input handler.
  input.addEventListener("input", () => {
    const start = input.selectionStart;
    const end = input.selectionEnd;
    input.value = input.value.toUpperCase();
    input.setSelectionRange(start, end);
  });

  // Submit on Enter for fast paste-and-go.
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      button.click();
    }
  });

  button.addEventListener("click", async () => {
    const code = input.value.trim();
    if (!code) {
      status.textContent = "Enter a code or skip with Ctrl+Space.";
      status.className = "text-xs text-gray-500 min-h-4";
      return;
    }
    button.disabled = true;
    status.textContent = "Saving...";
    status.className = "text-xs text-gray-500 min-h-4";
    try {
      await invoke("save_invite_code", { code });
      status.textContent = "Saved. Active on your next voice query.";
      status.className = "text-xs text-green-400 min-h-4";
    } catch (err) {
      status.textContent = String(err);
      status.className = "text-xs text-red-400 min-h-4";
    } finally {
      button.disabled = false;
    }
  });
}
