/**
 * Tests for console/ui/onboarding/onboarding.js
 *
 * Strategy: rebuild the DOM from the real index.html body and re-evaluate
 * onboarding.js as a plain script on each test so listeners are attached to the
 * fresh DOM, matching exactly how Tauri loads the file.
 */

import { readFileSync } from "fs";
import { resolve, dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));

const HTML_BODY = `
  <div class="window" data-tauri-drag-region>
    <div id="outer-halo">
      <div id="breathing-button"></div>
    </div>
    <button id="cursor-button" aria-label="Start Aegis">
      <img src="cursor.svg" alt="" />
    </button>
    <div id="enroll">
      <div id="invite">
        <input type="text" id="invite-code" placeholder="Invite code (optional)" autocomplete="off" autocapitalize="characters" spellcheck="false" />
        <span id="invite-status" aria-hidden="true"></span>
      </div>
      <div id="byok">
        <input type="password" id="key-anthropic" placeholder="Anthropic API key" autocomplete="off" spellcheck="false" />
        <input type="password" id="key-deepgram" placeholder="Deepgram API key" autocomplete="off" spellcheck="false" />
        <input type="password" id="key-cartesia" placeholder="Cartesia API key" autocomplete="off" spellcheck="false" />
      </div>
      <button type="button" id="byok-toggle" class="link-btn">Use your own API keys</button>
      <button type="button" id="byok-back" class="link-btn">Use an invite code instead</button>
      <p id="gate-hint" role="status" aria-live="polite"></p>
    </div>
    <div id="howto">
      <p class="howto-title">You're all set</p>
      <p class="howto-sub">Hold <kbd id="hotkey-combo">Ctrl + Space</kbd> and speak</p>
      <button id="howto-done" type="button">Got it</button>
    </div>
  </div>
`;

const WELCOME_JS = readFileSync(
  resolve(__dirname, "../ui/onboarding/onboarding.js"),
  "utf8"
);

// ---------- Tauri mock helpers ----------

function makeTauriMock(invokeImpl) {
  const closeMock = vi.fn();
  return {
    invoke: invokeImpl,
    core: { invoke: invokeImpl },
    window: {
      getCurrentWindow: () => ({ close: closeMock }),
    },
    _closeMock: closeMock,
  };
}

/**
 * Default invoke that rejects everything. Individual tests override via
 * `invokeFn` before clicking.
 */
function makeInvoke(overrides = {}) {
  return vi.fn(async (cmd, args) => {
    if (overrides[cmd] !== undefined) {
      const val = overrides[cmd];
      if (typeof val === "function") return val(args);
      if (val instanceof Error) throw val;
      return val;
    }
    // api_keys_status must resolve by default (returns all false = nothing saved)
    if (cmd === "api_keys_status") return { anthropic: false, deepgram: false, cartesia: false };
    throw new Error(`Unhandled invoke: ${cmd}`);
  });
}

/**
 * Reset the DOM, install mocks, and evaluate welcome.js.
 * Returns handles to the DOM elements and the invoke mock for assertions.
 */
async function setup(invokeOverrides = {}, { noTauri = false } = {}) {
  document.body.innerHTML = HTML_BODY;

  const invokeFn = makeInvoke(invokeOverrides);
  const tauri = makeTauriMock(invokeFn);

  if (noTauri) {
    delete window.__TAURI__;
  } else {
    window.__TAURI__ = tauri;
  }

  // Evaluate the script in the current window context. The IIFE for
  // api_keys_status fires here; we await a microtask flush afterward so its
  // promise settles before tests begin.
  // eslint-disable-next-line no-eval
  eval(WELCOME_JS);

  // Flush microtasks so the api_keys_status IIFE finishes
  await Promise.resolve();
  await Promise.resolve();

  return {
    invokeFn,
    tauri,
    win: () => document.querySelector(".window"),
    codeInput: () => document.getElementById("invite-code"),
    codeStatus: () => document.getElementById("invite-status"),
    gateHint: () => document.getElementById("gate-hint"),
    cursorBtn: () => document.getElementById("cursor-button"),
    byokToggle: () => document.getElementById("byok-toggle"),
    byokBack: () => document.getElementById("byok-back"),
    anthropicField: () => document.getElementById("key-anthropic"),
    deepgramField: () => document.getElementById("key-deepgram"),
    cartesiaField: () => document.getElementById("key-cartesia"),
    howto: () => document.getElementById("howto"),
    howtoDone: () => document.getElementById("howto-done"),
    hotkeyCombo: () => document.getElementById("hotkey-combo"),
  };
}

