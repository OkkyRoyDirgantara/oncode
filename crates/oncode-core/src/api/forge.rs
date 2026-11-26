//! GitHub and GitLab integration: login, repository list, pull/merge requests.

use std::time::{SystemTime, UNIX_EPOCH};

use oncode_forge::*;
use oncode_git::ProgressFn;
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::events::{CoreEvent, NoticeKind};
use crate::state::{PendingLogin, Shared};

fn host_for(kind: ForgeKind, host: Option<String>) -> AppResult<ForgeHost> {
    match host.map(|h| h.trim().trim_end_matches('/').to_string()).filter(|h| !h.is_empty()) {
        None => Ok(match kind {
            ForgeKind::GitHub => ForgeHost::github(),
            ForgeKind::GitLab => ForgeHost::gitlab(),
        }),
        Some(host) if host.starts_with("https://") || host.starts_with("http://") => Ok(ForgeHost { kind, host }),
        Some(host) => Ok(ForgeHost { kind, host: format!("https://{host}") }),
    }
}

/// The `forge.clientIds` key for a server: its origin in lowercase.
fn client_id_key(host: &ForgeHost) -> String {
    host.web_base().to_ascii_lowercase()
}

/// The OAuth app Client ID for a server. The order is: that server's own Client ID
/// (`forge.clientIds`), the general Client ID from the settings, then the one baked in at build time. The
/// last one is only for github.com / gitlab.com, because an OAuth app is only known to the
/// server where it was registered.
fn client_id_for(core: &Shared, host: &ForgeHost) -> Option<String> {
    let settings = core.user_settings().forge;
    let (configured, built_in) = match host.kind {
        ForgeKind::GitHub => (settings.github_client_id, option_env!("ONCODE_GITHUB_CLIENT_ID")),
        ForgeKind::GitLab => (settings.gitlab_client_id, option_env!("ONCODE_GITLAB_CLIENT_ID")),
    };
    let built_in = built_in.filter(|_| host.is_public()).map(String::from);
    [settings.client_ids.get(&client_id_key(host)).cloned(), configured, built_in]
        .into_iter()
        .flatten()
        .map(|id| id.trim().to_string())
        .find(|id| !id.is_empty())
}

/// Stores the OAuth app Client ID for one server in `forge.clientIds`; an empty string
/// removes it so that server goes back to using the general Client ID.
pub async fn forge_set_client_id(core: &Shared, kind: ForgeKind, host: Option<String>, client_id: String) -> AppResult<()> {
    let host = host_for(kind, host)?;
    let client_id = client_id.trim().to_string();
    if client_id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AppError::invalid("A Client ID cannot contain spaces"));
    }
    if client_id.chars().count() > 200 {
        return Err(AppError::invalid("This is too long for a Client ID"));
    }
    let mut settings = core.user_settings();
    let key = client_id_key(&host);
    if client_id.is_empty() {
        settings.forge.client_ids.remove(&key);
    } else {
        settings.forge.client_ids.insert(key, client_id);
    }
    crate::api::app::settings_set(core, settings).await
}

/// Keychain access can block (e.g. waiting for an unlock dialog), so it runs on its own thread.
pub(crate) async fn keychain<T: Send + 'static>(core: &Shared, f: impl FnOnce(&dyn TokenStore) -> oncode_forge::Result<T> + Send + 'static) -> AppResult<T> {
    let store = core.tokens.clone();
    core.runtime()
        .spawn_blocking(move || f(store.as_ref() as &dyn TokenStore))
        .await
        .map_err(|e| AppError::new("keychain", e.to_string()))?
        .map_err(Into::into)
}

fn now_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Host name without scheme, port, and path; comparable to `RepoRef.host`.
fn hostname(origin: &str) -> String {
    let authority = origin.split("://").nth(1).unwrap_or(origin).split('/').next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    // IPv6 in square brackets is not cut at the colon.
    let host = if host.starts_with('[') { host } else { host.split(':').next().unwrap_or(host) };
    host.to_lowercase()
}

