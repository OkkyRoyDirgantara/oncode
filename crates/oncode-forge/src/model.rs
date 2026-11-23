//! Struktur data yang dikirim ke UI. Semua field memakai camelCase di sisi TypeScript.

use serde::{Deserialize, Serialize};

pub use crate::remote::parse_remote_url;

/// Detik sejak epoch Unix. `f64` agar aman diekspor sebagai `number` di TypeScript.
pub type Timestamp = f64;

/// Jenis penyedia hosting Git.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ForgeKind {
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "gitlab")]
    GitLab,
}

impl ForgeKind {
    /// Nama pendek yang stabil: `"github"` atau `"gitlab"`. Dipakai di id akun.
    pub fn as_str(self) -> &'static str {
        match self {
            ForgeKind::GitHub => "github",
            ForgeKind::GitLab => "gitlab",
        }
    }
}

/// Satu instance forge. `host` adalah origin web TANPA garis miring di akhir, misalnya
/// `"https://github.com"`, `"https://gitlab.com"`, atau `"https://git.perusahaan.id"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeHost {
    pub kind: ForgeKind,
    pub host: String,
}

impl ForgeHost {
    /// Membuat host dari masukan pengguna: spasi dan garis miring di akhir dibuang, dan
    /// `https://` ditambahkan bila skema tidak ditulis.
    pub fn new(kind: ForgeKind, host: &str) -> Self {
        let trimmed = host.trim().trim_end_matches('/');
        let host = if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
        Self { kind, host }
    }

    /// github.com.
    pub fn github() -> Self {
        Self { kind: ForgeKind::GitHub, host: "https://github.com".to_string() }
    }

    /// gitlab.com.
    pub fn gitlab() -> Self {
        Self { kind: ForgeKind::GitLab, host: "https://gitlab.com".to_string() }
    }

    /// Origin web tanpa garis miring di akhir.
    pub fn web_base(&self) -> &str {
        self.host.trim().trim_end_matches('/')
    }

    /// Nama host saja, huruf kecil, tanpa skema, port, maupun path. Bentuknya sama dengan
    /// [`RepoRef::host`], sehingga keduanya bisa langsung dibandingkan.
    pub fn hostname(&self) -> String {
        let rest = match self.host.split_once("://") {
            Some((_, rest)) => rest,
            None => self.host.as_str(),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
        let host = if authority.starts_with('[') {
            match authority.find(']') {
                Some(end) => &authority[..=end],
                None => authority,
            }
        } else {
            authority.split(':').next().unwrap_or_default()
        };
        host.trim().to_ascii_lowercase()
    }

    /// Basis REST: github.com → `https://api.github.com`; GitHub Enterprise Cloud dengan
    /// residensi data (`*.ghe.com`) → `https://api.{host}`; GitHub Enterprise Server →
    /// `{host}/api/v3`; GitLab → `{host}/api/v4`.
    pub fn api_base(&self) -> String {
        let base = self.web_base();
        match self.kind {
            ForgeKind::GitHub => {
                let hostname = self.hostname();
                if hostname == "github.com" || hostname == "www.github.com" {
                    "https://api.github.com".to_string()
                } else if hostname.ends_with(".ghe.com") {
                    format!("https://api.{hostname}")
                } else {
                    format!("{base}/api/v3")
                }
            }
            ForgeKind::GitLab => format!("{base}/api/v4"),
        }
    }

    /// `true` bila `repo` berada di host ini (perbandingan nama host, tanpa port).
    pub fn matches(&self, repo: &RepoRef) -> bool {
        self.hostname() == repo.host.to_ascii_lowercase()
    }

    /// `true` untuk layanan publik (github.com, gitlab.com), bukan instance sendiri.
    pub fn is_public(&self) -> bool {
        let hostname = self.hostname();
        match self.kind {
            ForgeKind::GitHub => hostname == "github.com" || hostname == "www.github.com",
            ForgeKind::GitLab => hostname == "gitlab.com" || hostname == "www.gitlab.com",
        }
    }

    /// Halaman web untuk membuat Personal Access Token, dengan nama dan scope yang dibutuhkan
    /// OnCode sudah terisi. Jalan login lewat browser yang tidak memerlukan aplikasi OAuth.
    pub fn token_creation_url(&self) -> String {
        let base = self.web_base();
        match self.kind {
            ForgeKind::GitHub => format!("{base}/settings/tokens/new?description=OnCode&scopes=repo,read:user"),
            ForgeKind::GitLab => format!("{base}/-/user_settings/personal_access_tokens?name=OnCode&scopes=api,read_user"),
        }
    }
}

/// Repositori di sebuah forge, hasil parse URL remote.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    /// Nama host saja, huruf kecil, misalnya `"github.com"`.
    pub host: String,
    /// Pemilik atau namespace. Bisa memuat `/` untuk subgrup GitLab.
    pub owner: String,
    /// Nama repositori tanpa akhiran `.git`.
    pub name: String,
}