/** Fire a keydown event with the given key on an element. */
function fireKeydown(el, key) {
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

/** Fire an input event (simulates typing). */
function fireInput(el) {
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Click a button. */
function click(el) {
  el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
}

// ============================================================
// Invite code: Enter-key verification
// ============================================================

describe("invite code: Enter-key verify", () => {
  test("Enter on empty field is a no-op: no invoke, no state change", async () => {
    const { invokeFn, codeInput, codeStatus } = await setup();

    codeInput().value = "";
    fireKeydown(codeInput(), "Enter");
    await Promise.resolve();

    // verify_invite_code must not have been called
    const verifyCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "verify_invite_code");
    expect(verifyCalls).toHaveLength(0);
    expect(codeStatus().classList.contains("valid")).toBe(false);
    expect(codeStatus().classList.contains("invalid")).toBe(false);
    expect(codeStatus().classList.contains("checking")).toBe(false);
  });

  test("non-Enter keys are ignored", async () => {
    const { invokeFn, codeInput } = await setup();

    codeInput().value = "HELLO";
    fireKeydown(codeInput(), "a");
    fireKeydown(codeInput(), " ");
    await Promise.resolve();

    const verifyCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "verify_invite_code");
    expect(verifyCalls).toHaveLength(0);
  });

  test("Enter with a code shows 'checking' then 'valid' on resolve", async () => {
    const { invokeFn, codeInput, codeStatus } = await setup({
      verify_invite_code: null, // resolves with undefined = success
    });

    codeInput().value = "ABC123";
    fireKeydown(codeInput(), "Enter");

    // Flush the async handler
    await new Promise((r) => setTimeout(r, 0));

    expect(codeInput().classList.contains("valid")).toBe(true);
    expect(codeStatus().classList.contains("valid")).toBe(true);
    expect(codeInput().classList.contains("invalid")).toBe(false);
  });

  test("Enter with a code shows 'invalid' on reject", async () => {
    const { codeInput, codeStatus } = await setup({
      verify_invite_code: new Error("bad code"),
    });

    codeInput().value = "BAD";
    fireKeydown(codeInput(), "Enter");
    await new Promise((r) => setTimeout(r, 0));

    expect(codeInput().classList.contains("invalid")).toBe(true);
    expect(codeStatus().classList.contains("invalid")).toBe(true);
    expect(codeInput().classList.contains("valid")).toBe(false);
  });

  test("value is normalized: trimmed and uppercased before invoke", async () => {
    const { invokeFn, codeInput } = await setup({
      verify_invite_code: null,
    });

    codeInput().value = "  abc-123  ";
    fireKeydown(codeInput(), "Enter");
    await new Promise((r) => setTimeout(r, 0));

    const calls = invokeFn.mock.calls.filter(([cmd]) => cmd === "verify_invite_code");
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toEqual({ code: "ABC-123" });
    expect(codeInput().value).toBe("ABC-123");
  });

  test("editing the field clears prior valid state and the hint", async () => {
    const { codeInput, codeStatus, gateHint } = await setup({
      verify_invite_code: null,
    });

    // First get the field into valid state
    codeInput().value = "GOOD";
    fireKeydown(codeInput(), "Enter");
    await new Promise((r) => setTimeout(r, 0));
    expect(codeInput().classList.contains("valid")).toBe(true);

    // Set a hint manually (simulating a prior gate attempt)
    gateHint().textContent = "Some hint";

    // Now edit the field
    codeInput().value = "GOOD2";
    fireInput(codeInput());

    expect(codeInput().classList.contains("valid")).toBe(false);
    expect(codeInput().classList.contains("invalid")).toBe(false);
    expect(codeStatus().classList.contains("valid")).toBe(false);
    expect(gateHint().textContent).toBe("");
  });

  test("editing the field clears prior invalid state", async () => {
    const { codeInput, codeStatus } = await setup({
      verify_invite_code: new Error("bad"),
    });

    codeInput().value = "BAD";
    fireKeydown(codeInput(), "Enter");
    await new Promise((r) => setTimeout(r, 0));
    expect(codeInput().classList.contains("invalid")).toBe(true);

    codeInput().value = "BAD2";
    fireInput(codeInput());

    expect(codeInput().classList.contains("invalid")).toBe(false);
    expect(codeStatus().classList.contains("invalid")).toBe(false);
  });
});

