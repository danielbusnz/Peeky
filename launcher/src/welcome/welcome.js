// Onboarding screen. Enrolls the user with an invite code or their own
// provider keys, gates "start" until one of those checks out, shows the
// push-to-talk hotkey, then spawns the aegis agent and closes.

// TODO: Pop sound disabled - not working on macOS (see GitHub issue)
// const popSound = new Audio("pop.mp3");
// popSound.preload = "auto";

// Invite code: press Enter to check it against the proxy before committing.
// Green outline + check = usable; red outline + x = rejected. Informational
// only; the code is still saved when the cursor button is clicked.
const codeInput = document.getElementById("invite-code");
const codeStatus = document.getElementById("invite-status");
const gateHint = document.getElementById("gate-hint");

function setHint(msg) {
    gateHint.textContent = msg || "";
}

// One-shot wobble to flag a field; the reflow lets it retrigger each click.
function shake(el) {
    el.classList.remove("shake");
    void el.offsetWidth;
    el.classList.add("shake");
    el.addEventListener("animationend", () => el.classList.remove("shake"), { once: true });
}

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
codeInput.addEventListener("input", () => {
    setInviteState(null);
    setHint("");
});

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

// Bring-your-own-keys: swap the invite field for the provider key fields,
// and reflect which keys are already stored so a blank submit keeps them.
const byokFields = {
    anthropic: document.getElementById("key-anthropic"),
    deepgram: document.getElementById("key-deepgram"),
    cartesia: document.getElementById("key-cartesia"),
};
const byokLabels = { anthropic: "Anthropic", deepgram: "Deepgram", cartesia: "Cartesia" };

document.getElementById("byok-toggle").addEventListener("click", () => {
    document.querySelector(".window").classList.add("show-byok");
    byokFields.anthropic.focus();
});
document.getElementById("byok-back").addEventListener("click", () => {
    document.querySelector(".window").classList.remove("show-byok");
    setHint("");
});

// Editing a key field clears its pass/fail mark and any gate hint.
for (const field of Object.values(byokFields)) {
    field.addEventListener("input", () => {
        field.classList.remove("valid", "invalid");
        setHint("");
    });
}

// Mark already-saved providers so the user knows a blank field is kept.
(async () => {
    try {
        const status = await window.__TAURI__.core.invoke("api_keys_status");
        for (const [name, field] of Object.entries(byokFields)) {
            if (status[name]) {
                field.classList.add("saved");
                field.placeholder = `${byokLabels[name]} key saved · leave blank to keep`;
            }
        }
    } catch (_) {
        // No Tauri (e.g. opened in a plain browser); leave defaults.
    }
})();

function isMac() {
    return /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent || "");
}

// Swap the welcome view for the "how to use it" card, filling in the
// platform's push-to-talk chord. macOS uses Ctrl+Space (the cross-platform
// winit hotkey); other platforms read the same.
function showHowTo() {
    document.getElementById("hotkey-combo").textContent = isMac() ? "⌃ Space" : "Ctrl + Space";
    document.querySelector(".window").classList.add("show-howto");
}

// macOS only: prompt for the agent's TCC permissions while the launcher is
// still in the foreground, so the agent inherits them on spawn instead of
// triggering prompts mid-session and forcing a restart. Mic + screen are
// API-grantable; accessibility can't be auto-granted, so we open its pane and
// continue (the hotkey starts working once the user toggles it). Commands come
// from tauri-plugin-macos-permissions.
//
// Returns true if a relaunch is needed (permissions were just granted and
// require app restart to take effect).
async function requestMacPermissions(invoke) {
    let needsRelaunch = false;

    // Check current state before requesting
    let hadMic = false;
    let hadScreen = false;
    try {
        hadMic = await invoke("plugin:macos-permissions|check_microphone_permission");
    } catch (_) {}
    try {
        hadScreen = await invoke("plugin:macos-permissions|check_screen_recording_permission");
    } catch (_) {}

    // Request permissions
    try {
        await invoke("plugin:macos-permissions|request_microphone_permission");
    } catch (_) {}
    try {
        await invoke("plugin:macos-permissions|request_screen_recording_permission");
    } catch (_) {}
    try {
        const granted = await invoke("plugin:macos-permissions|check_accessibility_permission");
        if (!granted) {
            await invoke("plugin:macos-permissions|request_accessibility_permission");
        }
    } catch (_) {}

    // Check if permissions changed (were just granted)
    let hasMic = false;
    let hasScreen = false;
    try {
        hasMic = await invoke("plugin:macos-permissions|check_microphone_permission");
    } catch (_) {}
    try {
        hasScreen = await invoke("plugin:macos-permissions|check_screen_recording_permission");
    } catch (_) {}

    // If any permission was just granted, we need to relaunch for it to take effect
    if ((!hadMic && hasMic) || (!hadScreen && hasScreen)) {
        needsRelaunch = true;
    }

    return needsRelaunch;
}