impl RepoRef {
    /// `"owner/name"`, bentuk yang dipakai kedua penyedia sebagai nama lengkap.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Akun yang sedang login di sebuah forge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// `"{kind}:{hostname}:{username}"`, misalnya `"github:github.com:octocat"`. Dipakai
    /// juga sebagai kunci token di keychain.
    pub id: String,
    pub kind: ForgeKind,
    /// Origin web instance, sama dengan [`ForgeHost::host`].
    pub host: String,
    pub username: String,
    /// Nama tampilan; sama dengan `username` bila pengguna tidak mengisinya.
    pub display_name: String,
    pub avatar_url: Option<String>,
}

impl Account {
    /// Menyusun id akun dari bagian-bagiannya.
    pub fn make_id(host: &ForgeHost, username: &str) -> String {
        format!("{}:{}:{}", host.kind.as_str(), host.hostname(), username)
    }

    /// Host forge tempat akun ini berada.
    pub fn forge_host(&self) -> ForgeHost {
        ForgeHost { kind: self.kind, host: self.host.clone() }
    }
}

/// Repositori di akun pengguna, untuk dialog clone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepo {
    /// `"owner/name"`; di GitLab memuat seluruh jalur subgrup.
    pub full_name: String,
    pub description: String,
    /// Di GitLab, visibilitas `internal` juga dihitung privat.
    pub private: bool,
    pub fork: bool,
    pub clone_url_https: String,
    pub clone_url_ssh: String,
    /// Kosong bila repositori belum punya commit.
    pub default_branch: String,
    pub updated_at: Timestamp,
    pub web_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// Pull request (GitHub) atau merge request (GitLab).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    /// Nomor PR di GitHub, `iid` MR di GitLab.
    pub number: u32,
    pub title: String,
    pub state: PrState,
    pub draft: bool,
    pub author: String,
    pub author_avatar: Option<String>,
    pub source_branch: String,
    pub target_branch: String,
    pub head_sha: String,
    pub updated_at: Timestamp,
    pub web_url: String,
    /// `true` bila branch sumber berada di repositori lain (fork).
    pub is_cross_repo: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetail {
    pub pull_request: PullRequest,
    /// Deskripsi dalam Markdown mentah; UI yang merender dan menyanitasinya.
    pub body: String,
    pub reviewers: Vec<String>,
    pub labels: Vec<String>,
    /// `None` selama penyedia masih menghitung apakah bisa di-merge.
    pub mergeable: Option<bool>,
    pub commits: u32,
    pub changed_files: u32,
    /// GitLab tidak menyediakan jumlah baris lewat REST, sehingga nilainya selalu 0 di sana.
    pub additions: u32,
    /// Lihat catatan pada `additions`.
    pub deletions: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckState {
    Pending,
    Running,
    Success,
    Failure,
    Cancelled,
    Skipped,
    Neutral,
}

/// Satu pemeriksaan CI pada sebuah commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub url: Option<String>,
}

/// Filter daftar PR/MR di sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrFilter {
    /// Semua yang masih terbuka.
    Open,
    /// Yang masih terbuka dan dibuat oleh akun ini.
    Mine,
    /// Yang masih terbuka dan menunggu review dari akun ini.
    ReviewRequested,
    /// Terbuka, tertutup, maupun sudah di-merge.
    All,
}

/// Masukan untuk membuat PR/MR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPullRequest {
    pub title: String,
    pub body: String,
    /// Di GitHub, PR dari fork ditulis `"pemilik:branch"`.
    pub source_branch: String,
    pub target_branch: String,
    pub draft: bool,
}

