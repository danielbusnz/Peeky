// Welcome window. Plays a pop on cursor click, spawns the aegis cursor +
// voice agent in the background, and swaps to the pre-spawned (visible:false)
// onboarding window centered on welcome's current position.

const popSound = new Audio("pop.mp3");
popSound.preload = "auto";

document.getElementById("cursor-button").addEventListener("click", async () => {
    popSound.currentTime = 0;
    popSound.play().catch(() => { });

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

    // Spawn aegis
    invoke("spawn_aegis").catch((err) =>
        console.error("[welcome] spawn aegis failed:", err),
    );

    // Just close the welcome window
    window.__TAURI__.window.getCurrentWindow().close();
});
