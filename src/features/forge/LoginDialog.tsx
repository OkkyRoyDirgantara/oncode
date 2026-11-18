// Dialog login: device flow (kode + browser) dengan jalan lain berupa Personal Access Token.
import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { Copy, ExternalLink, LoaderCircle } from "lucide-react";
import { commands, errorCode, type Account, type ForgeKind, type LoginStart } from "@/ipc";
import { choose, openDialog, promptInput, toast } from "@/app/overlay";
import { pollDeviceLogin, TOKEN_SCOPES, type Timer } from "./login";
import { loadAccounts } from "./store";
import { describeForgeError, forgeName, hostName, normalizeHost, openExternal } from "./util";

type DevicePhase =
  | { name: "starting" }
  | { name: "waiting"; start: LoginStart; retrying: boolean }
  | { name: "denied" }
  | { name: "expired" }
  | { name: "error"; message: string; detail?: string };

export interface LoginDialogProps {
  kind: ForgeKind;
  /** null = github.com / gitlab.com. */
  host: string | null;
  close: (account: Account | null) => void;
  initialMode?: "device" | "token";
  /** Hanya untuk pengujian. */
  timer?: Timer;
}

export function LoginDialog({ kind, host, close, initialMode = "device", timer }: LoginDialogProps) {
  const [mode, setMode] = useState<"device" | "token">(initialMode);
  const [phase, setPhase] = useState<DevicePhase>({ name: "starting" });
  const [attempt, setAttempt] = useState(0);
  /** Device flow tidak tersedia (Client ID OAuth belum diisi). */
  const [unsupported, setUnsupported] = useState(false);
  const closeRef = useRef(close);
  closeRef.current = close;
  const name = forgeName(kind);
  const where = host ? hostName(host) : kind === "github" ? "github.com" : "gitlab.com";

  useEffect(() => {
    if (mode !== "device") return;
    // Satu sesi per percobaan; pembersihan efek menghentikan polling dan membatalkan login di backend.
    const session = { cancelled: false, stop: () => {} };
    setPhase({ name: "starting" });
    void (async () => {
      let start: LoginStart;
      try {
        start = await commands.forgeLoginStart(kind, host);
      } catch (e) {
        if (session.cancelled) return;
        if (errorCode(e) === "unsupported") {
          setUnsupported(true);
          setMode("token");
          return;
        }
        const text = describeForgeError(e, { host: where });
        setPhase({ name: "error", message: text.title, detail: text.detail });
        return;
      }
      if (session.cancelled) {
        // Dialog sudah ditutup saat permintaan masih berjalan: buang kode yang baru dibuat.
        void commands.forgeLoginCancel(start.userCode).catch(() => undefined);
        return;
      }
      setPhase({ name: "waiting", start, retrying: false });
      const polling = pollDeviceLogin({
        userCode: start.userCode,
        interval: start.interval,
        expiresIn: start.expiresIn,
        poll: (code) => commands.forgeLoginPoll(code),
        cancel: (code) => commands.forgeLoginCancel(code),
        timer,
        onProgress: (p) => {
          if (!session.cancelled) setPhase((current) => (current.name === "waiting" ? { ...current, retrying: p.retrying } : current));
        },
      });
      session.stop = polling.cancel;
      const outcome = await polling.result;
      if (session.cancelled) return;
      switch (outcome.status) {
        case "done":
          void loadAccounts();
          toast.success(`Signed in to ${hostName(outcome.account.host)} as ${outcome.account.username}.`);
          closeRef.current(outcome.account);
          return;
        case "denied":
          setPhase({ name: "denied" });
          return;
        case "expired":
          setPhase({ name: "expired" });
          return;
        case "error": {
          const text = describeForgeError(outcome.error, { host: where });
          setPhase({ name: "error", message: text.title, detail: text.detail });
          return;
        }
        case "cancelled":
          return;
      }
    })();
    return () => {
      session.cancelled = true;
      session.stop();
    };
  }, [mode, attempt, kind, host, where, timer]);

  return (
    <>
      <div className="oc-dialog-header">Sign in to {name}{host ? ` (${where})` : ""}</div>
      {mode === "device" ? (
        <DeviceBody phase={phase} name={name} onRetry={() => setAttempt((n) => n + 1)} onUseToken={() => setMode("token")} onCancel={() => close(null)} />
      ) : (
        <TokenBody
          kind={kind}
          host={host}
          where={where}
          unsupported={unsupported}
          onUseBrowser={unsupported ? undefined : () => setMode("device")}
          onCancel={() => close(null)}
          onDone={(account) => close(account)}
        />
      )}
    </>
  );
}

