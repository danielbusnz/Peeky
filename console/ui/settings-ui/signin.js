// Sign-in window. A "Continue with GitHub" button invokes the launcher's
// github_sign_in command, which opens the system browser and polls the proxy
// until the session token lands in the OS keychain. The browser holds the
// OAuth dance; this window just kicks it off and reflects the result.

const { invoke } = window.__TAURI__.core;

// The card fills the OS window edge to edge (sized in tauri.conf.json), same
// as the settings and onboarding pages, so there is no per-card resize here.

const els = {
    btn: document.getElementById("github-btn"),
    label: document.getElementById("github-label"),
    status: document.getElementById("status"),
    signedIn: document.getElementById("signed-in"),
    signedInEmail: document.getElementById("signed-in-email"),
    signOut: document.getElementById("signout-btn"),
};

function showSignedIn() {
    // Signed in: hand off to the settings page (same window, new page).
    window.location.href = "settings.html";
}

function showSignedOut() {
    els.signedIn.classList.add("hidden");
    els.btn.classList.remove("hidden");
    els.btn.disabled = false;
    els.label.textContent = "Continue with GitHub";
    els.status.textContent = "";
}

els.btn.addEventListener("click", async () => {
    els.btn.disabled = true;
    els.label.textContent = "Waiting for GitHub…";
    els.status.textContent = "Finish signing in in your browser.";
    try {
        const account = await invoke("github_sign_in");
        showSignedIn(account.email);
    } catch (err) {
        els.status.textContent = String(err);
        els.btn.disabled = false;
        els.label.textContent = "Continue with GitHub";
    }
});

els.signOut.addEventListener("click", async () => {
    await invoke("sign_out");
    showSignedOut();
});

// On open, reflect any existing session so a returning user sees it.
(async () => {
    try {
        const status = await invoke("account_status");
        if (status.signed_in) showSignedIn(status.email);
    } catch {
        // No session yet; leave the default signed-out UI.
    }
})();
