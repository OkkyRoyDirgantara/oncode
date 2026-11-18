// Pemberitahuan dari backend (mis. keychain tidak tersedia) ditampilkan sebagai toast.
import { events } from "@/ipc";
import { toast } from "./overlay";

export function installNotices(): void {
  void events.notice.listen((e) => {
    if (e.payload.kind === "error") toast.error(e.payload.message);
    else toast.warning(e.payload.message);
  });
}