// True when the key-entry panel is the active enrollment mode.
function inBYOK() {
    return document.querySelector(".window").classList.contains("show-byok");
}

// The gate. Resolves true only when the active path has working credentials:
// the invite code verified against the proxy, or all three provider keys
// live-checked. Paints the relevant fields so a bad code or typo'd key shows
// why it was blocked instead of silently advancing.
async function hasValidCredentials(invoke) {
    if (inBYOK()) {
        const keys = {
            anthropic: byokFields.anthropic.value.trim(),
            deepgram: byokFields.deepgram.value.trim(),
            cartesia: byokFields.cartesia.value.trim(),
        };
        // A blank field is only allowed if that provider already has a saved
        // key (the backend falls back to the keychain for blanks).
        let anyBlank = false;
        for (const [name, field] of Object.entries(byokFields)) {
            if (!keys[name] && !field.classList.contains("saved")) {
                shake(field);
                anyBlank = true;
            }
        }
        if (anyBlank) {
            setHint("Enter all three API keys, or use an invite code.");
            return false;
        }
        setHint("Checking your keys…");
        let status;
        try {
            status = await invoke("verify_api_keys", keys);
        } catch (_) {
            setHint("Couldn't reach the providers. Check your connection.");
            return false;
        }
        for (const [name, field] of Object.entries(byokFields)) {
            field.classList.remove("valid", "invalid");
            field.classList.add(status[name] ? "valid" : "invalid");
            if (!status[name]) shake(field);
        }
        const allValid = status.anthropic && status.deepgram && status.cartesia;
        setHint(allValid ? "" : "One or more keys didn't work.");
        return allValid;
    }

    const code = codeInput.value.trim().toUpperCase();
    if (!code) {
        setHint("Enter an access code, or use your own API keys.");
        shake(codeInput);
        return false;
    }
    codeInput.value = code;
    setInviteState("checking");
    try {
        await invoke("verify_invite_code", { code });
        setInviteState("valid");
        return true;
    } catch (_) {
        setInviteState("invalid");
        setHint("That access code isn't valid.");
        shake(codeInput);
        return false;
    }
}

document.getElementById("cursor-button").addEventListener("click", async () => {
    // TODO: Pop sound disabled - not working on macOS
    // popSound.currentTime = 0;
    // popSound.play().catch(() => { });

    const { invoke } = window.__TAURI__.core;

    // Gate: can't advance without a valid code or working keys.
    if (!(await hasValidCredentials(invoke))) return;

    // Save invite code if entered
    const code = codeInput.value.trim().toUpperCase();
    if (code) {
        try {
            await invoke("save_invite_code", { code });
        } catch (err) {
            console.error("[welcome] save invite code failed:", err);
        }
    }

    // Persist any entered provider keys (blank fields are kept as-is).
    const anthropic = byokFields.anthropic.value.trim();
    const deepgram = byokFields.deepgram.value.trim();
    const cartesia = byokFields.cartesia.value.trim();
    if (anthropic || deepgram || cartesia) {
        try {
            await invoke("save_api_keys", { anthropic, deepgram, cartesia });
        } catch (err) {
            console.error("[welcome] save api keys failed:", err);
        }
    }

    // Show the hotkey card. Onboarding is finalized + aegis spawned only when
    // the user acknowledges it, so they always see how to talk before it starts.
    showHowTo();
});

document.getElementById("howto-done").addEventListener("click", async () => {
    const { invoke } = window.__TAURI__.core;

    // Mark onboarding complete so the next launch skips this screen.
    await invoke("mark_onboarded").catch(() => { });

    // Grant the agent's macOS permissions before it spawns (no-op elsewhere).
    // If permissions were just granted, relaunch so they take effect.
    if (isMac()) {
        const needsRelaunch = await requestMacPermissions(invoke);
        if (needsRelaunch) {
            // Relaunch the app so permissions take effect. On relaunch,
            // the onboarded file exists so the launcher skips the UI and
            // spawns aegis directly with the new permissions.
            window.__TAURI__.process.relaunch();
            return;
        }
    }

    // Fire-and-forget: the agent runs as its own process, so don't block the
    // window close on it booting.
    invoke("spawn_aegis").catch((err) =>
        console.error("[welcome] spawn aegis failed:", err),
    );

    window.__TAURI__.window.getCurrentWindow().close();
});