function DeviceBody({ phase, name, onRetry, onUseToken, onCancel }: { phase: DevicePhase; name: string; onRetry: () => void; onUseToken: () => void; onCancel: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async (code: string) => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
    } catch {
      toast.warning("Could not copy to the clipboard. Select the code and copy it manually.");
    }
  };
  return (
    <>
      <div className="oc-dialog-body">
        {phase.name === "starting" && (
          <p className="fg-login-status" role="status">
            <LoaderCircle size={14} className="oc-spin" /> Requesting a sign-in code from {name}…
          </p>
        )}
        {phase.name === "waiting" && (
          <>
            <p>Enter this code on the {name} page that opens in your browser, then approve access for OnCode.</p>
            <div className="fg-login-code">
              <span className="fg-login-code-text oc-mono oc-selectable" aria-label="Sign-in code">{phase.start.userCode}</span>
              <button className="oc-btn oc-btn-sm" onClick={() => void copy(phase.start.userCode)}>
                <Copy size={13} /> {copied ? "Copied" : "Copy"}
              </button>
            </div>
            <div className="fg-login-url oc-selectable oc-muted">{phase.start.verificationUri}</div>
            <button className="oc-btn oc-btn-primary" onClick={() => void openExternal(phase.start.verificationUriComplete ?? phase.start.verificationUri)}>
              <ExternalLink size={14} /> Open browser
            </button>
            <p className="fg-login-status oc-muted" role="status" style={{ marginTop: 12 }}>
              <LoaderCircle size={14} className="oc-spin" />
              {phase.retrying ? "Connection problem, retrying…" : "Waiting for you to approve in the browser…"}
            </p>
          </>
        )}
        {phase.name === "denied" && <p role="alert">The sign-in request was declined in the browser. Nothing was saved.</p>}
        {phase.name === "expired" && <p role="alert">The sign-in code expired before it was approved. Request a new code to try again.</p>}
        {phase.name === "error" && (
          <div role="alert">
            <p>{phase.message}</p>
            {phase.detail && phase.detail !== phase.message && <p className="oc-muted oc-selectable">{phase.detail}</p>}
          </div>
        )}
        <button className="fg-link" onClick={onUseToken}>
          Use a token instead
        </button>
      </div>
      <div className="oc-dialog-footer">
        <button className="oc-btn" onClick={onCancel}>Cancel</button>
        {(phase.name === "denied" || phase.name === "expired" || phase.name === "error") && (
          <button className="oc-btn oc-btn-primary" onClick={onRetry}>Retry</button>
        )}
      </div>
    </>
  );
}

interface TokenBodyProps {
  kind: ForgeKind;
  host: string | null;
  where: string;
  unsupported: boolean;
  onUseBrowser?: () => void;
  onCancel: () => void;
  onDone: (account: Account) => void;
}

