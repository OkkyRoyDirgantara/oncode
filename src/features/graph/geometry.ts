// Geometri potongan graph per baris: posisi jalur, bentuk garis, dan lebar SVG.
import type { Edge, GraphRow } from "@/ipc";

export const LANE_WIDTH = 14;
export const ROW_HEIGHT = 26;
export const DOT_RADIUS = 4;
/** Jalur di luar batas ini tidak digambar (SVG dipotong). */
export const MAX_LANES = 20;
/** Jumlah warna jalur di tema (`--lane-0` … `--lane-7`). */
export const LANE_COLORS = 8;

/** Koordinat x titik tengah sebuah jalur. */
export function laneX(lane: number): number {
  return lane * LANE_WIDTH + LANE_WIDTH / 2;
}

export function laneColor(color: number): string {
  return `var(--lane-${((color % LANE_COLORS) + LANE_COLORS) % LANE_COLORS})`;
}

/**
 * Path SVG untuk satu garis pada baris setinggi `height`.
 * - `through`: lurus dari atas ke bawah.
 * - `top`: dari jalur `from` di tepi atas menuju titik commit di jalur `to`.
 * - `bottom`: dari titik commit di jalur `from` menuju jalur `to` di tepi bawah.
 * Garis yang berpindah jalur berbelok dengan sudut membulat lalu mendatar ke titik commit.
 */
export function edgePath(edge: Pick<Edge, "from" | "to" | "kind">, height: number = ROW_HEIGHT): string {
  const mid = height / 2;
  const x1 = laneX(edge.from);
  const x2 = laneX(edge.to);
  if (edge.kind === "through") return `M ${x1} 0 V ${height}`;
  if (x1 === x2) return edge.kind === "top" ? `M ${x1} 0 V ${mid}` : `M ${x1} ${mid} V ${height}`;
  const dir = x2 > x1 ? 1 : -1;
  const r = Math.min(LANE_WIDTH / 2, mid, Math.abs(x2 - x1));
  if (edge.kind === "top") {
    // Turun di jalur asal, belok, lalu mendatar ke titik commit.
    return `M ${x1} 0 V ${mid - r} Q ${x1} ${mid} ${x1 + dir * r} ${mid} H ${x2}`;
  }
  // Mendatar dari titik commit, belok, lalu turun di jalur tujuan.
  return `M ${x1} ${mid} H ${x2 - dir * r} Q ${x2} ${mid} ${x2} ${mid + r} V ${height}`;
}

/** Jalur paling kanan yang dipakai oleh baris-baris ini (-1 bila kosong). */
export function maxLane(rows: readonly GraphRow[]): number {
  let max = -1;
  for (const row of rows) {
    if (row.column > max) max = row.column;
    for (const edge of row.edges) {
      if (edge.from > max) max = edge.from;
      if (edge.to > max) max = edge.to;
    }
  }
  return max;
}

/** Lebar SVG untuk graph dengan jalur terkanan `lane`, dibatasi `MAX_LANES`. */
export function graphWidth(lane: number): number {
  return Math.min(Math.max(lane + 1, 1), MAX_LANES) * LANE_WIDTH;
}
