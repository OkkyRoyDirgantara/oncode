//! Integrasi GitHub dan GitLab: login, daftar repositori, pull/merge request.

use std::time::{SystemTime, UNIX_EPOCH};

use oncode_forge::*;
use oncode_git::Progress;
use serde::Serialize;
use specta::Type;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::error::{AppError, AppResult};
use crate::events::{AccountsChanged, NoticeKind};
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
fn client_id(state: &Shared, kind: ForgeKind) -> AppResult<String> {
    let settings = state.user_settings().forge;
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
async fn keychain<T: Send + 'static>(state: &Shared, f: impl FnOnce(&dyn TokenStore) -> oncode_forge::Result<T> + Send + 'static) -> AppResult<T> {
    let store = state.tokens.clone();
    tokio::task::spawn_blocking(move || f(store.as_ref() as &dyn TokenStore))
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
async fn client(state: &Shared, account_id: &str) -> AppResult<(Account, Box<dyn ForgeProvider + Send + Sync>)> {
    let account = state
        .store
        .accounts()
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| AppError::new("unauthorized", "Account not found; please log in again"))?;
    let host = ForgeHost { kind: account.kind, host: account.host.clone() };
    let id = account_id.to_string();
    let mut tokens = keychain(state, move |store| store.load(&id))
        .await?
        .ok_or_else(|| AppError::new("unauthorized", "The account token is not in the keychain; please log in again"))?;
    if let (Some(expires_at), Some(refresh)) = (tokens.expires_at, tokens.refresh_token.clone()) {
        if now_seconds() > expires_at - 60.0 {
            if let Ok(id) = client_id(state, account.kind) {
                // Refresh token GitLab sekali pakai: simpan pasangan baru sebelum dipakai.
                tokens = refresh_token(&host, &id, &refresh, None).await?;
                let (account_id, saved) = (account_id.to_string(), tokens.clone());
                keychain(state, move |store| store.save(&account_id, &saved)).await?;
            }
        }
    }
    Ok((account, provider(&host, &tokens.access_token, None)))
}

async fn finish_login(app: &AppHandle, state: &Shared, host: &ForgeHost, tokens: TokenSet) -> AppResult<Account> {
    let account = provider(host, &tokens.access_token, None).current_user().await?;
    let id = account.id.clone();
    keychain(state, move |store| store.save(&id, &tokens)).await?;
    // Tanpa keychain, token hanya ada di memori; akunnya pun hanya dicatat untuk sesi ini.
    state.store.upsert_account(account.clone(), !state.tokens.is_memory_only());
    let _ = AccountsChanged {}.emit(app);
    if state.tokens.is_memory_only() {
        state.notify(
            NoticeKind::Warning,
            "No system keychain is available, so the sign-in is kept in memory only. You will need to sign in again after restarting OnCode.",
        );
    }
    Ok(account)
}

#[tauri::command]
#[specta::specta]
pub async fn forge_accounts(state: State<'_, Shared>) -> AppResult<Vec<Account>> {
    Ok(state.store.accounts())
}

/// Yang ditampilkan ke pengguna saat login device flow. `device_code` sendiri tetap di backend.
#[derive(Debug, Clone, Serialize, Type)]
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
#[tauri::command]
#[specta::specta]
pub async fn forge_login_start(state: State<'_, Shared>, kind: ForgeKind, host: Option<String>) -> AppResult<LoginStart> {
    let host = host_for(kind, host)?;
    let code = device_start(&host, &client_id(&state, kind)?, None).await?;
    state.pending_logins.lock().unwrap().insert(code.user_code.clone(), (host, code.device_code.clone()));
    Ok(LoginStart {
        user_code: code.user_code,
        verification_uri: code.verification_uri,
        verification_uri_complete: code.verification_uri_complete,
        interval: code.interval,
        expires_in: code.expires_in,
    })
}

#[derive(Debug, Clone, Serialize, Type)]
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
#[tauri::command]
#[specta::specta]
pub async fn forge_login_poll(app: AppHandle, state: State<'_, Shared>, user_code: String) -> AppResult<LoginPoll> {
    let Some((host, device_code)) = state.pending_logins.lock().unwrap().get(&user_code).cloned() else {
        return Ok(LoginPoll::Expired);
    };
    let outcome = match device_poll(&host, &client_id(&state, host.kind)?, &device_code, None).await {
        Ok(DevicePoll::Pending) => return Ok(LoginPoll::Pending),
        Ok(DevicePoll::SlowDown) => return Ok(LoginPoll::SlowDown),
        // Galat jaringan bisa dicoba lagi; login tetap menunggu.
        Err(e) if e.code == ForgeErrorCode::Network => return Err(e.into()),
        Ok(DevicePoll::Denied) => Ok(LoginPoll::Denied),
        Ok(DevicePoll::Expired) => Ok(LoginPoll::Expired),
        Ok(DevicePoll::Token(tokens)) => finish_login(&app, &state, &host, tokens).await.map(|account| LoginPoll::Done { account }),
        Err(e) => Err(e.into()),
    };
    state.pending_logins.lock().unwrap().remove(&user_code);
    outcome
}

