// Welcome window. Spawns the aegis cursor + voice agent in the background,
// saves any invite code, and closes.

// TODO: Pop sound disabled - not working on macOS (see GitHub issue)
// const popSound = new Audio("pop.mp3");
// popSound.preload = "auto";

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

    // Mark onboarding complete so next launch skips this screen
    await invoke("mark_onboarded").catch(() => {});

    // Spawn aegis
    invoke("spawn_aegis").catch((err) =>
        console.error("[welcome] spawn aegis failed:", err),
    );

    // Close the welcome window
    window.__TAURI__.window.getCurrentWindow().close();
});
