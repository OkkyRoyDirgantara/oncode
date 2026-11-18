// Menjawab permintaan dari git (kredensial, konfirmasi host SSH, pesan commit) lewat dialog.
import { choose, promptInput } from "@/app/overlay";
import { commands, events, type PromptRequest } from "@/ipc";

/** Judul dialog yang menjelaskan apa yang diminta git. */
export function promptTitle(request: PromptRequest): string {
  const text = request.prompt.toLowerCase();
  if (request.kind === "message") return "Commit message";
  if (request.kind === "confirm") return "Confirm host";
  if (text.includes("passphrase")) return "SSH key passphrase";
  if (text.startsWith("username")) return "Git credentials: username";
  if (text.startsWith("password")) return "Git credentials: password or token";
  return "Git needs your input";
}

async function ask(request: PromptRequest): Promise<string | null> {
  switch (request.kind) {
    case "confirm": {
      const answer = await choose({
        title: promptTitle(request),
        message: request.prompt.trim(),
        buttons: [
          { id: "no", label: "No" },
          { id: "yes", label: "Yes, Continue", kind: "primary" },
        ],
      });
      // Menutup dialog berarti menolak; ssh mengharapkan jawaban eksplisit.
      return answer === "yes" ? "yes" : "no";
    }
    case "message":
      return promptInput({
        title: promptTitle(request),
        message: "Git is asking for a commit message (for example while rebasing with reword or squash).",
        value: request.initial,
        multiline: true,
        confirmLabel: "Use Message",
      });
    case "secret":
      return promptInput({ title: promptTitle(request), message: request.prompt.trim(), password: true, confirmLabel: "Continue" });
    case "text":
      return promptInput({ title: promptTitle(request), message: request.prompt.trim(), confirmLabel: "Continue" });
  }
}

export function installPromptHandler(): void {
  void events.promptRequest.listen((e) => {
    void ask(e.payload)
      .catch(() => null)
      .then((value) => commands.promptRespond(e.payload.id, value));
  });
}
