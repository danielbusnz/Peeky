// Welcome window. Spawns the aegis cursor + voice agent in the background,
// saves any invite code, and closes.

// TODO: Pop sound disabled - not working on macOS (see GitHub issue)
// const popSound = new Audio("pop.mp3");
// popSound.preload = "auto";

// Invite code: press Enter to check it against the proxy before committing.
// Green outline + check = usable; red outline + x = rejected. Informational
// only; the code is still saved when the cursor button is clicked.
const codeInput = document.getElementById("invite-code");
const codeStatus = document.getElementById("invite-status");

function setInviteState(state) {
    codeInput.classList.remove("valid", "invalid");
    codeStatus.classList.remove("valid", "invalid", "checking");
    if (state === "valid" || state === "invalid") {
        codeInput.classList.add(state);
        codeStatus.classList.add(state);
    } else if (state === "checking") {
        codeStatus.classList.add("checking");
    }
}

// Clear any prior result the moment the user edits, so stale feedback doesn't
// linger over a code that no longer matches it.
codeInput.addEventListener("input", () => setInviteState(null));

codeInput.addEventListener("keydown", async (e) => {
    if (e.key !== "Enter") return;
    e.preventDefault();
    const code = codeInput.value.trim().toUpperCase();
    if (!code) {
        setInviteState(null);
        return;
    }
    codeInput.value = code; // reflect the normalized value we actually check
    setInviteState("checking");
    try {
        await window.__TAURI__.core.invoke("verify_invite_code", { code });
        setInviteState("valid");
    } catch (err) {
        setInviteState("invalid");
    }
});

// Swap the welcome view for the "how to use it" card, filling in the
// platform's push-to-talk chord. macOS uses Ctrl+Space (the cross-platform
// winit hotkey); other platforms read the same.
function showHowTo() {
    const isMac = /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent || "");
    document.getElementById("hotkey-combo").textContent = isMac ? "⌃ Space" : "Ctrl + Space";
    document.querySelector(".window").classList.add("show-howto");
}

document.getElementById("cursor-button").addEventListener("click", async () => {
    // TODO: Pop sound disabled - not working on macOS
    // popSound.currentTime = 0;
    // popSound.play().catch(() => { });

    const { invoke } = window.__TAURI__.core;

    // Save invite code if entered
    const code = document.getElementById("invite-code").value.trim().toUpperCase();
    if (code) {
        try {
            await invoke("save_invite_code", { code });
        } catch (err) {
            console.error("[welcome] save invite code failed:", err);
        }
    }

    // Show the hotkey card. Onboarding is finalized + aegis spawned only when
    // the user acknowledges it, so they always see how to talk before it starts.
    showHowTo();
});

document.getElementById("howto-done").addEventListener("click", async () => {
    const { invoke } = window.__TAURI__.core;

    // Mark onboarding complete so next launch skips this screen
    await invoke("mark_onboarded").catch(() => {});

    // Spawn aegis
    invoke("spawn_aegis").catch((err) =>
        console.error("[welcome] spawn aegis failed:", err),
    );

    // Close the welcome window
    window.__TAURI__.window.getCurrentWindow().close();
});
