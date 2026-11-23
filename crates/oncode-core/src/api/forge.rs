//! Integrasi GitHub dan GitLab: login, daftar repositori, pull/merge request.

use std::time::{SystemTime, UNIX_EPOCH};

use oncode_forge::*;
use oncode_git::ProgressFn;
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::events::{CoreEvent, NoticeKind};
use crate::state::Shared;

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

/// Client ID aplikasi OAuth: dari pengaturan, atau yang ditanam saat build.
fn client_id(core: &Shared, kind: ForgeKind) -> AppResult<String> {
    let settings = core.user_settings().forge;
    let (configured, built_in) = match kind {
        ForgeKind::GitHub => (settings.github_client_id, option_env!("ONCODE_GITHUB_CLIENT_ID")),
        ForgeKind::GitLab => (settings.gitlab_client_id, option_env!("ONCODE_GITLAB_CLIENT_ID")),
    };
    configured
        .filter(|c| !c.trim().is_empty())
        .or_else(|| built_in.map(String::from))
        .ok_or_else(|| AppError::new("unsupported", "Browser login is not configured (OAuth client ID is empty); log in with a token instead"))
}

/// Akses keychain bisa memblokir (mis. menunggu dialog buka kunci), jadi dijalankan di thread tersendiri.
async fn keychain<T: Send + 'static>(core: &Shared, f: impl FnOnce(&dyn TokenStore) -> oncode_forge::Result<T> + Send + 'static) -> AppResult<T> {
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

/// Nama host tanpa skema, port, dan path; sebanding dengan `RepoRef.host`.
fn hostname(origin: &str) -> String {
    let authority = origin.split("://").nth(1).unwrap_or(origin).split('/').next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    // IPv6 dalam kurung siku tidak dipotong di titik dua.
    let host = if host.starts_with('[') { host } else { host.split(':').next().unwrap_or(host) };
    host.to_lowercase()
}

/// Provider untuk sebuah akun; token GitLab yang kedaluwarsa diperbarui lebih dulu.
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
            if let Ok(id) = client_id(core, account.kind) {
                // Refresh token GitLab sekali pakai: simpan pasangan baru sebelum dipakai.
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
    // Tanpa keychain, token hanya ada di memori; akunnya pun hanya dicatat untuk sesi ini.
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

/// Yang ditampilkan ke pengguna saat login device flow. `device_code` sendiri tetap di backend.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStart {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    /// Jeda polling dalam detik.
    pub interval: u32,
    pub expires_in: u32,
}

/// Memulai login device flow: UI menampilkan kode dan membuka halaman verifikasi di browser.
pub async fn forge_login_start(core: &Shared, kind: ForgeKind, host: Option<String>) -> AppResult<LoginStart> {
    let host = host_for(kind, host)?;
    let code = device_start(&host, &client_id(core, kind)?, None).await?;
    core.pending_logins.lock().unwrap().insert(code.user_code.clone(), (host, code.device_code.clone()));
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
    /// Server meminta jeda polling diperpanjang (tambah 5 detik).
    SlowDown,
    Denied,
    Expired,
    Done { account: Account },
}

/// Satu kali polling untuk login yang dimulai dengan `forge_login_start`.
pub async fn forge_login_poll(core: &Shared, user_code: String) -> AppResult<LoginPoll> {
    let Some((host, device_code)) = core.pending_logins.lock().unwrap().get(&user_code).cloned() else {
        return Ok(LoginPoll::Expired);
    };
    let outcome = match device_poll(&host, &client_id(core, host.kind)?, &device_code, None).await {
        Ok(DevicePoll::Pending) => return Ok(LoginPoll::Pending),
        Ok(DevicePoll::SlowDown) => return Ok(LoginPoll::SlowDown),
        // Galat jaringan bisa dicoba lagi; login tetap menunggu.
        Err(e) if e.code == ForgeErrorCode::Network => return Err(e.into()),
        Ok(DevicePoll::Denied) => Ok(LoginPoll::Denied),
        Ok(DevicePoll::Expired) => Ok(LoginPoll::Expired),
        Ok(DevicePoll::Token(tokens)) => finish_login(core, &host, tokens).await.map(|account| LoginPoll::Done { account }),
        Err(e) => Err(e.into()),
    };
    core.pending_logins.lock().unwrap().remove(&user_code);
    outcome
}

/// Membatalkan login yang sedang menunggu.
pub async fn forge_login_cancel(core: &Shared, user_code: String) -> AppResult<()> {
    core.pending_logins.lock().unwrap().remove(&user_code);
    Ok(())
}

/// Login dengan Personal Access Token (juga untuk instance self-hosted tanpa device flow).
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

/// Hubungan sebuah repositori lokal dengan forge, diturunkan dari URL remote-nya.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeContext {
    pub remote: String,
    pub repo: RepoRef,
    /// Jenis forge bila bisa dikenali dari akun atau nama host.
    pub kind: Option<ForgeKind>,
    /// Akun yang sudah login untuk host ini.
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

/// Mengambil ref PR/MR ke branch lokal `pr/<nomor>` lalu pindah ke sana. Mengembalikan nama branch.
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
        assert_eq!(hostname("https://git.contoh.id:8443/sub"), "git.contoh.id");
        assert_eq!(hostname("https://[::1]:8443"), "[::1]:8443");
    }

    #[test]
    fn host_for_normalises_user_input() {
        assert_eq!(host_for(ForgeKind::GitHub, None).unwrap().host, "https://github.com");
        assert_eq!(host_for(ForgeKind::GitLab, Some("git.contoh.id/".into())).unwrap().host, "https://git.contoh.id");
        assert_eq!(host_for(ForgeKind::GitLab, Some("http://127.0.0.1:8080".into())).unwrap().host, "http://127.0.0.1:8080");
    }

    #[tokio::test]
    async fn token_login_stores_the_account_and_logout_removes_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/user"))
            .and(header("authorization", "Bearer token-uji"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 7, "username": "budi", "name": "Budi Santoso", "avatar_url": null })))
            .mount(&server)
            .await;
        let mut t = TestCore::new();

        assert_eq!(forge_login_token(&t.core, ForgeKind::GitLab, Some(server.uri()), "  ".into()).await.unwrap_err().code, "invalidInput");
        let account = forge_login_token(&t.core, ForgeKind::GitLab, Some(format!("{}/", server.uri())), " token-uji\n".into()).await.unwrap();
        assert_eq!((account.kind, account.username.as_str(), account.host.as_str()), (ForgeKind::GitLab, "budi", server.uri().as_str()));
        recv(&mut t.events, |e| matches!(e, CoreEvent::AccountsChanged).then_some(())).await;
        assert_eq!(forge_accounts(&t.core).await.unwrap(), vec![account.clone()]);
        assert_eq!(t.core.tokens.load(&account.id).unwrap().unwrap().access_token, "token-uji");
        // Akun tersimpan di state.json (keychain uji berfungsi), token tidak.
        let state = std::fs::read_to_string(t.dir.path().join("app/data/state.json")).unwrap();
        assert!(state.contains("\"budi\"") && !state.contains("token-uji"));

        forge_logout(&t.core, account.id.clone()).await.unwrap();
        recv(&mut t.events, |e| matches!(e, CoreEvent::AccountsChanged).then_some(())).await;
        assert!(forge_accounts(&t.core).await.unwrap().is_empty());
        assert!(t.core.tokens.load(&account.id).unwrap().is_none());
        // Akun yang sudah logout tidak bisa dipakai lagi.
        assert_eq!(forge_repos(&t.core, account.id, String::new(), 1).await.err().map(|e| e.code).as_deref(), Some("unauthorized"));
    }

    #[tokio::test]
    async fn pending_logins_are_tracked_by_user_code() {
        let t = TestCore::new();
        assert_eq!(forge_login_poll(&t.core, "TIDAK-ADA".into()).await.unwrap(), LoginPoll::Expired);
        t.core.pending_logins.lock().unwrap().insert("ABCD-1234".into(), (ForgeHost::github(), "kode-perangkat".into()));
        forge_login_cancel(&t.core, "ABCD-1234".into()).await.unwrap();
        assert!(t.core.pending_logins.lock().unwrap().is_empty());
        // Tanpa Client ID (pengaturan kosong dan tidak ditanam saat build) login browser tidak tersedia.
        if option_env!("ONCODE_GITHUB_CLIENT_ID").is_none() {
            assert_eq!(forge_login_start(&t.core, ForgeKind::GitHub, None).await.unwrap_err().code, "unsupported");
        }
    }

    #[tokio::test]
    async fn context_is_derived_from_the_origin_remote() {
        let t = TestCore::new();
        let project = t.trusted_repo("proyek");
        let root = git_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();
        assert_eq!(forge_context(&t.core, root.clone()).await.unwrap(), None);

        git(&project, &["remote", "add", "cadangan", "https://gitlab.contoh.id/tim/proyek.git"]);
        let context = forge_context(&t.core, root.clone()).await.unwrap().expect("konteks dari remote");
        assert_eq!((context.remote.as_str(), context.kind, context.web_url.as_str()), ("cadangan", Some(ForgeKind::GitLab), "https://gitlab.contoh.id/tim/proyek"));

        // `origin` didahulukan; akun yang login untuk host itu ikut dikembalikan.
        git(&project, &["remote", "add", "origin", "git@github.com:ana/proyek.git"]);
        let account = Account {
            id: "github:github.com:ana".into(),
            kind: ForgeKind::GitHub,
            host: "https://github.com".into(),
            username: "ana".into(),
            display_name: "Ana".into(),
            avatar_url: None,
        };
        t.core.store.upsert_account(account.clone(), true);
        let context = forge_context(&t.core, root).await.unwrap().expect("konteks dari origin");
        assert_eq!(context.remote, "origin");
        assert_eq!(context.repo, RepoRef { host: "github.com".into(), owner: "ana".into(), name: "proyek".into() });
        assert_eq!((context.kind, context.account, context.web_url.as_str()), (Some(ForgeKind::GitHub), Some(account), "https://github.com/ana/proyek"));
        assert_eq!(forge_context(&t.core, "/tidak/dibuka".into()).await.unwrap_err().code, "notARepo");
    }
}