/// The provider for an account; an expired GitLab token is refreshed first.
async fn client(core: &Shared, account_id: &str) -> AppResult<(Account, Box<dyn ForgeProvider + Send + Sync>)> {
    let account = core
        .store
        .accounts()
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| AppError::new("unauthorized", "Account not found; please log in again"))?;
    let host = ForgeHost { kind: account.kind, host: account.host.clone() };
    let id = account_id.to_string();
    let mut tokens = keychain(core, move |store| store.load(&id))
        .await?
        .ok_or_else(|| AppError::new("unauthorized", "The account token is not in the keychain; please log in again"))?;
    if let (Some(expires_at), Some(refresh)) = (tokens.expires_at, tokens.refresh_token.clone()) {
        if now_seconds() > expires_at - 60.0 {
            if let Some(id) = client_id_for(core, &host) {
                // GitLab refresh tokens are single-use: store the new pair before using it.
                tokens = refresh_token(&host, &id, &refresh, None).await?;
                let (account_id, saved) = (account_id.to_string(), tokens.clone());
                keychain(core, move |store| store.save(&account_id, &saved)).await?;
            }
        }
    }
    Ok((account, provider(&host, &tokens.access_token, None)))
}

async fn finish_login(core: &Shared, host: &ForgeHost, tokens: TokenSet) -> AppResult<Account> {
    let account = provider(host, &tokens.access_token, None).current_user().await?;
    let id = account.id.clone();
    keychain(core, move |store| store.save(&id, &tokens)).await?;
    // Without a keychain, the token exists only in memory; the account is also only recorded for this session.
    core.store.upsert_account(account.clone(), !core.tokens.is_memory_only());
    core.emit(CoreEvent::AccountsChanged);
    if core.tokens.is_memory_only() {
        core.notify(
            NoticeKind::Warning,
            "No system keychain is available, so the sign-in is kept in memory only. You will need to sign in again after restarting OnCode.",
        );
    }
    Ok(account)
}

pub async fn forge_accounts(core: &Shared) -> AppResult<Vec<Account>> {
    Ok(core.store.accounts())
}

/// What is shown to the user during a device flow login. The `device_code` itself stays in the backend.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStart {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    /// Polling interval in seconds.
    pub interval: u32,
    pub expires_in: u32,
}

/// Starts a device flow login: the UI shows the code and opens the verification page in the browser.
///
/// The `noClientId` error means there is no Client ID for that server yet (see [`forge_set_client_id`]);
/// logging in with a token is still possible, including via the [`ForgeHost::token_creation_url`] page.
pub async fn forge_login_start(core: &Shared, kind: ForgeKind, host: Option<String>) -> AppResult<LoginStart> {
    let host = host_for(kind, host)?;
    let client_id = client_id_for(core, &host).ok_or_else(|| {
        AppError::new("noClientId", format!("Browser sign-in to {} needs the Client ID of an OAuth application registered on that server", host.hostname()))
    })?;
    let code = device_start(&host, &client_id, None).await?;
    core.pending_logins.lock().unwrap().insert(code.user_code.clone(), PendingLogin { host, device_code: code.device_code.clone(), client_id });
    Ok(LoginStart {
        user_code: code.user_code,
        verification_uri: code.verification_uri,
        verification_uri_complete: code.verification_uri_complete,
        interval: code.interval,
        expires_in: code.expires_in,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum LoginPoll {
    Pending,
    /// The server asks for the polling interval to be extended (add 5 seconds).
    SlowDown,
    Denied,
    Expired,
    Done { account: Account },
}

/// A single poll for a login started with `forge_login_start`.
pub async fn forge_login_poll(core: &Shared, user_code: String) -> AppResult<LoginPoll> {
    let Some(PendingLogin { host, device_code, client_id }) = core.pending_logins.lock().unwrap().get(&user_code).cloned() else {
        return Ok(LoginPoll::Expired);
    };
    let outcome = match device_poll(&host, &client_id, &device_code, None).await {
        Ok(DevicePoll::Pending) => return Ok(LoginPoll::Pending),
        Ok(DevicePoll::SlowDown) => return Ok(LoginPoll::SlowDown),
        // A network error can be retried; the login keeps waiting.
        Err(e) if e.code == ForgeErrorCode::Network => return Err(e.into()),
        Ok(DevicePoll::Denied) => Ok(LoginPoll::Denied),
        Ok(DevicePoll::Expired) => Ok(LoginPoll::Expired),
        Ok(DevicePoll::Token(tokens)) => finish_login(core, &host, tokens).await.map(|account| LoginPoll::Done { account }),
        Err(e) => Err(e.into()),
    };
    core.pending_logins.lock().unwrap().remove(&user_code);
    outcome
}

/// Cancels a login that is waiting.
pub async fn forge_login_cancel(core: &Shared, user_code: String) -> AppResult<()> {
    core.pending_logins.lock().unwrap().remove(&user_code);
    Ok(())
}

/// Logs in with a Personal Access Token (also for self-hosted instances without device flow).
pub async fn forge_login_token(core: &Shared, kind: ForgeKind, host: Option<String>, token: String) -> AppResult<Account> {
    let host = host_for(kind, host)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::invalid("Token is empty"));
    }
    finish_login(core, &host, TokenSet { access_token: token, refresh_token: None, expires_at: None }).await
}

pub async fn forge_logout(core: &Shared, account_id: String) -> AppResult<()> {
    let id = account_id.clone();
    let _ = keychain(core, move |store| store.delete(&id)).await;
    core.store.remove_account(&account_id);
    core.emit(CoreEvent::AccountsChanged);
    Ok(())
}

/// The relationship of a local repository to a forge, derived from its remote URL.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeContext {
    pub remote: String,
    pub repo: RepoRef,
    /// The forge kind if it can be recognized from the account or host name.
    pub kind: Option<ForgeKind>,
    /// The account already logged in for this host.
    pub account: Option<Account>,
    pub web_url: String,
}

