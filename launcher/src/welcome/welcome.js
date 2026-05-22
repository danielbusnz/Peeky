// Welcome window. Plays a pop on cursor click, spawns the aegis cursor +
// voice agent in the background, and swaps to the pre-spawned (visible:false)
// onboarding window centered on welcome's current position.

const popSound = new Audio("pop.mp3");
popSound.preload = "auto";

document.getElementById("cursor-button").addEventListener("click", async () => {
    popSound.currentTime = 0;
    popSound.play().catch(() => { });

    // Fire-and-forget: start the actual aegis binary (cursor overlay + voice
    // loop). It runs as its own child process so we don't await — by the time
    // the onboarding screen finishes its instructions, aegis is already
    // listening for the hotkey.
    const { invoke } = window.__TAURI__.core;
    invoke("spawn_aegis").catch((err) =>
        console.error("[welcome] spawn aegis failed:", err),
    );

    try {
        const { getCurrentWindow, PhysicalPosition } = window.__TAURI__.window;
        const { WebviewWindow } = window.__TAURI__.webviewWindow;

        const welcome = getCurrentWindow();
        const next = await WebviewWindow.getByLabel("onboarding");
        if (!next) {
            console.error("[click] onboarding window not found");
            return;
        }

        const pos = await welcome.outerPosition();
        const wSize = await welcome.outerSize();
        const sSize = await next.outerSize();
        const cx = pos.x + Math.floor(wSize.width / 2 - sSize.width / 2);
        const cy = pos.y + Math.floor(wSize.height / 2 - sSize.height / 2);
        await next.setPosition(new PhysicalPosition(cx, cy));

        await next.show();
        await welcome.close();
    } catch (err) {
        console.error("[click] transition failed:", err);
    }
});
