//! Satu trait untuk semua penyedia, plus pembantu yang bergantung pada jenis penyedia.

use crate::error::{ForgeError, Result};
use crate::github::GitHubProvider;
use crate::gitlab::GitLabProvider;
use crate::model::{
    Account, Check, ForgeHost, ForgeKind, NewPullRequest, Page, PrFilter, PullRequest, PullRequestDetail, RemoteRepo,
    RepoRef,
};

/// Operasi yang didukung semua penyedia. Nomor halaman mulai dari 1 (0 dianggap 1);
/// untuk halaman berikutnya, kirim kembali `Page::next_page` apa adanya.
#[async_trait::async_trait]
pub trait ForgeProvider: Send + Sync {
    /// Akun pemilik token. Dipakai juga untuk memvalidasi token (termasuk PAT) saat login.
    async fn current_user(&self) -> Result<Account>;

    /// Repositori yang bisa diakses akun ini, yang paling baru diperbarui lebih dulu.
    /// `query` menyaring berdasarkan nama; string kosong berarti tanpa saringan.
    ///
    /// GitLab mencari di sisi server. GitHub tidak punya pencarian sisi server untuk
    /// "repositori yang bisa saya akses", jadi di sana daftar dipindai di sisi klien:
    /// satu panggilan bisa mengembalikan lebih atau kurang dari 30 butir (bahkan nol)
    /// sementara `next_page` masih terisi.
    async fn list_repos(&self, query: &str, page: u32) -> Result<Page<RemoteRepo>>;

    /// PR/MR di `repo`, yang paling baru diperbarui lebih dulu.
    async fn list_pull_requests(&self, repo: &RepoRef, filter: PrFilter, page: u32) -> Result<Page<PullRequest>>;

    /// Detail satu PR/MR.
    async fn get_pull_request(&self, repo: &RepoRef, number: u32) -> Result<PullRequestDetail>;

    /// Pemeriksaan CI untuk commit `sha`. GitHub: check run digabung dengan commit status.
    /// GitLab: job dari pipeline terbaru untuk commit itu, atau status pipelinenya bila
    /// belum ada job. Kosong bila commit tidak punya CI.
    async fn list_checks(&self, repo: &RepoRef, sha: &str) -> Result<Vec<Check>>;

    /// Membuat PR/MR. Branch sumber harus sudah ada di remote.
    async fn create_pull_request(&self, repo: &RepoRef, input: &NewPullRequest) -> Result<PullRequest>;

    /// Branch bawaan repositori (untuk mengisi awal branch tujuan).
    async fn default_branch(&self, repo: &RepoRef) -> Result<String>;
}

/// Ref Git yang di-fetch untuk checkout PR/MR `number`: GitHub `refs/pull/{n}/head`,
/// GitLab `refs/merge-requests/{n}/head`.
pub fn pull_request_ref(kind: ForgeKind, number: u32) -> String {
    match kind {
        ForgeKind::GitHub => format!("refs/pull/{number}/head"),
        ForgeKind::GitLab => format!("refs/merge-requests/{number}/head"),
    }
}

/// Pasangan username/password yang dipakai git lewat HTTPS untuk host ini dengan token
/// OAuth atau PAT: GitHub `("x-access-token", token)`, GitLab `("oauth2", token)`.
///
/// Nilai kedua adalah rahasia: jangan dicatat ke log dan jangan dijadikan argumen perintah.
pub fn git_credentials(kind: ForgeKind, token: &str) -> (String, String) {
    let username = match kind {
        ForgeKind::GitHub => "x-access-token",
        ForgeKind::GitLab => "oauth2",
    };
    (username.to_string(), token.to_string())
}

/// Membuat penyedia untuk `host` dengan `token`. `api_base_override` mengganti basis
/// REST, supaya tes bisa menunjuk ke server tiruan.
pub fn provider(host: &ForgeHost, token: &str, api_base_override: Option<&str>) -> Box<dyn ForgeProvider + Send + Sync> {
    match host.kind {
        ForgeKind::GitHub => Box::new(GitHubProvider::new(host, token, api_base_override)),
        ForgeKind::GitLab => Box::new(GitLabProvider::new(host, token, api_base_override)),
    }
}

/// Memeriksa masukan PR/MR sebelum dikirim ke server.
pub(crate) fn validate_new_pull_request(input: &NewPullRequest) -> Result<()> {
    if input.title.trim().is_empty() {
        return Err(ForgeError::invalid("Title must not be empty"));
    }
    if input.source_branch.trim().is_empty() {
        return Err(ForgeError::invalid("Source branch must not be empty"));
    }
    if input.target_branch.trim().is_empty() {
        return Err(ForgeError::invalid("Target branch must not be empty"));
    }
    if input.source_branch.trim() == input.target_branch.trim() {
        return Err(ForgeError::invalid("Source and target branches must be different"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ForgeErrorCode;

    #[test]
    fn validates_new_pull_request() {
        let valid = NewPullRequest {
            title: "Judul".into(),
            body: String::new(),
            source_branch: "fitur".into(),
            target_branch: "main".into(),
            draft: false,
        };
        assert!(validate_new_pull_request(&valid).is_ok());
        for broken in [
            NewPullRequest { title: "  ".into(), ..valid.clone() },
            NewPullRequest { source_branch: String::new(), ..valid.clone() },
            NewPullRequest { target_branch: String::new(), ..valid.clone() },
            NewPullRequest { source_branch: "main".into(), ..valid.clone() },
        ] {
            let err = validate_new_pull_request(&broken).expect_err("harus ditolak");
            assert_eq!(err.code, ForgeErrorCode::InvalidInput);
        }
    }

    #[test]
    fn pull_request_refs() {
        assert_eq!(pull_request_ref(ForgeKind::GitHub, 42), "refs/pull/42/head");
        assert_eq!(pull_request_ref(ForgeKind::GitLab, 7), "refs/merge-requests/7/head");
    }

    #[test]
    fn git_credentials_per_kind() {
        assert_eq!(
            git_credentials(ForgeKind::GitHub, "gho_abc"),
            ("x-access-token".to_string(), "gho_abc".to_string())
        );
        assert_eq!(git_credentials(ForgeKind::GitLab, "glpat-abc"), ("oauth2".to_string(), "glpat-abc".to_string()));
    }

    #[test]
    fn provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        let github = provider(&ForgeHost::github(), "token", None);
        let gitlab = provider(&ForgeHost::gitlab(), "token", None);
        assert_send_sync(&github);
        assert_send_sync(&gitlab);
    }
}