/// Membatalkan login yang sedang menunggu.
#[tauri::command]
#[specta::specta]
pub async fn forge_login_cancel(state: State<'_, Shared>, user_code: String) -> AppResult<()> {
    state.pending_logins.lock().unwrap().remove(&user_code);
    Ok(())
}

/// Login dengan Personal Access Token (juga untuk instance self-hosted tanpa device flow).
#[tauri::command]
#[specta::specta]
pub async fn forge_login_token(app: AppHandle, state: State<'_, Shared>, kind: ForgeKind, host: Option<String>, token: String) -> AppResult<Account> {
    let host = host_for(kind, host)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::invalid("Token is empty"));
    }
    finish_login(&app, &state, &host, TokenSet { access_token: token, refresh_token: None, expires_at: None }).await
}

#[tauri::command]
#[specta::specta]
pub async fn forge_logout(app: AppHandle, state: State<'_, Shared>, account_id: String) -> AppResult<()> {
    let id = account_id.clone();
    let _ = keychain(&state, move |store| store.delete(&id)).await;
    state.store.remove_account(&account_id);
    let _ = AccountsChanged {}.emit(&app);
    Ok(())
}

/// Hubungan sebuah repositori lokal dengan forge, diturunkan dari URL remote-nya.
#[derive(Debug, Clone, Serialize, Type)]
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

#[tauri::command]
#[specta::specta]
pub async fn forge_context(state: State<'_, Shared>, root: String) -> AppResult<Option<ForgeContext>> {
    let remotes = state.repo(&root)?.remotes().await?;
    let accounts = state.store.accounts();
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

#[tauri::command]
#[specta::specta]
pub async fn forge_repos(state: State<'_, Shared>, account_id: String, query: String, page: u32) -> AppResult<Page<RemoteRepo>> {
    let (_, provider) = client(&state, &account_id).await?;
    Ok(provider.list_repos(&query, page.max(1)).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn forge_prs(state: State<'_, Shared>, account_id: String, repo: RepoRef, filter: PrFilter, page: u32) -> AppResult<Page<PullRequest>> {
    let (_, provider) = client(&state, &account_id).await?;
    Ok(provider.list_pull_requests(&repo, filter, page.max(1)).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn forge_pr(state: State<'_, Shared>, account_id: String, repo: RepoRef, number: u32) -> AppResult<PullRequestDetail> {
    let (_, provider) = client(&state, &account_id).await?;
    Ok(provider.get_pull_request(&repo, number).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn forge_checks(state: State<'_, Shared>, account_id: String, repo: RepoRef, sha: String) -> AppResult<Vec<Check>> {
    let (_, provider) = client(&state, &account_id).await?;
    Ok(provider.list_checks(&repo, &sha).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn forge_default_branch(state: State<'_, Shared>, account_id: String, repo: RepoRef) -> AppResult<String> {
    let (_, provider) = client(&state, &account_id).await?;
    Ok(provider.default_branch(&repo).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn forge_pr_create(state: State<'_, Shared>, account_id: String, repo: RepoRef, input: NewPullRequest) -> AppResult<PullRequest> {
    let (_, provider) = client(&state, &account_id).await?;
    Ok(provider.create_pull_request(&repo, &input).await?)
}

/// Mengambil ref PR/MR ke branch lokal `pr/<nomor>` lalu pindah ke sana. Mengembalikan nama branch.
#[tauri::command]
#[specta::specta]
pub async fn forge_pr_checkout(
    state: State<'_, Shared>,
    root: String,
    kind: ForgeKind,
    remote: String,
    number: u32,
    operation_id: String,
    on_progress: Channel<Progress>,
) -> AppResult<String> {
    let repo = state.repo(&root)?;
    let branch = format!("pr/{number}");
    let refspec = format!("+{}:refs/heads/{branch}", pull_request_ref(kind, number));
    let cancel = state.begin_operation(&operation_id);
    let progress: oncode_git::ProgressFn = std::sync::Arc::new(move |p| {
        let _ = on_progress.send(p);
    });
    let result = async {
        repo.fetch_refspec(&remote, &refspec, Some(cancel), Some(progress)).await?;
        repo.switch(&branch).await
    }
    .await;
    state.end_operation(&operation_id);
    state.refresh(&root);
    state.refresh_refs(&root);
    result?;
    Ok(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