// ============================================================
// BYOK toggle
// ============================================================

describe("BYOK toggle", () => {
  test("clicking #byok-toggle adds .show-byok to .window", async () => {
    const { win, byokToggle } = await setup();

    expect(win().classList.contains("show-byok")).toBe(false);
    click(byokToggle());
    expect(win().classList.contains("show-byok")).toBe(true);
  });

  test("clicking #byok-toggle focuses the anthropic field", async () => {
    const { byokToggle, anthropicField } = await setup();

    // jsdom supports focus tracking
    click(byokToggle());
    expect(document.activeElement).toBe(anthropicField());
  });

  test("clicking #byok-back removes .show-byok", async () => {
    const { win, byokToggle, byokBack } = await setup();

    click(byokToggle());
    expect(win().classList.contains("show-byok")).toBe(true);

    click(byokBack());
    expect(win().classList.contains("show-byok")).toBe(false);
  });

  test("clicking #byok-back clears the hint", async () => {
    const { gateHint, byokToggle, byokBack } = await setup();

    click(byokToggle());
    gateHint().textContent = "Some leftover hint";
    click(byokBack());
    expect(gateHint().textContent).toBe("");
  });

  test("editing a key field clears its valid mark", async () => {
    const { anthropicField } = await setup();

    anthropicField().classList.add("valid");
    anthropicField().value = "new";
    fireInput(anthropicField());
    expect(anthropicField().classList.contains("valid")).toBe(false);
  });

  test("editing a key field clears its invalid mark", async () => {
    const { deepgramField } = await setup();

    deepgramField().classList.add("invalid");
    deepgramField().value = "x";
    fireInput(deepgramField());
    expect(deepgramField().classList.contains("invalid")).toBe(false);
  });

  test("editing a key field clears the gate hint", async () => {
    const { gateHint, cartesiaField } = await setup();

    gateHint().textContent = "One or more keys didn't work.";
    cartesiaField().value = "y";
    fireInput(cartesiaField());
    expect(gateHint().textContent).toBe("");
  });

  test("each key field independently clears on input", async () => {
    const { anthropicField, deepgramField, cartesiaField } = await setup();

    [anthropicField(), deepgramField(), cartesiaField()].forEach((f) => {
      f.classList.add("invalid");
    });

    fireInput(anthropicField());
    expect(anthropicField().classList.contains("invalid")).toBe(false);
    expect(deepgramField().classList.contains("invalid")).toBe(true);
    expect(cartesiaField().classList.contains("invalid")).toBe(true);
  });
});

// ============================================================
// api_keys_status prefill
// ============================================================

