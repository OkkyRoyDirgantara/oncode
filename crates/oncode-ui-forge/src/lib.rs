//! GitHub and GitLab integration: accounts, pull/merge request lists and details, creating PRs, and
//! cloning from an account.

pub mod accounts;
pub mod clone;
pub mod logic;
pub mod login;
pub mod util;
pub mod views;

use gpui_kit::{App, AppContext as _, Context, Window};
use oncode_ui::commands::Command;
use oncode_ui::{Env, Lucide, SidebarView, TabContent, TabData, TabKind, ViewTab, Workbench};
use serde_json::Value;

use logic::{PrTabParams, PR_TAB_KIND};
use views::{PullRequestTab, PullRequestsView, SIDEBAR_ID};

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    oncode_ui::register_tab_kind(cx, PR_TAB_KIND, PrKind);
    oncode_ui_settings::register_accounts_view(cx, |_, _, cx| accounts::accounts_view(cx).into());
    oncode_ui::register_feature(cx, setup);
}

struct PrKind;

impl TabKind for PrKind {
    fn build(&self, tab: &TabData, env: &Env, _: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let params: PrTabParams = serde_json::from_value(tab.params.clone()).expect("valid PR tab params");
        let env = env.clone();
        Box::new(ViewTab(cx.new(|cx| PullRequestTab::new(env, params, cx))))
    }

    fn icon(&self, _: &TabData) -> Option<Lucide> {
        Some(Lucide::GitPullRequest)
    }
}

fn setup(workbench: &mut Workbench, _: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    let core = env.core.clone();
    // Sign in, sign out, and clone are also available without a folder (welcome screen).
    workbench.register_command(Command::new("forge.signIn", "Sign In to GitHub or GitLab…", |_, window, cx| accounts::sign_in(window, cx)).category("Accounts"));
    workbench.register_command(Command::new("forge.signOut", "Sign Out…", |_, window, cx| accounts::sign_out(window, cx)).category("Accounts"));
    workbench.register_command(Command::new("git.clone", "Clone Repository…", move |_, window, cx| views::clone_repository(core.clone(), window, cx)).category("Git"));
    if !env.trusted || env.repo_root.is_none() {
        return;
    }
    let list = cx.new(|cx| PullRequestsView::new(env.clone(), cx));
    workbench.set_extension(list.clone());
    workbench.register_sidebar(SidebarView::new(SIDEBAR_ID, "Pull Requests", Lucide::GitPullRequest, 50, {
        let list = list.clone();
        move |_, _, _| list.clone().into()
    }));
    let refresh_list = list.clone();
    workbench.register_command(Command::new("forge.refresh", "Refresh", move |_: &Value, _, cx| refresh_list.update(cx, |list, cx| list.reload(cx))).category("Pull Requests"));
    let browser_list = list.clone();
    workbench.register_command(
        Command::new("forge.openInBrowser", "Open Repository in Browser", move |_, window, cx| match browser_list.read(cx).context().map(|c| c.web_url.clone()) {
            Some(url) => {
                util::open_external(&url, cx);
            }
            None => oncode_ui::overlay::toast_info("This repository has no GitHub or GitLab remote.", window, cx),
        })
        .category("Pull Requests"),
    );
    workbench.register_command(Command::new("forge.createPullRequest", "Create Pull Request…", |_, window, cx| views::create_pull_request(window, cx)).category("Pull Requests"));
}

/// This window's PR list (for tests).
pub fn pull_requests(workbench: &Workbench) -> Option<gpui_kit::Entity<PullRequestsView>> {
    workbench.extension::<gpui_kit::Entity<PullRequestsView>>()
}
