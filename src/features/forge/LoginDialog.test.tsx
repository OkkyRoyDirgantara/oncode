import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { openUrl } from "@tauri-apps/plugin-opener";
import { mockCommands, type Account, type LoginPoll, type LoginStart } from "@/ipc";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { appError, githubAccount } from "./fixtures";
import { LoginDialog, signIn } from "./LoginDialog";
import { resetForgeForTest, useForge } from "./store";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => undefined) }));

const start: LoginStart = {
  userCode: "WDJB-MJHT",
  verificationUri: "https://github.com/login/device",
  verificationUriComplete: "https://github.com/login/device?user_code=WDJB-MJHT",
  interval: 5,
  expiresIn: 900,
};

/** Membiarkan promise IPC (mikrotask) selesai di bawah timer palsu. */
async function flush(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("LoginDialog", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetForgeForTest();
    resetOverlayForTest();
    vi.mocked(openUrl).mockClear();
  });
  afterEach(() => {
    vi.useRealTimers();
    clearMocks();
  });

  it("menampilkan kode, membuka browser, dan menutup dengan akun setelah polling selesai", async () => {
    const answers: LoginPoll[] = [{ status: "pending" }, { status: "done", account: githubAccount }];
    const poll = vi.fn(() => answers.shift() ?? ({ status: "pending" } as LoginPoll));
    const cancel = vi.fn(() => null);
    const startCall = vi.fn(() => start);
    mockCommands({ forgeLoginStart: startCall, forgeLoginPoll: poll, forgeLoginCancel: cancel, forgeAccounts: () => [githubAccount] });
    const close = vi.fn();
    render(
      <>
        <LoginDialog kind="github" host={null} close={close} />
        <OverlayHost />
      </>,
    );
    await flush();
    expect(startCall).toHaveBeenCalledWith({ kind: "github", host: null });
    expect(screen.getByLabelText("Sign-in code")).toHaveTextContent("WDJB-MJHT");
    expect(screen.getByText("https://github.com/login/device")).toBeInTheDocument();
    expect(screen.getByText(/Waiting for you to approve/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Open browser/ }));
    expect(openUrl).toHaveBeenCalledWith("https://github.com/login/device?user_code=WDJB-MJHT");

    await flush(5000);
    expect(poll).toHaveBeenCalledTimes(1);
    expect(poll).toHaveBeenCalledWith({ userCode: "WDJB-MJHT" });
    expect(close).not.toHaveBeenCalled();

    await flush(5000);
    expect(poll).toHaveBeenCalledTimes(2);
    expect(close).toHaveBeenCalledWith(githubAccount);
    expect(screen.getByText("Signed in to github.com as octocat.")).toBeInTheDocument();
    expect(cancel).not.toHaveBeenCalled();
    expect(useForge.getState().accounts).toEqual([githubAccount]);
  });

  it("beralih ke mode token bila device flow tidak dikonfigurasi (unsupported)", async () => {
    const poll = vi.fn();
    const token = vi.fn((_args: { kind: string; host: string | null; token: string }): Account => githubAccount);
    mockCommands({
      forgeLoginStart: () => {
        throw appError("unsupported", "Login lewat browser belum dikonfigurasi");
      },
      forgeLoginPoll: poll,
      forgeLoginToken: token,
      forgeAccounts: () => [githubAccount],
    });
    const close = vi.fn();
    render(<LoginDialog kind="github" host={null} close={close} />);
    await flush();

    expect(screen.getByText(/Browser sign-in needs an OAuth Client ID/)).toBeInTheDocument();
    expect(screen.getByText(/Settings → Accounts/)).toBeInTheDocument();
    expect(screen.getByText("repo")).toBeInTheDocument();
    expect(screen.getByText("read:user")).toBeInTheDocument();
    const field = screen.getByLabelText("Personal access token") as HTMLInputElement;
    expect(field.type).toBe("password");
    expect(field.autocomplete).toBe("off");
    const submit = screen.getByRole("button", { name: "Sign In" });
    expect(submit).toBeDisabled();
    // Tanpa Client ID, tidak ada jalan kembali ke login browser.
    expect(screen.queryByText("Sign in with the browser instead")).toBeNull();

    fireEvent.change(field, { target: { value: "  ghp_secret-token-value  " } });
    expect(submit).toBeEnabled();
    fireEvent.click(submit);
    await flush();

    expect(token).toHaveBeenCalledWith({ kind: "github", host: null, token: "ghp_secret-token-value" });
    expect(close).toHaveBeenCalledWith(githubAccount);
    // Nilai token tidak tertinggal di DOM maupun di store mana pun.
    expect(field.value).toBe("");
    expect(JSON.stringify(useForge.getState())).not.toContain("ghp_secret-token-value");
    expect(document.body.innerHTML).not.toContain("ghp_secret-token-value");
    await flush(60_000);
    expect(poll).not.toHaveBeenCalled();
  });

  it("scope GitLab ditampilkan untuk GitLab", async () => {
    mockCommands({
      forgeLoginStart: () => {
        throw appError("unsupported");
      },
    });
    render(<LoginDialog kind="gitlab" host="https://gitlab.example.com" close={vi.fn()} />);
    await flush();
    expect(screen.getByText("api")).toBeInTheDocument();
    expect(screen.getByText("read_user")).toBeInTheDocument();
    expect(screen.getByText(/Create a personal access token on gitlab\.example\.com/)).toBeInTheDocument();
  });

  it("token yang ditolak menampilkan pesan dan dialog tetap terbuka", async () => {
    mockCommands({
      forgeLoginStart: () => {
        throw appError("unsupported");
      },
      forgeLoginToken: () => {
        throw appError("unauthorized", "Token ditolak");
      },
    });
    const close = vi.fn();
    render(<LoginDialog kind="github" host={null} close={close} />);
    await flush();
    fireEvent.change(screen.getByLabelText("Personal access token"), { target: { value: "bad-token" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign In" }));
    await flush();
    expect(screen.getByRole("alert")).toHaveTextContent(/github\.com rejected this token/);
    expect(screen.getByRole("alert")).not.toHaveTextContent("bad-token");
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Sign In" })).toBeEnabled();
  });

  it("menutup dialog saat menunggu menghentikan polling dan memanggil forgeLoginCancel", async () => {
    const poll = vi.fn((): LoginPoll => ({ status: "pending" }));
    const cancel = vi.fn(() => null);
    mockCommands({ forgeLoginStart: () => start, forgeLoginPoll: poll, forgeLoginCancel: cancel });
    render(<OverlayHost />);
    let result: Account | null | undefined;
    act(() => {
      void signIn("github").then((r) => (result = r));
    });
    await flush();
    await flush(5000);
    expect(poll).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await flush();
    expect(result).toBeNull();
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(cancel).toHaveBeenCalledWith({ userCode: "WDJB-MJHT" });
    expect(screen.queryByLabelText("Sign-in code")).toBeNull();

    await flush(120_000);
    expect(poll).toHaveBeenCalledTimes(1);
  });

  it("Escape juga membatalkan login yang sedang menunggu", async () => {
    const poll = vi.fn((): LoginPoll => ({ status: "pending" }));
    const cancel = vi.fn(() => null);
    mockCommands({ forgeLoginStart: () => start, forgeLoginPoll: poll, forgeLoginCancel: cancel });
    render(<OverlayHost />);
    act(() => void signIn("github"));
    await flush();
    fireEvent.keyDown(window, { key: "Escape" });
    await flush();
    expect(cancel).toHaveBeenCalledWith({ userCode: "WDJB-MJHT" });
    await flush(60_000);
    expect(poll).not.toHaveBeenCalled();
  });

  it("beralih ke token secara manual membatalkan device flow", async () => {
    const poll = vi.fn((): LoginPoll => ({ status: "pending" }));
    const cancel = vi.fn(() => null);
    mockCommands({ forgeLoginStart: () => start, forgeLoginPoll: poll, forgeLoginCancel: cancel });
    render(<LoginDialog kind="github" host={null} close={vi.fn()} />);
    await flush();
    fireEvent.click(screen.getByText("Use a token instead"));
    await flush();
    expect(cancel).toHaveBeenCalledWith({ userCode: "WDJB-MJHT" });
    expect(screen.getByLabelText("Personal access token")).toBeInTheDocument();
    // Client ID ada, jadi login browser tetap ditawarkan.
    expect(screen.getByText("Sign in with the browser instead")).toBeInTheDocument();
    await flush(60_000);
    expect(poll).not.toHaveBeenCalled();
  });

  it("StrictMode (efek dijalankan dua kali): kode pertama dibatalkan, hanya kode kedua yang di-polling", async () => {
    let n = 0;
    const poll = vi.fn((_args: { userCode: string }): LoginPoll => ({ status: "pending" }));
    const cancel = vi.fn((_args: { userCode: string }) => null);
    mockCommands({ forgeLoginStart: () => ({ ...start, userCode: `CODE-${++n}` }), forgeLoginPoll: poll, forgeLoginCancel: cancel });
    render(
      <StrictMode>
        <LoginDialog kind="github" host={null} close={vi.fn()} />
      </StrictMode>,
    );
    await flush();
    await flush(5000);
    expect(n).toBe(2);
    expect(cancel.mock.calls.map((c) => c[0].userCode)).toEqual(["CODE-1"]);
    expect(screen.getByLabelText("Sign-in code")).toHaveTextContent("CODE-2");
    expect(poll.mock.calls.map((c) => c[0].userCode)).toEqual(["CODE-2"]);
  });

  it("ditolak dan kedaluwarsa menawarkan Retry yang meminta kode baru", async () => {
    const answers: LoginPoll[] = [{ status: "denied" }, { status: "expired" }];
    const startCall = vi.fn(() => start);
    mockCommands({ forgeLoginStart: startCall, forgeLoginPoll: () => answers.shift() ?? { status: "pending" }, forgeLoginCancel: () => null });
    render(<LoginDialog kind="github" host={null} close={vi.fn()} />);
    await flush();
    await flush(5000);
    expect(screen.getByRole("alert")).toHaveTextContent(/declined/);
    expect(screen.queryByLabelText("Sign-in code")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await flush();
    expect(startCall).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText("Sign-in code")).toBeInTheDocument();
    await flush(5000);
    expect(screen.getByRole("alert")).toHaveTextContent(/expired/);
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.getByText("Use a token instead")).toBeInTheDocument();
  });
});