describe("api_keys_status prefill on load", () => {
  test("saved providers get .saved class and updated placeholder", async () => {
    const { anthropicField, deepgramField, cartesiaField } = await setup({
      api_keys_status: { anthropic: true, deepgram: true, cartesia: false },
    });

    expect(anthropicField().classList.contains("saved")).toBe(true);
    expect(anthropicField().placeholder).toMatch(/Anthropic key saved/);
    expect(deepgramField().classList.contains("saved")).toBe(true);
    expect(deepgramField().placeholder).toMatch(/Deepgram key saved/);
    expect(cartesiaField().classList.contains("saved")).toBe(false);
    expect(cartesiaField().placeholder).toBe("Cartesia API key"); // unchanged
  });

  test("unsaved providers have no .saved class", async () => {
    const { anthropicField, deepgramField, cartesiaField } = await setup({
      api_keys_status: { anthropic: false, deepgram: false, cartesia: false },
    });

    expect(anthropicField().classList.contains("saved")).toBe(false);
    expect(deepgramField().classList.contains("saved")).toBe(false);
    expect(cartesiaField().classList.contains("saved")).toBe(false);
  });

  test("missing window.__TAURI__ does not throw", async () => {
    // setup with noTauri=true; if the IIFE throws the test itself fails
    await expect(setup({}, { noTauri: true })).resolves.toBeDefined();
  });

  test("all three providers saved", async () => {
    const { anthropicField, deepgramField, cartesiaField } = await setup({
      api_keys_status: { anthropic: true, deepgram: true, cartesia: true },
    });

    expect(anthropicField().classList.contains("saved")).toBe(true);
    expect(deepgramField().classList.contains("saved")).toBe(true);
    expect(cartesiaField().classList.contains("saved")).toBe(true);
  });
});

// ============================================================
// The gate: #cursor-button click — invite flow
// ============================================================

describe("gate: invite flow", () => {
  test("empty invite code → blocked, hint set, #invite-code shakes, no save called", async () => {
    const { invokeFn, codeInput, gateHint, cursorBtn, win } = await setup();

    codeInput().value = "";
    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(win().classList.contains("show-howto")).toBe(false);
    expect(gateHint().textContent).toBeTruthy();
    expect(codeInput().classList.contains("shake")).toBe(true);

    const saveCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "save_invite_code");
    expect(saveCalls).toHaveLength(0);
  });

  test("invite code rejected → blocked, #invite-code gets .invalid + .shake, hint set", async () => {
    const { invokeFn, codeInput, gateHint, cursorBtn, win } = await setup({
      verify_invite_code: new Error("rejected"),
    });

    codeInput().value = "BADCODE";
    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(win().classList.contains("show-howto")).toBe(false);
    expect(codeInput().classList.contains("invalid")).toBe(true);
    expect(codeInput().classList.contains("shake")).toBe(true);
    expect(gateHint().textContent).toBeTruthy();

    const saveCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "save_invite_code");
    expect(saveCalls).toHaveLength(0);
  });

  test("good invite code → .show-howto added, save_invite_code called with normalized code", async () => {
    const { invokeFn, codeInput, cursorBtn, win } = await setup({
      verify_invite_code: null,
      save_invite_code: null,
    });

    codeInput().value = "  good-code  ";
    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(win().classList.contains("show-howto")).toBe(true);

    const saveCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "save_invite_code");
    expect(saveCalls).toHaveLength(1);
    expect(saveCalls[0][1]).toEqual({ code: "GOOD-CODE" });
  });

  test("good invite code → normalized code passed to verify", async () => {
    const { invokeFn, codeInput, cursorBtn } = await setup({
      verify_invite_code: null,
      save_invite_code: null,
    });

    codeInput().value = "  lowercase  ";
    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    const verifyCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "verify_invite_code");
    expect(verifyCalls).toHaveLength(1);
    expect(verifyCalls[0][1]).toEqual({ code: "LOWERCASE" });
  });
});

