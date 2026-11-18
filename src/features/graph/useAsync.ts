// Hook kecil untuk memuat data dari backend dengan keadaan loading/galat.
import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage } from "@/ipc";

export type AsyncState<T> = { status: "loading"; data: T | null; error: null } | { status: "ready"; data: T; error: null } | { status: "error"; data: null; error: string };

/** Menjalankan `load` setiap kali `deps` berubah; hasil dari pemanggilan lama diabaikan. */
export function useAsync<T>(load: () => Promise<T>, deps: readonly unknown[]): AsyncState<T> & { reload: () => void } {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading", data: null, error: null });
  const [tick, setTick] = useState(0);
  const lastTick = useRef(0);
  useEffect(() => {
    let cancelled = false;
    // Data lama tetap ditampilkan selagi memuat ulang, tetapi tidak saat yang diminta berganti.
    const keep = lastTick.current !== tick;
    lastTick.current = tick;
    setState((s) => ({ status: "loading", data: keep ? s.data : null, error: null }));
    load().then(
      (data) => !cancelled && setState({ status: "ready", data, error: null }),
      (e) => !cancelled && setState({ status: "error", data: null, error: errorMessage(e) }),
    );
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { ...state, reload };
}
