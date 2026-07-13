// The menubar tray (built in Rust) is the primary UI. This window is a
// status/settings surface — its main job is the sign-in flow.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface AuthStatus {
  signed_in: boolean;
  email: string | null;
  remaining: number;
}

interface Settings {
  private_uploads: boolean;
  sign_expires_secs: number;
}

const account = document.getElementById("account")!;
const settings = document.getElementById("settings")!;

/** Signed-URL lifetime choices (seconds), shown in the expiry picker. */
const EXPIRY_OPTIONS: { label: string; secs: number }[] = [
  { label: "1 hour", secs: 3600 },
  { label: "1 day", secs: 86400 },
  { label: "7 days", secs: 604800 },
  { label: "30 days", secs: 2592000 },
];

function esc(s: string): string {
  return s.replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!
  );
}

function setMsg(el: HTMLElement, text: string, isError: boolean): void {
  el.textContent = text;
  el.classList.toggle("error", isError);
}

async function refresh(): Promise<void> {
  const status = await invoke<AuthStatus>("auth_status");
  if (status.signed_in && status.email) {
    renderSignedIn(status.email);
    await renderSettings();
  } else {
    renderSignedOut(status.remaining);
    // Private uploads require a signed-in secret key — hide the controls.
    settings.hidden = true;
    settings.innerHTML = "";
  }
}

/** Render the private-uploads controls (only shown while signed in). */
async function renderSettings(): Promise<void> {
  const current = await invoke<Settings>("get_settings");
  const opts = EXPIRY_OPTIONS.map(
    (o) =>
      `<option value="${o.secs}"${
        o.secs === current.sign_expires_secs ? " selected" : ""
      }>${o.label}</option>`
  ).join("");

  settings.innerHTML = `
    <label class="set-row">
      <input id="private-toggle" type="checkbox"${
        current.private_uploads ? " checked" : ""
      } />
      <span>
        <span class="set-title">Private uploads</span>
        <span class="set-sub">Paste a signed URL only you can share, instead of a public link.</span>
      </span>
    </label>
    <div class="set-row set-expiry"${current.private_uploads ? "" : " hidden"}>
      <label class="set-title" for="expiry">Link expires after</label>
      <select id="expiry">${opts}</select>
    </div>
    <p class="acct-msg" id="set-msg"></p>
  `;
  settings.hidden = false;

  const toggle = settings.querySelector<HTMLInputElement>("#private-toggle")!;
  const expiryRow = settings.querySelector<HTMLDivElement>(".set-expiry")!;
  const expiry = settings.querySelector<HTMLSelectElement>("#expiry")!;
  const msg = settings.querySelector<HTMLParagraphElement>("#set-msg")!;

  async function save(): Promise<void> {
    try {
      await invoke("set_settings", {
        privateUploads: toggle.checked,
        signExpiresSecs: Number(expiry.value),
      });
      setMsg(
        msg,
        toggle.checked
          ? "New uploads will be private."
          : "New uploads will be public.",
        false
      );
    } catch (e) {
      setMsg(msg, String(e), true);
    }
  }

  toggle.addEventListener("change", () => {
    expiryRow.hidden = !toggle.checked;
    void save();
  });
  expiry.addEventListener("change", () => void save());
}

function renderSignedOut(remaining: number): void {
  account.innerHTML = `
    <p class="acct-title">Sign in for permanent uploads &amp; history</p>
    <p class="acct-sub">${remaining} free anonymous uploads left · they expire in 30 days.</p>
    <div class="acct-row">
      <input id="email" type="email" placeholder="you@example.com" autocomplete="email" />
      <button id="send" type="button">Send code</button>
    </div>
    <div id="code-step" hidden>
      <div class="acct-row">
        <input id="code" inputmode="numeric" maxlength="6" placeholder="6-digit code" />
        <button id="verify" type="button">Verify</button>
      </div>
    </div>
    <p class="acct-msg" id="msg"></p>
  `;

  const email = account.querySelector<HTMLInputElement>("#email")!;
  const send = account.querySelector<HTMLButtonElement>("#send")!;
  const codeStep = account.querySelector<HTMLDivElement>("#code-step")!;
  const msg = account.querySelector<HTMLParagraphElement>("#msg")!;

  send.addEventListener("click", async () => {
    const value = email.value.trim();
    if (!value) {
      setMsg(msg, "Enter your email.", true);
      return;
    }
    send.disabled = true;
    setMsg(msg, "Sending…", false);
    try {
      await invoke("sign_in_start", { email: value });
      codeStep.hidden = false;
      setMsg(msg, "We emailed you a 6-digit code.", false);
      account.querySelector<HTMLInputElement>("#code")?.focus();
      wireVerify(value, msg);
    } catch (e) {
      setMsg(msg, String(e), true);
    } finally {
      send.disabled = false;
    }
  });
}

function wireVerify(email: string, msg: HTMLElement): void {
  const verify = account.querySelector<HTMLButtonElement>("#verify");
  const code = account.querySelector<HTMLInputElement>("#code");
  if (!verify || !code) return;
  verify.onclick = async () => {
    const c = code.value.trim();
    if (!c) {
      setMsg(msg, "Enter the code.", true);
      return;
    }
    verify.disabled = true;
    setMsg(msg, "Verifying…", false);
    try {
      await invoke("sign_in_complete", { email, code: c });
      await refresh();
    } catch (e) {
      setMsg(msg, String(e), true);
      verify.disabled = false;
    }
  };
}

function renderSignedIn(email: string): void {
  account.innerHTML = `
    <p class="acct-title">Signed in as ${esc(email)}</p>
    <p class="acct-sub">Your uploads are saved to your account and auto-archive after 30 days.</p>
    <button id="signout" type="button" class="secondary">Sign out</button>
  `;
  account
    .querySelector<HTMLButtonElement>("#signout")!
    .addEventListener("click", async () => {
      await invoke("sign_out");
      await refresh();
    });
}

window.addEventListener("DOMContentLoaded", () => {
  refresh().catch((e) => console.error(e));

  // The window is hidden/shown from the tray (and the hard gate), not reloaded,
  // so DOMContentLoaded fires only once. Re-render whenever it regains focus so
  // the counter + auth state are never stale.
  getCurrentWindow()
    .onFocusChanged(({ payload: focused }) => {
      if (focused) refresh().catch((e) => console.error(e));
    })
    .catch((e) => console.error(e));
});