// ============================================================
// The gate: #cursor-button click — BYOK flow
// ============================================================

describe("gate: BYOK flow", () => {
  test("all keys valid → .show-howto added, save_api_keys called with trimmed values", async () => {
    const { invokeFn, anthropicField, deepgramField, cartesiaField, byokToggle, cursorBtn, win } =
      await setup({
        verify_api_keys: { anthropic: true, deepgram: true, cartesia: true },
        save_api_keys: null,
      });

    click(byokToggle());
    anthropicField().value = "  sk-ant  ";
    deepgramField().value = "dg-key";
    cartesiaField().value = "  cart-key  ";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(win().classList.contains("show-howto")).toBe(true);

    const saveCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "save_api_keys");
    expect(saveCalls).toHaveLength(1);
    expect(saveCalls[0][1]).toEqual({
      anthropic: "sk-ant",
      deepgram: "dg-key",
      cartesia: "cart-key",
    });
  });

  test("one key invalid → blocked, invalid field gets .invalid + .shake, valid fields get .valid, hint set", async () => {
    const { anthropicField, deepgramField, cartesiaField, byokToggle, cursorBtn, win, gateHint } =
      await setup({
        verify_api_keys: { anthropic: true, deepgram: false, cartesia: true },
        save_api_keys: null,
      });

    click(byokToggle());
    anthropicField().value = "sk-ant";
    deepgramField().value = "bad-key";
    cartesiaField().value = "cart-key";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(win().classList.contains("show-howto")).toBe(false);
    expect(anthropicField().classList.contains("valid")).toBe(true);
    expect(deepgramField().classList.contains("invalid")).toBe(true);
    expect(deepgramField().classList.contains("shake")).toBe(true);
    expect(cartesiaField().classList.contains("valid")).toBe(true);
    expect(gateHint().textContent).toBeTruthy();
  });

  test("blank field without .saved → blocked BEFORE verify_api_keys, blank field shakes", async () => {
    const { invokeFn, anthropicField, deepgramField, cartesiaField, byokToggle, cursorBtn, win } =
      await setup({
        verify_api_keys: { anthropic: true, deepgram: true, cartesia: true },
      });

    click(byokToggle());
    anthropicField().value = "sk-ant";
    deepgramField().value = ""; // blank, not saved
    cartesiaField().value = "cart-key";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(win().classList.contains("show-howto")).toBe(false);

    // verify_api_keys must NOT have been called
    const verifyCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "verify_api_keys");
    expect(verifyCalls).toHaveLength(0);

    expect(deepgramField().classList.contains("shake")).toBe(true);
  });

  test("blank field that IS .saved → allowed through to verify_api_keys", async () => {
    const { invokeFn, anthropicField, deepgramField, cartesiaField, byokToggle, cursorBtn } =
      await setup({
        api_keys_status: { anthropic: false, deepgram: true, cartesia: false },
        verify_api_keys: { anthropic: true, deepgram: true, cartesia: true },
        save_api_keys: null,
      });

    // deepgramField gets .saved from the prefill IIFE because api_keys_status
    // reports deepgram=true. Leave it blank.
    click(byokToggle());
    anthropicField().value = "sk-ant";
    deepgramField().value = ""; // blank but .saved
    cartesiaField().value = "cart-key";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    const verifyCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "verify_api_keys");
    expect(verifyCalls).toHaveLength(1);
  });

  test("blank .saved field → blank string passed to verify_api_keys", async () => {
    const { invokeFn, anthropicField, deepgramField, cartesiaField, byokToggle, cursorBtn } =
      await setup({
        api_keys_status: { anthropic: false, deepgram: true, cartesia: false },
        verify_api_keys: { anthropic: true, deepgram: true, cartesia: true },
        save_api_keys: null,
      });

    click(byokToggle());
    anthropicField().value = "sk-ant";
    deepgramField().value = "";
    cartesiaField().value = "cart-key";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    const verifyCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "verify_api_keys");
    expect(verifyCalls[0][1]).toEqual({
      anthropic: "sk-ant",
      deepgram: "",
      cartesia: "cart-key",
    });
  });

  test("multiple blank unsaved fields → all shake", async () => {
    const { deepgramField, cartesiaField, byokToggle, cursorBtn, anthropicField } = await setup({
      verify_api_keys: { anthropic: true, deepgram: true, cartesia: true },
    });

    click(byokToggle());
    anthropicField().value = "sk-ant";
    deepgramField().value = "";
    cartesiaField().value = "";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(deepgramField().classList.contains("shake")).toBe(true);
    expect(cartesiaField().classList.contains("shake")).toBe(true);
  });

  test("hint text set when blank unsaved fields present", async () => {
    const { deepgramField, byokToggle, cursorBtn, gateHint, anthropicField, cartesiaField } =
      await setup();

    click(byokToggle());
    anthropicField().value = "sk-ant";
    deepgramField().value = "";
    cartesiaField().value = "cart-key";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(gateHint().textContent).toBe("Enter all three API keys, or use an invite code.");
  });

  test("verify_api_keys called with correct keys object", async () => {
    const { invokeFn, anthropicField, deepgramField, cartesiaField, byokToggle, cursorBtn } =
      await setup({
        verify_api_keys: { anthropic: true, deepgram: true, cartesia: true },
        save_api_keys: null,
      });

    click(byokToggle());
    anthropicField().value = "ant";
    deepgramField().value = "dg";
    cartesiaField().value = "cart";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    const verifyCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "verify_api_keys");
    expect(verifyCalls[0][1]).toEqual({ anthropic: "ant", deepgram: "dg", cartesia: "cart" });
  });

  test("all invalid keys → all fields marked invalid and shake", async () => {
    const { anthropicField, deepgramField, cartesiaField, byokToggle, cursorBtn } = await setup({
      verify_api_keys: { anthropic: false, deepgram: false, cartesia: false },
    });

    click(byokToggle());
    anthropicField().value = "bad1";
    deepgramField().value = "bad2";
    cartesiaField().value = "bad3";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(anthropicField().classList.contains("invalid")).toBe(true);
    expect(deepgramField().classList.contains("invalid")).toBe(true);
    expect(cartesiaField().classList.contains("invalid")).toBe(true);
    expect(anthropicField().classList.contains("shake")).toBe(true);
    expect(deepgramField().classList.contains("shake")).toBe(true);
    expect(cartesiaField().classList.contains("shake")).toBe(true);
  });

  test("verify_api_keys network error → hint set, not advanced", async () => {
    const { gateHint, anthropicField, deepgramField, cartesiaField, byokToggle, cursorBtn, win } =
      await setup({
        verify_api_keys: new Error("network error"),
      });

    click(byokToggle());
    anthropicField().value = "ant";
    deepgramField().value = "dg";
    cartesiaField().value = "cart";

    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(win().classList.contains("show-howto")).toBe(false);
    expect(gateHint().textContent).toMatch(/Couldn't reach/);
  });
});

