// Settings page, shown once the user is signed in. Account section reads the
// keychain session (account_status / sign_out). Integration status comes from
// the aegis `integrations-status` subcommand, surfaced via the
// integrations_status Tauri command. Connect flows are still to come.

const { invoke } = window.__TAURI__.core;

// The integrations Aegis supports. `connect` describes how a user authenticates
// it today (all local). Wired to real connect flows later.
const INTEGRATIONS = [
    { key: "github", name: "GitHub", connect: "gh auth login" },
    { key: "gmail", name: "Gmail", connect: "Google OAuth" },
    { key: "spotify", name: "Spotify", connect: "spotify_player authenticate" },
    { key: "youtube", name: "YouTube", connect: "yt-dlp (no auth)" },
];

// How each backend state renders as a pill: label + the color classes layered
// on top of the shared pill base.
const PILL = {
    checking: { label: "Checking…", cls: "bg-white/10 text-gray-500" },
    connected: { label: "Connected", cls: "bg-green-500/15 text-green-400" },
    not_connected: { label: "Not connected", cls: "bg-white/10 text-gray-400" },
    error: { label: "Error", cls: "bg-red-500/15 text-red-400" },
    unknown: { label: "Unknown", cls: "bg-white/10 text-gray-500" },
};
const PILL_BASE = "ml-auto rounded-full px-2 py-0.5 text-xs";

/** Resize the (frameless) window to something settings-appropriate. */
async function sizeWindow() {
    const { getCurrentWindow, LogicalSize } = window.__TAURI__.window;
    await getCurrentWindow().setSize(new LogicalSize(720, 640));
    await getCurrentWindow().center();
}

function integrationRow({ key, name }) {
    const li = document.createElement("li");
    li.className = "flex items-center gap-3 px-4 py-3";
    li.innerHTML = `
        <img src="../icons/${key}.svg" alt="" class="h-6 w-6" />
        <div class="min-w-0">
            <p class="font-medium">${name}</p>
        </div>
        <span class="${PILL_BASE} ${PILL.checking.cls}" data-status="${key}">${PILL.checking.label}</span>
        <button class="rounded-md bg-[#ff8c00] px-3 py-1 text-sm font-semibold text-white
                       hover:bg-[#ffa500]" data-connect="${key}">Connect</button>
    `;
    return li;
}

/** Paint one integration's pill + Connect button from a backend state. */
function applyState(key, state, detail) {
    const pill = document.querySelector(`[data-status="${key}"]`);
    const btn = document.querySelector(`[data-connect="${key}"]`);
    const look = PILL[state] ?? PILL.unknown;
    if (pill) {
        pill.className = `${PILL_BASE} ${look.cls}`;
        pill.textContent = look.label;
        if (detail) pill.title = detail;
    }
    if (btn) {
        const connected = state === "connected";
        btn.disabled = connected;
        btn.textContent = connected ? "Connected" : "Connect";
        btn.className = connected
            ? "rounded-md bg-white/10 px-3 py-1 text-sm font-semibold text-gray-400 cursor-default"
            : "rounded-md bg-[#ff8c00] px-3 py-1 text-sm font-semibold text-white hover:bg-[#ffa500]";
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
        const key = btn.getAttribute("data-connect");
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
