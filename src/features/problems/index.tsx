// Fitur Problems: panel bawah `problems` dan ringkasan di status bar.
import { registerPanelView, registerStatusItem } from "@/app/registry";
import { ProblemsPanel, ProblemsStatus, useProblemsBadge } from "./ProblemsPanel";

export function registerProblemsFeature(): void {
  registerPanelView({ id: "problems", title: "Problems", order: 20, component: ProblemsPanel, useBadge: useProblemsBadge });
  registerStatusItem({ id: "problems.summary", align: "left", order: 20, component: ProblemsStatus });
}
