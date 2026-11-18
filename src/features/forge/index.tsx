// Fitur forge (GitHub/GitLab): sidebar Pull Requests, tab detail PR/MR, login, buat PR, dan clone.
import { GitPullRequest } from "lucide-react";
import { registerCommands } from "@/app/commands";
import { quickPick, toast, type PickItem } from "@/app/overlay";
import { registerSidebarView, registerTabType } from "@/app/registry";
import { revealSidebar, useApp } from "@/app/store";
import { commands, type Account, type ForgeKind } from "@/ipc";
import { activeRepo } from "@/features/scm/store";
import { cloneRepository, initRepository } from "./CloneDialog";
import { createPullRequest } from "./CreatePrDialog";
import { ForgeView, signOut } from "./ForgeView";
import { signIn, signInOtherHost } from "./LoginDialog";
import { PullRequestTab } from "./PrTab";
import { installForgeStore, loadAccounts, refreshForge, useForge } from "./store";
import { describeForgeError, forgeName, hostName, openExternal } from "./util";

async function signInCommand(kind?: ForgeKind, host?: string | null): Promise<Account | null> {
  if (kind === "github" || kind === "gitlab") return signIn(kind, typeof host === "string" ? host : null);
  const items: PickItem<"github" | "gitlab" | "other">[] = [
    { label: "GitHub", description: "github.com", value: "github" },
    { label: "GitLab", description: "gitlab.com", value: "gitlab" },
    { label: "Other host…", description: "GitHub Enterprise or self-hosted GitLab", value: "other" },
  ];
  const picked = await quickPick({ placeholder: "Sign in to…", items });
  if (!picked) return null;
  return picked === "other" ? signInOtherHost() : signIn(picked);
}

async function signOutCommand(): Promise<void> {
  const accounts = await loadAccounts();
  if (accounts.length === 0) {
    toast.info("No account is signed in.");
    return;
  }
  const account =
    accounts.length === 1
      ? accounts[0]
      : await quickPick({
          placeholder: "Sign out which account?",
          items: accounts.map((a) => ({ label: a.username, description: `${forgeName(a.kind)} · ${hostName(a.host)}`, value: a })),
        });
  if (account) await signOut(account);
}

async function openInBrowserCommand(): Promise<void> {
  const repo = activeRepo();
  if (!repo) {
    toast.info("Open a Git repository first.");
    return;
  }
  try {
    const context = useForge.getState().root === repo.root && useForge.getState().context ? useForge.getState().context : await commands.forgeContext(repo.root);
    if (!context) {
      toast.info("No remote of this repository points to GitHub or GitLab.");
      return;
    }
    await openExternal(context.webUrl);
  } catch (e) {
    toast.error(describeForgeError(e).title);
  }
}

export function registerForgeFeature(): void {
  installForgeStore();

  registerSidebarView({
    id: "pullRequests",
    title: "Pull Requests",
    icon: <GitPullRequest size={20} />,
    order: 50,
    component: ForgeView,
  });

  registerTabType({
    kind: "pullRequest",
    component: PullRequestTab,
    icon: () => <GitPullRequest size={14} />,
  });

  const hasRepo = () => activeRepo() !== null;
  registerCommands([
    { id: "forge.signIn", title: "Sign In to GitHub or GitLab…", category: "Accounts", run: signInCommand },
    { id: "forge.signOut", title: "Sign Out…", category: "Accounts", run: signOutCommand },
    {
      id: "forge.refresh",
      title: "Refresh",
      category: "Pull Requests",
      run: async () => {
        revealSidebar("pullRequests");
        await refreshForge();
      },
    },
    { id: "forge.openInBrowser", title: "Open Repository in Browser", category: "Pull Requests", when: hasRepo, run: openInBrowserCommand },
    { id: "forge.createPullRequest", title: "Create Pull Request…", category: "Pull Requests", when: hasRepo, run: () => createPullRequest() },
    { id: "git.clone", title: "Clone Repository…", category: "Git", when: () => !!useApp.getState().info?.git, run: (url?: unknown) => cloneRepository(typeof url === "string" ? url : undefined) },
    {
      id: "git.init",
      title: "Initialize Repository",
      category: "Git",
      when: () => {
        const { workspace, info } = useApp.getState();
        return !!workspace && workspace.trusted && !workspace.repoRoot && !!info?.git;
      },
      run: initRepository,
    },
  ]);
}

export { cloneRepository, createPullRequest, signIn };