pub async fn forge_context(core: &Shared, root: String) -> AppResult<Option<ForgeContext>> {
    let remotes = core.repo(&root)?.remotes().await?;
    let accounts = core.store.accounts();
    let ordered = remotes.iter().filter(|r| r.name == "origin").chain(remotes.iter().filter(|r| r.name != "origin"));
    for remote in ordered {
        let Some(repo) = parse_remote_url(&remote.fetch_url) else { continue };
        let account = accounts.iter().find(|a| hostname(&a.host) == repo.host).cloned();
        let kind = account.as_ref().map(|a| a.kind).or_else(|| {
            if repo.host == "github.com" {
                Some(ForgeKind::GitHub)
            } else if repo.host.contains("gitlab") {
                Some(ForgeKind::GitLab)
            } else {
                None
            }
        });
        let web_url = format!("https://{}/{}/{}", repo.host, repo.owner, repo.name);
        return Ok(Some(ForgeContext { remote: remote.name.clone(), repo, kind, account, web_url }));
    }
    Ok(None)
}

pub async fn forge_repos(core: &Shared, account_id: String, query: String, page: u32) -> AppResult<Page<RemoteRepo>> {
    let (_, provider) = client(core, &account_id).await?;
    Ok(provider.list_repos(&query, page.max(1)).await?)
}

pub async fn forge_prs(core: &Shared, account_id: String, repo: RepoRef, filter: PrFilter, page: u32) -> AppResult<Page<PullRequest>> {
    let (_, provider) = client(core, &account_id).await?;
    Ok(provider.list_pull_requests(&repo, filter, page.max(1)).await?)
}

pub async fn forge_pr(core: &Shared, account_id: String, repo: RepoRef, number: u32) -> AppResult<PullRequestDetail> {
    let (_, provider) = client(core, &account_id).await?;
    Ok(provider.get_pull_request(&repo, number).await?)
}

pub async fn forge_checks(core: &Shared, account_id: String, repo: RepoRef, sha: String) -> AppResult<Vec<Check>> {
    let (_, provider) = client(core, &account_id).await?;
    Ok(provider.list_checks(&repo, &sha).await?)
}

pub async fn forge_default_branch(core: &Shared, account_id: String, repo: RepoRef) -> AppResult<String> {
    let (_, provider) = client(core, &account_id).await?;
    Ok(provider.default_branch(&repo).await?)
}

pub async fn forge_pr_create(core: &Shared, account_id: String, repo: RepoRef, input: NewPullRequest) -> AppResult<PullRequest> {
    let (_, provider) = client(core, &account_id).await?;
    Ok(provider.create_pull_request(&repo, &input).await?)
}