// ============================================================
// "Got it" — #howto-done button
// ============================================================

describe("#howto-done: finalize onboarding", () => {
  test("calls mark_onboarded, then spawn_aegis, then closes the window", async () => {
    const { invokeFn, tauri, howtoDone } = await setup({
      verify_invite_code: null,
      save_invite_code: null,
      mark_onboarded: null,
      spawn_aegis: null,
    });

    click(howtoDone());
    await new Promise((r) => setTimeout(r, 10));

    const calls = invokeFn.mock.calls.map(([cmd]) => cmd);
    expect(calls).toContain("mark_onboarded");
    expect(calls).toContain("spawn_aegis");
    expect(tauri._closeMock).toHaveBeenCalledOnce();
  });

  test("mark_onboarded is called before close", async () => {
    const order = [];
    const invokeFn = vi.fn(async (cmd) => {
      order.push(cmd);
    });
    const closeMock = vi.fn(() => order.push("close"));
    window.__TAURI__ = {
      core: { invoke: invokeFn },
      window: { getCurrentWindow: () => ({ close: closeMock }) },
    };

    document.body.innerHTML = HTML_BODY;
    eval(WELCOME_JS); // eslint-disable-line no-eval
    await Promise.resolve();
    await Promise.resolve();

    click(document.getElementById("howto-done"));
    await new Promise((r) => setTimeout(r, 10));

    const markIdx = order.indexOf("mark_onboarded");
    const closeIdx = order.indexOf("close");
    expect(markIdx).toBeGreaterThanOrEqual(0);
    expect(closeIdx).toBeGreaterThan(markIdx);
  });

  test("spawn_aegis is called before close", async () => {
    const order = [];
    const invokeFn = vi.fn(async (cmd) => {
      order.push(cmd);
    });
    const closeMock = vi.fn(() => order.push("close"));
    window.__TAURI__ = {
      core: { invoke: invokeFn },
      window: { getCurrentWindow: () => ({ close: closeMock }) },
    };

    document.body.innerHTML = HTML_BODY;
    eval(WELCOME_JS); // eslint-disable-line no-eval
    await Promise.resolve();
    await Promise.resolve();

    click(document.getElementById("howto-done"));
    await new Promise((r) => setTimeout(r, 10));

    const spawnIdx = order.indexOf("spawn_aegis");
    const closeIdx = order.indexOf("close");
    expect(spawnIdx).toBeGreaterThanOrEqual(0);
    expect(closeIdx).toBeGreaterThan(spawnIdx);
  });

  test("mark_onboarded failure does not prevent close", async () => {
    const { tauri, howtoDone } = await setup({
      mark_onboarded: new Error("db locked"),
      spawn_aegis: null,
    });

    click(howtoDone());
    await new Promise((r) => setTimeout(r, 10));

    expect(tauri._closeMock).toHaveBeenCalledOnce();
  });
});

