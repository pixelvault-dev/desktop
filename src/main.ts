// The menubar tray (built in Rust) is the primary UI. This window is a
// status/settings surface — its main job is the sign-in flow.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface AuthStatus {
  signed_in: boolean;
  email: string | null;
  remaining: number;
}

const account = document.getElementById("account")!;

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
  } else {
    renderSignedOut(status.remaining);
  }
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
