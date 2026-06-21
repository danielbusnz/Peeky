// Settings page, shown once the user is signed in. Account section reads the
// keychain session (account_status / sign_out). Integration status comes from
// the peeky `integrations-status` subcommand, surfaced via the
// integrations_status Tauri command. Connect flows are still to come.

const { invoke } = window.__TAURI__.core;

// The integrations Peeky supports. `connect` describes how a user authenticates
// it today (all local). Wired to real connect flows later.
const INTEGRATIONS = [
    { key: "github", name: "GitHub", connect: "gh auth login" },
    { key: "gmail", name: "Gmail", connect: "Google OAuth" },
    { key: "spotify", name: "Spotify", connect: "spotify_player authenticate" },
    { key: "youtube", name: "YouTube", connect: "yt-dlp (no auth)" },
];

// How each backend state renders as a pill: label + modifier class applied
// on top of the base "pill" class (defined in settings.css).
const PILL = {
    checking:      { label: "Checking…",     cls: "pill--checking" },
    connected:     { label: "Connected",     cls: "pill--connected" },
    not_connected: { label: "Not connected", cls: "pill--not-connected" },
    error:         { label: "Error",         cls: "pill--error" },
    unknown:       { label: "Unknown",       cls: "pill--unknown" },
};

/** Resize the (frameless) window to something settings-appropriate. */
async function sizeWindow() {
    const { getCurrentWindow, LogicalSize } = window.__TAURI__.window;
    await getCurrentWindow().setSize(new LogicalSize(720, 640));
    await getCurrentWindow().center();
}

function integrationRow({ key, name }) {
    const li = document.createElement("li");
    li.className = "integration-row";
    li.innerHTML = `
        <img src="../icons/${key}.svg" alt="" class="integration-icon" />
        <span class="integration-name">${name}</span>
        <span class="pill ${PILL.checking.cls}" data-status="${key}">${PILL.checking.label}</span>
        <button class="btn-connect btn-connect--active" data-connect="${key}">Connect</button>
    `;
    return li;
}

/** Paint one integration's pill + Connect button from a backend state. */
function applyState(key, state, detail) {
    const pill = document.querySelector(`[data-status="${key}"]`);
    const btn  = document.querySelector(`[data-connect="${key}"]`);
    const look = PILL[state] ?? PILL.unknown;

    if (pill) {
        pill.className = `pill ${look.cls}`;
        pill.textContent = look.label;
        if (detail) pill.title = detail;
    }

    if (btn) {
        const connected = state === "connected";
        btn.disabled = connected;
        btn.textContent = connected ? "Connected" : "Connect";
        btn.className = connected
            ? "btn-connect btn-connect--done"
            : "btn-connect btn-connect--active";
    }
}

/** Fetch real status and paint every row. Falls back to "unknown" on failure. */
async function refreshStatus() {
    try {
        const rows = await invoke("integrations_status"); // [{name, state, detail}]
        for (const r of rows) applyState(r.name, r.state, r.detail);
    } catch (e) {
        for (const integ of INTEGRATIONS) applyState(integ.key, "unknown", String(e));
    }
}

function renderIntegrations() {
    const list = document.getElementById("integrations");
    for (const integ of INTEGRATIONS) list.appendChild(integrationRow(integ));

    list.addEventListener("click", (e) => {
        const btn = e.target.closest("[data-connect]");
        if (!btn || btn.disabled) return;
        // Placeholder: connect flows arrive with the integrations panel work.
        const key  = btn.getAttribute("data-connect");
        const pill = list.querySelector(`[data-status="${key}"]`);
        if (pill) pill.textContent = "Coming soon";
    });
}

document.getElementById("signout-btn").addEventListener("click", async () => {
    await invoke("sign_out");
    window.location.href = "signin.html";
});

(async () => {
    sizeWindow();
    renderIntegrations();
    try {
        const status = await invoke("account_status");
        if (!status.signed_in) {
            // Not signed in (e.g. signed out elsewhere): bounce back to sign-in.
            window.location.href = "signin.html";
            return;
        }
        document.getElementById("account-email").textContent = status.email || "";
    } catch {
        window.location.href = "signin.html";
        return;
    }
    refreshStatus();
})();