// ============================================================
// showHowTo: hotkey combo text
// ============================================================

describe("showHowTo: hotkey combo text", () => {
  test("shows Ctrl + Space on non-Mac platform", async () => {
    Object.defineProperty(navigator, "platform", {
      value: "Linux x86_64",
      configurable: true,
    });

    const { invokeFn, codeInput, cursorBtn, hotkeyCombo } = await setup({
      verify_invite_code: null,
      save_invite_code: null,
    });

    codeInput().value = "CODE";
    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(hotkeyCombo().textContent).toBe("Ctrl + Space");
  });

  test("shows control-Space on Mac platform", async () => {
    Object.defineProperty(navigator, "platform", {
      value: "MacIntel",
      configurable: true,
    });

    const { codeInput, cursorBtn, hotkeyCombo } = await setup({
      verify_invite_code: null,
      save_invite_code: null,
    });

    codeInput().value = "CODE";
    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    expect(hotkeyCombo().textContent).toBe("⌃ Space");

    // restore
    Object.defineProperty(navigator, "platform", {
      value: "Linux x86_64",
      configurable: true,
    });
  });
});

// ============================================================
// save_api_keys only called when at least one key entered
// ============================================================

describe("save_api_keys conditional call", () => {
  test("no key fields entered in invite flow → save_api_keys not called", async () => {
    const { invokeFn, codeInput, cursorBtn } = await setup({
      verify_invite_code: null,
      save_invite_code: null,
      save_api_keys: null,
    });

    codeInput().value = "GOODCODE";
    click(cursorBtn());
    await new Promise((r) => setTimeout(r, 0));

    const saveCalls = invokeFn.mock.calls.filter(([cmd]) => cmd === "save_api_keys");
    expect(saveCalls).toHaveLength(0);
  });
});