function TokenBody({ kind, host, where, unsupported, onUseBrowser, onCancel, onDone }: TokenBodyProps) {
  // Token tidak pernah masuk state React maupun store: dibaca dari elemen input saat submit lalu dikosongkan.
  const input = useRef<HTMLInputElement>(null);
  const [hasValue, setHasValue] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ title: string; detail?: string } | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    input.current?.focus();
    return () => {
      mounted.current = false;
    };
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const field = input.current;
    const token = field?.value.trim() ?? "";
    if (!token || busy) return;
    setBusy(true);
    setError(null);
    try {
      const account = await commands.forgeLoginToken(kind, host, token);
      if (field) field.value = "";
      void loadAccounts();
      toast.success(`Signed in to ${hostName(account.host)} as ${account.username}.`);
      onDone(account);
    } catch (err) {
      if (!mounted.current) return;
      const text = describeForgeError(err, { host: where });
      setError(errorCode(err) === "unauthorized" ? { title: `${where} rejected this token. Check that it is correct, not expired, and has the scopes listed above.`, detail: text.detail } : text);
      setBusy(false);
    }
  };

  const scopes: readonly string[] = TOKEN_SCOPES[kind];
  return (
    <form onSubmit={(e) => void submit(e)} autoComplete="off">
      <div className="oc-dialog-body">
        {unsupported && (
          <p className="fg-note" role="status">
            Browser sign-in needs an OAuth Client ID, which is not configured. You can set one in Settings → Accounts, or sign in with a personal access token below.
          </p>
        )}
        <p>
          Create a personal access token on {where} with the {scopes.length === 1 ? "scope" : "scopes"}{" "}
          {scopes.map((s, i) => (
            <span key={s}>
              {i > 0 && (i === scopes.length - 1 ? " and " : ", ")}
              <code className="fg-code">{s}</code>
            </span>
          ))}
          , then paste it here. The token is stored only in the system keychain.
        </p>
        <div className="oc-field">
          <label htmlFor="fg-token">Personal access token</label>
          <input
            id="fg-token"
            ref={input}
            className="oc-input oc-mono"
            type="password"
            autoComplete="off"
            spellCheck={false}
            disabled={busy}
            aria-invalid={error ? true : undefined}
            onChange={(e) => setHasValue(e.target.value.trim() !== "")}
          />
        </div>
        {error && (
          <div role="alert">
            <div className="oc-error-text">{error.title}</div>
            {error.detail && error.detail !== error.title && <div className="oc-faint oc-selectable" style={{ fontSize: 12, marginTop: 2 }}>{error.detail}</div>}
          </div>
        )}
        {onUseBrowser && (
          <button type="button" className="fg-link" onClick={onUseBrowser}>
            Sign in with the browser instead
          </button>
        )}
      </div>
      <div className="oc-dialog-footer">
        <button type="button" className="oc-btn" onClick={onCancel}>Cancel</button>
        <button type="submit" className="oc-btn oc-btn-primary" disabled={!hasValue || busy}>
          {busy ? "Signing in…" : "Sign In"}
        </button>
      </div>
    </form>
  );
}

/** Membuka dialog login; selesai dengan akun yang login atau null bila dibatalkan. */
export function signIn(kind: ForgeKind, host: string | null = null, options: { mode?: "device" | "token" } = {}): Promise<Account | null> {
  return openDialog<Account>((close) => <LoginDialog kind={kind} host={host} close={close} initialMode={options.mode} />);
}

/** Login ke instance self-hosted: tanya jenis dan alamatnya lebih dulu. */
export async function signInOtherHost(): Promise<Account | null> {
  const kind = await choose({
    title: "Sign in to another host",
    message: "Which kind of server is it?",
    buttons: [
      { id: "cancel", label: "Cancel" },
      { id: "github", label: "GitHub Enterprise" },
      { id: "gitlab", label: "GitLab (self-hosted)" },
    ],
  });
  if (kind !== "github" && kind !== "gitlab") return null;
  const value = await promptInput({
    title: `Sign in to ${kind === "github" ? "GitHub Enterprise" : "GitLab"}`,
    label: "Server address",
    placeholder: kind === "github" ? "https://github.example.com" : "https://gitlab.example.com",
    confirmLabel: "Continue",
    validate: (v) => (v.trim() === "" || normalizeHost(v) ? null : "Enter the web address of the server, for example https://git.example.com"),
  });
  if (!value) return null;
  const host = normalizeHost(value);
  if (!host) return null;
  return signIn(kind, host);
}