/// Satu halaman hasil. `next_page` diisi bila masih ada halaman berikutnya; nilainya
/// dikirim kembali apa adanya sebagai argumen `page`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page: Option<u32>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, next_page: Option<u32>) -> Self {
        Self { items, next_page }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_per_host() {
        assert_eq!(ForgeHost::github().api_base(), "https://api.github.com");
        assert_eq!(ForgeHost::new(ForgeKind::GitHub, "https://GitHub.com/").api_base(), "https://api.github.com");
        assert_eq!(
            ForgeHost::new(ForgeKind::GitHub, "https://ghe.perusahaan.id").api_base(),
            "https://ghe.perusahaan.id/api/v3"
        );
        assert_eq!(
            ForgeHost::new(ForgeKind::GitHub, "https://octocorp.ghe.com").api_base(),
            "https://api.octocorp.ghe.com"
        );
        assert_eq!(ForgeHost::gitlab().api_base(), "https://gitlab.com/api/v4");
        assert_eq!(
            ForgeHost::new(ForgeKind::GitLab, "https://git.perusahaan.id/").api_base(),
            "https://git.perusahaan.id/api/v4"
        );
        // Instance GitLab di bawah sub-path tetap utuh.
        assert_eq!(
            ForgeHost::new(ForgeKind::GitLab, "https://perusahaan.id/gitlab").api_base(),
            "https://perusahaan.id/gitlab/api/v4"
        );
    }

    #[test]
    fn host_normalization() {
        let host = ForgeHost::new(ForgeKind::GitLab, "  git.perusahaan.id/ ");
        assert_eq!(host.host, "https://git.perusahaan.id");
        assert_eq!(host.hostname(), "git.perusahaan.id");
        assert_eq!(ForgeHost::new(ForgeKind::GitLab, "http://Git.Lokal:8080/").hostname(), "git.lokal");
        assert_eq!(ForgeHost::github().hostname(), "github.com");
        assert_eq!(ForgeHost::new(ForgeKind::GitLab, "https://[::1]:8443").hostname(), "[::1]");
    }

    #[test]
    fn token_creation_url_prefills_name_and_scopes() {
        assert_eq!(ForgeHost::github().token_creation_url(), "https://github.com/settings/tokens/new?description=OnCode&scopes=repo,read:user");
        assert_eq!(
            ForgeHost::new(ForgeKind::GitLab, "git.contoh.id/").token_creation_url(),
            "https://git.contoh.id/-/user_settings/personal_access_tokens?name=OnCode&scopes=api,read_user"
        );
        // Instance di bawah sub-path tetap memakai path dasarnya.
        assert_eq!(
            ForgeHost::new(ForgeKind::GitLab, "https://contoh.id/gitlab").token_creation_url(),
            "https://contoh.id/gitlab/-/user_settings/personal_access_tokens?name=OnCode&scopes=api,read_user"
        );
    }

    #[test]
    fn only_github_com_and_gitlab_com_are_public() {
        assert!(ForgeHost::github().is_public());
        assert!(ForgeHost::gitlab().is_public());
        assert!(!ForgeHost::new(ForgeKind::GitLab, "git.contoh.id").is_public());
        assert!(!ForgeHost::new(ForgeKind::GitHub, "ghe.contoh.id").is_public());
        // Jenis yang tidak cocok dengan host-nya bukan layanan publik.
        assert!(!ForgeHost::new(ForgeKind::GitLab, "github.com").is_public());
    }

    #[test]
    fn host_matches_repo() {
        let repo = RepoRef { host: "github.com".into(), owner: "octo".into(), name: "hello".into() };
        assert!(ForgeHost::github().matches(&repo));
        assert!(!ForgeHost::gitlab().matches(&repo));
        assert_eq!(repo.full_name(), "octo/hello");
    }

    #[test]
    fn account_id_format() {
        assert_eq!(Account::make_id(&ForgeHost::github(), "octocat"), "github:github.com:octocat");
        assert_eq!(
            Account::make_id(&ForgeHost::new(ForgeKind::GitLab, "https://Git.Perusahaan.id:8443"), "budi"),
            "gitlab:git.perusahaan.id:budi"
        );
    }

    #[test]
    fn serde_names() {
        assert_eq!(serde_json::to_value(ForgeKind::GitHub).expect("json"), serde_json::json!("github"));
        assert_eq!(serde_json::to_value(ForgeKind::GitLab).expect("json"), serde_json::json!("gitlab"));
        assert_eq!(
            serde_json::from_value::<PrFilter>(serde_json::json!("reviewRequested")).expect("json"),
            PrFilter::ReviewRequested
        );
        let input: NewPullRequest = serde_json::from_value(serde_json::json!({
            "title": "Judul", "body": "", "sourceBranch": "fitur", "targetBranch": "main", "draft": true
        }))
        .expect("json");
        assert_eq!(input.source_branch, "fitur");
        let page = Page::new(vec![1u32, 2], Some(2));
        assert_eq!(serde_json::to_value(&page).expect("json"), serde_json::json!({ "items": [1, 2], "nextPage": 2 }));
    }
}