/// Fetches the PR/MR ref into the local branch `pr/<number>` and then switches to it. Returns the branch name.
pub async fn forge_pr_checkout(
    core: &Shared,
    root: String,
    kind: ForgeKind,
    remote: String,
    number: u32,
    operation_id: String,
    on_progress: ProgressFn,
) -> AppResult<String> {
    let repo = core.repo(&root)?;
    let branch = format!("pr/{number}");
    let refspec = format!("+{}:refs/heads/{branch}", pull_request_ref(kind, number));
    let cancel = core.begin_operation(&operation_id);
    let result = async {
        repo.fetch_refspec(&remote, &refspec, Some(cancel), Some(on_progress)).await?;
        repo.switch(&branch).await
    }
    .await;
    core.end_operation(&operation_id);
    core.refresh(&root);
    core.refresh_refs(&root);
    result?;
    Ok(branch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::git::git_open;
    use crate::testing::{git, recv, TestCore};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn hostname_drops_scheme_port_and_path() {
        assert_eq!(hostname("https://GitHub.com"), "github.com");
        assert_eq!(hostname("http://127.0.0.1:8080"), "127.0.0.1");
        assert_eq!(hostname("https://git.example.com:8443/sub"), "git.example.com");
        assert_eq!(hostname("https://[::1]:8443"), "[::1]:8443");
    }

    #[test]
    fn host_for_normalises_user_input() {
        assert_eq!(host_for(ForgeKind::GitHub, None).unwrap().host, "https://github.com");
        assert_eq!(host_for(ForgeKind::GitLab, Some("git.example.com/".into())).unwrap().host, "https://git.example.com");
        assert_eq!(host_for(ForgeKind::GitLab, Some("http://127.0.0.1:8080".into())).unwrap().host, "http://127.0.0.1:8080");
    }

    #[tokio::test]
    async fn token_login_stores_the_account_and_logout_removes_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/user"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 7, "username": "bob", "name": "Bob Smith", "avatar_url": null })))
            .mount(&server)
            .await;
        let mut t = TestCore::new();

        assert_eq!(forge_login_token(&t.core, ForgeKind::GitLab, Some(server.uri()), "  ".into()).await.unwrap_err().code, "invalidInput");
        let account = forge_login_token(&t.core, ForgeKind::GitLab, Some(format!("{}/", server.uri())), " test-token\n".into()).await.unwrap();
        assert_eq!((account.kind, account.username.as_str(), account.host.as_str()), (ForgeKind::GitLab, "bob", server.uri().as_str()));
        recv(&mut t.events, |e| matches!(e, CoreEvent::AccountsChanged).then_some(())).await;
        assert_eq!(forge_accounts(&t.core).await.unwrap(), vec![account.clone()]);
        assert_eq!(t.core.tokens.load(&account.id).unwrap().unwrap().access_token, "test-token");
        // The account is stored in state.json (the test keychain works), the token is not.
        let state = std::fs::read_to_string(t.dir.path().join("app/data/state.json")).unwrap();
        assert!(state.contains("\"bob\"") && !state.contains("test-token"));

        forge_logout(&t.core, account.id.clone()).await.unwrap();
        recv(&mut t.events, |e| matches!(e, CoreEvent::AccountsChanged).then_some(())).await;
        assert!(forge_accounts(&t.core).await.unwrap().is_empty());
        assert!(t.core.tokens.load(&account.id).unwrap().is_none());
        // An account that has logged out can no longer be used.
        assert_eq!(forge_repos(&t.core, account.id, String::new(), 1).await.err().map(|e| e.code).as_deref(), Some("unauthorized"));
    }

    #[tokio::test]
    async fn pending_logins_are_tracked_by_user_code() {
        let t = TestCore::new();
        assert_eq!(forge_login_poll(&t.core, "DOES-NOT-EXIST".into()).await.unwrap(), LoginPoll::Expired);
        let pending = PendingLogin { host: ForgeHost::github(), device_code: "device-code".into(), client_id: "client".into() };
        t.core.pending_logins.lock().unwrap().insert("ABCD-1234".into(), pending);
        forge_login_cancel(&t.core, "ABCD-1234".into()).await.unwrap();
        assert!(t.core.pending_logins.lock().unwrap().is_empty());
        // Without a Client ID (empty settings and none baked in at build time) browser login is unavailable.
        if option_env!("ONCODE_GITHUB_CLIENT_ID").is_none() {
            assert_eq!(forge_login_start(&t.core, ForgeKind::GitHub, None).await.unwrap_err().code, "noClientId");
        }
        // The build's default Client ID is never used for self-hosted servers.
        assert_eq!(forge_login_start(&t.core, ForgeKind::GitLab, Some("git.example.com".into())).await.unwrap_err().code, "noClientId");
    }

    #[tokio::test]
    async fn client_id_prefers_the_server_then_the_general_setting() {
        let t = TestCore::new();
        let own = ForgeHost::new(ForgeKind::GitLab, "git.example.com");
        assert_eq!(client_id_for(&t.core, &own), None);

        // The general Client ID (old behavior) also applies to your own server.
        let mut settings = t.core.user_settings();
        settings.forge.gitlab_client_id = Some("general".into());
        t.core.set_user_settings(settings).unwrap();
        assert_eq!(client_id_for(&t.core, &own).as_deref(), Some("general"));

        // The server's own Client ID takes precedence; the key is the normalized origin.
        forge_set_client_id(&t.core, ForgeKind::GitLab, Some("https://Git.Example.com/".into()), " specific ".into()).await.unwrap();
        assert_eq!(t.core.user_settings().forge.client_ids.get("https://git.example.com").map(String::as_str), Some("specific"));
        assert_eq!(client_id_for(&t.core, &own).as_deref(), Some("specific"));
        assert_eq!(client_id_for(&t.core, &ForgeHost::gitlab()).as_deref(), Some("general"));
        // Stored in the settings file.
        assert!(crate::Settings::load(t.core.settings_path()).forge.client_ids.contains_key("https://git.example.com"));

        assert_eq!(forge_set_client_id(&t.core, ForgeKind::GitLab, Some("git.example.com".into()), "two words".into()).await.unwrap_err().code, "invalidInput");
        assert_eq!(forge_set_client_id(&t.core, ForgeKind::GitLab, Some("git.example.com".into()), "x".repeat(201)).await.unwrap_err().code, "invalidInput");

        // An empty string removes the server's own Client ID.
        forge_set_client_id(&t.core, ForgeKind::GitLab, Some("git.example.com".into()), String::new()).await.unwrap();
        assert_eq!(client_id_for(&t.core, &own).as_deref(), Some("general"));
    }

    #[tokio::test]
    async fn device_login_uses_the_client_id_of_the_server() {
        use wiremock::matchers::body_string_contains;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/authorize_device"))
            .and(body_string_contains("client_id=server-client"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dev-1", "user_code": "WXYZ-0001", "verification_uri": format!("{}/oauth/device", server.uri()), "expires_in": 300, "interval": 1
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("client_id=server-client"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "access_token": "token-server", "refresh_token": "fresh", "expires_in": 7200, "token_type": "Bearer" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/user"))
            .and(header("authorization", "Bearer token-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 9, "username": "sally", "name": "Sally", "avatar_url": null })))
            .mount(&server)
            .await;
        let t = TestCore::new();
        forge_set_client_id(&t.core, ForgeKind::GitLab, Some(server.uri()), "server-client".into()).await.unwrap();

        let start = forge_login_start(&t.core, ForgeKind::GitLab, Some(server.uri())).await.unwrap();
        assert_eq!(start.user_code, "WXYZ-0001");
        // Polling uses the Client ID from when the login started, even if the setting changes midway.
        forge_set_client_id(&t.core, ForgeKind::GitLab, Some(server.uri()), "other".into()).await.unwrap();
        let LoginPoll::Done { account } = forge_login_poll(&t.core, start.user_code).await.unwrap() else { panic!("login not finished yet") };
        assert_eq!(account.username, "sally");
        assert_eq!(t.core.tokens.load(&account.id).unwrap().unwrap().access_token, "token-server");
    }

    #[tokio::test]
    async fn context_is_derived_from_the_origin_remote() {
        let t = TestCore::new();
        let project = t.trusted_repo("project");
        let root = git_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();
        assert_eq!(forge_context(&t.core, root.clone()).await.unwrap(), None);

        git(&project, &["remote", "add", "backup", "https://gitlab.example.com/team/project.git"]);
        let context = forge_context(&t.core, root.clone()).await.unwrap().expect("context from remote");
        assert_eq!((context.remote.as_str(), context.kind, context.web_url.as_str()), ("backup", Some(ForgeKind::GitLab), "https://gitlab.example.com/team/project"));

        // `origin` takes precedence; the account logged in for that host is also returned.
        git(&project, &["remote", "add", "origin", "git@github.com:ana/project.git"]);
        let account = Account {
            id: "github:github.com:ana".into(),
            kind: ForgeKind::GitHub,
            host: "https://github.com".into(),
            username: "ana".into(),
            display_name: "Ana".into(),
            avatar_url: None,
        };
        t.core.store.upsert_account(account.clone(), true);
        let context = forge_context(&t.core, root).await.unwrap().expect("context from origin");
        assert_eq!(context.remote, "origin");
        assert_eq!(context.repo, RepoRef { host: "github.com".into(), owner: "ana".into(), name: "project".into() });
        assert_eq!((context.kind, context.account, context.web_url.as_str()), (Some(ForgeKind::GitHub), Some(account), "https://github.com/ana/project"));
        assert_eq!(forge_context(&t.core, "/not/opened".into()).await.unwrap_err().code, "notARepo");
    }
}
