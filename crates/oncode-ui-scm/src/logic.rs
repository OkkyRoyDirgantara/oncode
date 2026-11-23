//! Logika murni fitur Source Control: teks, validasi, dan penurunan tampilan dari snapshot.
//! Tidak ada yang menyentuh GPUI di sini, sehingga semuanya bisa diuji dengan `#[test]`.

use std::collections::HashSet;

use oncode_git::{ChangeKind, ConflictKind, Head, Operation, Refs, RepoSnapshot};

/// "1 file" / "2 files".
pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Nama pendek untuk HEAD saat ini.
pub fn head_label(snapshot: &RepoSnapshot) -> String {
    match &snapshot.head {
        Head::Branch { name } | Head::Unborn { name } => name.clone(),
        Head::Detached { oid } => oid.chars().take(8).collect(),
    }
}

/// Jumlah berkas yang berubah, untuk lencana ikon Source Control. Berkas yang muncul di
/// beberapa kelompok (mis. staged dan unstaged) dihitung sekali.
pub fn change_count(snapshot: &RepoSnapshot) -> usize {
    let mut paths: HashSet<&str> = HashSet::new();
    paths.extend(snapshot.staged.iter().map(|c| c.path.as_str()));
    paths.extend(snapshot.unstaged.iter().map(|c| c.path.as_str()));
    paths.extend(snapshot.untracked.iter().map(String::as_str));
    paths.extend(snapshot.conflicted.iter().map(|c| c.path.as_str()));
    paths.len()
}

/// Memisahkan path menjadi (nama berkas, folder).
pub fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[i + 1..], &path[..i]),
        None => (path, ""),
    }
}

pub fn kind_letter(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::TypeChanged => "T",
    }
}

pub fn kind_title(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "Added",
        ChangeKind::Modified => "Modified",
        ChangeKind::Deleted => "Deleted",
        ChangeKind::Renamed => "Renamed",
        ChangeKind::Copied => "Copied",
        ChangeKind::TypeChanged => "Type changed",
    }
}

pub fn conflict_title(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::BothModified => "Both modified",
        ConflictKind::BothAdded => "Both added",
        ConflictKind::BothDeleted => "Both deleted",
        ConflictKind::AddedByUs => "Added by us",
        ConflictKind::AddedByThem => "Added by them",
        ConflictKind::DeletedByUs => "Deleted by us",
        ConflictKind::DeletedByThem => "Deleted by them",
    }
}

/// Satu baris pada kelompok "Changes": perubahan working tree atau berkas untracked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRow {
    pub path: String,
    pub orig_path: Option<String>,
    pub kind: ChangeKind,
    pub untracked: bool,
}

/// Berkas untracked ditampilkan bersama perubahan lain, diurutkan menurut path.
pub fn change_rows(snapshot: &RepoSnapshot) -> Vec<ChangeRow> {
    let mut rows: Vec<ChangeRow> =
        snapshot.unstaged.iter().map(|c| ChangeRow { path: c.path.clone(), orig_path: c.orig_path.clone(), kind: c.kind, untracked: false }).collect();
    rows.extend(snapshot.untracked.iter().map(|path| ChangeRow { path: path.clone(), orig_path: None, kind: ChangeKind::Added, untracked: true }));
    rows.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
    rows
}

/// Membagi daftar path menjadi berkas terlacak dan untracked menurut snapshot.
pub fn split_tracked(snapshot: &RepoSnapshot, paths: &[String]) -> (Vec<String>, Vec<String>) {
    let untracked: HashSet<&str> = snapshot.untracked.iter().map(String::as_str).collect();
    paths.iter().cloned().partition(|p| !untracked.contains(p.as_str()))
}

/// Judul dan isi konfirmasi discard, yang menyebut apa yang akan hilang. `None` bila tidak ada berkas.
pub fn discard_prompt(tracked: &[String], untracked: &[String]) -> Option<(String, String)> {
    let total = tracked.len() + untracked.len();
    if total == 0 {
        return None;
    }
    let title = match (total, tracked.first().or(untracked.first())) {
        (1, Some(single)) => format!("Discard changes in {single}?"),
        _ => format!("Discard changes in {}?", plural(total, "file")),
    };
    let mut parts = Vec::new();
    if !tracked.is_empty() {
        parts.push(format!("Changes to {} will be lost permanently.", plural(tracked.len(), "tracked file")));
    }
    if !untracked.is_empty() {
        parts.push(format!("{} will be moved to the trash.", plural(untracked.len(), "untracked file")));
    }
    Some((title, parts.join("\n")))
}

/// Teks tombol commit.
pub fn commit_label(amend: bool, staged: usize) -> String {
    if amend {
        "Amend Commit".into()
    } else if staged > 0 {
        format!("Commit {}", plural(staged, "file"))
    } else {
        "Commit".into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationText {
    pub title: String,
    pub detail: String,
    pub can_skip: bool,
}

/// Teks banner operasi yang sedang berhenti (merge, rebase, cherry-pick, revert).
pub fn operation_text(operation: &Operation) -> OperationText {
    match operation {
        Operation::Merge { incoming } => OperationText { title: "Merge in progress".into(), detail: format!("Merging {incoming}"), can_skip: false },
        Operation::Rebase { interactive, step, total, onto, head_name, .. } => OperationText {
            title: if *interactive { "Interactive rebase in progress".into() } else { "Rebase in progress".into() },
            detail: format!("Step {step} of {total}{} onto {onto}", if head_name.is_empty() { String::new() } else { format!(" · {head_name}") }),
            can_skip: true,
        },
        Operation::CherryPick { commit } => OperationText { title: "Cherry-pick in progress".into(), detail: format!("Applying {commit}"), can_skip: true },
        Operation::Revert { commit } => OperationText { title: "Revert in progress".into(), detail: format!("Reverting {commit}"), can_skip: true },
    }
}

/// Nama operasi untuk kalimat ("merge", "rebase", "cherry-pick", "revert").
pub fn operation_name(operation: Option<&Operation>) -> &'static str {
    match operation {
        Some(Operation::Merge { .. }) => "merge",
        Some(Operation::Rebase { .. }) => "rebase",
        Some(Operation::CherryPick { .. }) => "cherry-pick",
        Some(Operation::Revert { .. }) => "revert",
        None => "operation",
    }
}

/// Baris status konflik pada banner operasi.
pub fn conflicts_left(conflicts: usize) -> String {
    if conflicts > 0 {
        format!("{} left to resolve.", plural(conflicts, "conflicted file"))
    } else {
        "No conflicts left. Continue when you are ready.".into()
    }
}

/// Teks item branch di status bar: nama, penanda detached, dan operasi yang berlangsung.
pub fn branch_status_label(snapshot: &RepoSnapshot) -> String {
    let mut label = head_label(snapshot);
    if matches!(snapshot.head, Head::Detached { .. }) {
        label.push_str(" (detached)");
    }
    if snapshot.operation.is_some() {
        label.push_str(&format!(" ({})", operation_name(snapshot.operation.as_ref())));
    }
    label
}

/// Keadaan tombol sinkronisasi (panel dan status bar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncState {
    /// Tidak ditampilkan: HEAD detached/unborn, atau belum ada remote.
    Hidden,
    /// Branch belum punya upstream; tawarkan publish.
    Publish,
    Sync { upstream: String, ahead: u32, behind: u32 },
}

pub fn sync_state(snapshot: &RepoSnapshot, has_remote: bool) -> SyncState {
    if !matches!(snapshot.head, Head::Branch { .. }) {
        return SyncState::Hidden;
    }
    match &snapshot.upstream {
        Some(upstream) if upstream.exists => SyncState::Sync { upstream: upstream.name.clone(), ahead: upstream.ahead, behind: upstream.behind },
        _ if has_remote => SyncState::Publish,
        _ => SyncState::Hidden,
    }
}

/// Aturan dasar nama ref Git, agar galat muncul sebelum perintah dijalankan.
pub fn validate_ref_name(value: &str) -> Option<&'static str> {
    let name = value.trim();
    if name.is_empty() {
        return Some("Enter a name");
    }
    if name.starts_with('-') {
        return Some("Names cannot start with a dash");
    }
    let bad_char = name.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'));
    let bad_shape =
        name.contains("..") || name.contains("@{") || name.contains("//") || name.starts_with('/') || name.ends_with('/') || name.ends_with('.') || name.ends_with(".lock");
    if bad_char || bad_shape {
        return Some("This is not a valid Git ref name");
    }
    None
}

/// Sasaran yang dipilih di pemilih branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutTarget {
    Create,
    Branch(String),
    Remote(String),
    Detached(String),
}

impl CheckoutTarget {
    /// Bentuk string untuk `PickItem::value`.
    pub fn encode(&self) -> String {
        match self {
            Self::Create => "create:".into(),
            Self::Branch(name) => format!("branch:{name}"),
            Self::Remote(name) => format!("remote:{name}"),
            Self::Detached(name) => format!("detached:{name}"),
        }
    }

    pub fn decode(value: &str) -> Option<Self> {
        let (kind, name) = value.split_once(':')?;
        match kind {
            "create" => Some(Self::Create),
            "branch" => Some(Self::Branch(name.into())),
            "remote" => Some(Self::Remote(name.into())),
            "detached" => Some(Self::Detached(name.into())),
            _ => None,
        }
    }
}

/// Isi pemilih branch: (label, keterangan, sasaran).
pub fn branch_pick_items(refs: &Refs) -> Vec<(String, String, CheckoutTarget)> {
    let mut items = vec![("+ Create new branch…".to_string(), String::new(), CheckoutTarget::Create)];
    for branch in &refs.branches {
        let description = if branch.is_head {
            "current".to_string()
        } else {
            branch.upstream.as_ref().map(|u| format!("→ {u}")).unwrap_or_default()
        };
        items.push((branch.name.clone(), description, CheckoutTarget::Branch(branch.name.clone())));
    }
    for branch in &refs.remote_branches {
        items.push((branch.name.clone(), "remote".into(), CheckoutTarget::Remote(branch.name.clone())));
    }
    for tag in &refs.tags {
        items.push((tag.name.clone(), "tag".into(), CheckoutTarget::Detached(tag.name.clone())));
    }
    items
}

/// Ref yang bisa di-merge atau dijadikan dasar rebase: semua kecuali branch aktif. (label, keterangan)
pub fn ref_pick_items(refs: &Refs) -> Vec<(String, &'static str)> {
    let mut items: Vec<(String, &'static str)> = refs.branches.iter().filter(|b| !b.is_head).map(|b| (b.name.clone(), "branch")).collect();
    items.extend(refs.remote_branches.iter().map(|b| (b.name.clone(), "remote branch")));
    items.extend(refs.tags.iter().map(|t| (t.name.clone(), "tag")));
    items
}

/// Nama branch lokal untuk sebuah branch remote (`origin/fitur/x` → `fitur/x`).
pub fn local_name_of_remote(name: &str) -> &str {
    name.split_once('/').map_or(name, |(_, rest)| rest)
}

/// Pull `--ff-only` gagal karena riwayat bercabang.
pub fn is_diverged(message: &str, detail: Option<&str>) -> bool {
    let text = format!("{message} {}", detail.unwrap_or("")).to_lowercase();
    text.contains("fast-forward") || text.contains("diverg")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// Tindak lanjut yang ditawarkan bersama pesan galat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    PullThenPush,
    ForcePush,
    PublishBranch,
    StashAndContinue,
    ShowOutput,
    Accounts,
}

impl FollowUp {
    pub fn label(self) -> &'static str {
        match self {
            Self::PullThenPush => "Pull, then push",
            Self::ForcePush => "Force push…",
            Self::PublishBranch => "Publish branch",
            Self::StashAndContinue => "Stash and continue",
            Self::ShowOutput => "Show output",
            Self::Accounts => "Accounts",
        }
    }

    /// Bagian id elemen tombolnya (`scm-toast-action-<slug>`).
    pub fn slug(self) -> &'static str {
        match self {
            Self::PullThenPush => "pull-then-push",
            Self::ForcePush => "force-push",
            Self::PublishBranch => "publish",
            Self::StashAndContinue => "stash-and-continue",
            Self::ShowOutput => "show-output",
            Self::Accounts => "accounts",
        }
    }
}

/// Pesan dan tindak lanjut untuk sebuah galat Git, dipilih menurut `AppError.code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorNotice {
    pub level: NoticeLevel,
    pub message: String,
    pub follow_ups: Vec<FollowUp>,
    /// Tampilkan Source Control (konflik perlu diselesaikan di sana).
    pub reveal_scm: bool,
}

/// Menerjemahkan kode galat Git menjadi pesan dengan aksi berikutnya. `None` = tidak perlu
/// ditampilkan (operasi dibatalkan pengguna). `can_retry`: operasinya bisa diulang setelah stash.
pub fn error_notice(code: &str, message: &str, label: &str, can_retry: bool) -> Option<ErrorNotice> {
    let notice = |level, message: String, follow_ups: Vec<FollowUp>| Some(ErrorNotice { level, message, follow_ups, reveal_scm: false });
    match code {
        "cancelled" => None,
        "pushRejected" => notice(
            NoticeLevel::Error,
            format!("{label} was rejected because the remote has commits you don't have locally."),
            vec![FollowUp::PullThenPush, FollowUp::ForcePush],
        ),
        "noUpstream" => notice(NoticeLevel::Error, "This branch is not published yet.".into(), vec![FollowUp::PublishBranch]),
        "dirtyWorkTree" => notice(
            NoticeLevel::Error,
            format!("{label} would overwrite your local changes."),
            if can_retry { vec![FollowUp::StashAndContinue] } else { Vec::new() },
        ),
        "conflict" => Some(ErrorNotice {
            level: NoticeLevel::Warning,
            message: format!("{label} stopped because of conflicts. Resolve them in Source Control, then continue."),
            follow_ups: Vec::new(),
            reveal_scm: true,
        }),
        "indexLocked" => notice(
            NoticeLevel::Error,
            "Another Git process seems to be running in this repository (index.lock exists). Try again in a moment.".into(),
            Vec::new(),
        ),
        "hookFailed" => notice(NoticeLevel::Error, format!("{label} was stopped by a Git hook."), vec![FollowUp::ShowOutput]),
        "authFailed" => notice(NoticeLevel::Error, format!("{label} failed: authentication was rejected."), vec![FollowUp::Accounts]),
        "nothingToCommit" => notice(NoticeLevel::Info, "There is nothing to commit.".into(), Vec::new()),
        "stale" => notice(NoticeLevel::Warning, message.to_string(), Vec::new()),
        _ => notice(NoticeLevel::Error, format!("{label} failed: {message}"), Vec::new()),
    }
}

/// "just now", "5m ago", "3h ago", "2d ago", atau tanggal (UTC) untuk yang lebih lama.
pub fn relative_time(seconds: f64, now: f64) -> String {
    let diff = now - seconds;
    if diff < 60.0 {
        "just now".into()
    } else if diff < 3600.0 {
        format!("{}m ago", (diff / 60.0).floor() as u64)
    } else if diff < 86400.0 {
        format!("{}h ago", (diff / 3600.0).floor() as u64)
    } else if diff < 86400.0 * 30.0 {
        format!("{}d ago", (diff / 86400.0).floor() as u64)
    } else {
        let (year, month, day) = civil_from_days((seconds / 86400.0).floor() as i64);
        format!("{year:04}-{month:02}-{day:02}")
    }
}

/// Tanggal kalender (proleptik Gregorian) dari jumlah hari sejak 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Jam:menit:detik (UTC) untuk cap waktu di Git Output.
pub fn clock_time(seconds_since_epoch: u64) -> String {
    let day = seconds_since_epoch % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

// ----- Clone -----

/// Pesan galat untuk nama folder tujuan, atau `None` bila sah.
pub fn validate_folder_name(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("Enter a folder name");
    }
    if name != name.trim() {
        return Some("The name cannot start or end with a space");
    }
    if name.contains(['/', '\\']) {
        return Some("The name cannot contain a path separator");
    }
    if name == "." || name == ".." {
        return Some("This name is reserved");
    }
    if name.chars().any(|c| c.is_control()) {
        return Some("The name cannot contain control characters");
    }
    if name.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return Some("The name cannot contain any of < > : \" | ? *");
    }
    if name.starts_with('-') {
        return Some("The name cannot start with a dash");
    }
    if name.ends_with('.') {
        return Some("The name cannot end with a dot");
    }
    if name.chars().count() > 255 {
        return Some("The name is too long");
    }
    None
}

/// Panjang skema (`https` pada `https://…`) bila teks diawali `skema://`.
fn scheme_of(value: &str) -> Option<&str> {
    let end = value.find("://")?;
    let scheme = &value[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))).then_some(scheme)
}

/// Nama folder bawaan untuk sebuah URL clone, seperti yang dipilih `git clone`: segmen terakhir
/// tanpa garis miring penutup dan tanpa akhiran `.git`. Mendukung bentuk https/ssh/git/file
/// maupun bentuk scp (`git@host:owner/repo.git`). Kosong bila tidak bisa ditebak.
pub fn repo_name_from_url(url: &str) -> String {
    let mut value = url.trim();
    // Query dan fragmen bukan bagian dari nama.
    if let Some(cut) = value.find(['?', '#']) {
        value = &value[..cut];
    }
    value = value.trim_end_matches(['/', '\\']);
    if value.len() >= 4 && value.is_char_boundary(value.len() - 4) && value[value.len() - 4..].eq_ignore_ascii_case(".git") {
        value = &value[..value.len() - 4];
    }
    value = value.trim_end_matches(['/', '\\']);
    let name = value.rfind(['/', '\\', ':']).map_or(value, |cut| &value[cut + 1..]);
    // Sisa yang hanya berupa host (mis. "https://example.com") bukan nama repositori.
    if let Some(scheme) = scheme_of(value) {
        if !value[scheme.len() + 3..].contains('/') {
            return String::new();
        }
    }
    if validate_folder_name(name).is_none() {
        name.to_string()
    } else {
        String::new()
    }
}

/// Pesan galat untuk URL clone, atau `None` bila bentuknya masuk akal. Git sendiri yang memeriksa sisanya.
pub fn validate_clone_url(url: &str) -> Option<&'static str> {
    let value = url.trim();
    if value.is_empty() {
        return Some("Enter a repository URL");
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("The URL cannot contain spaces");
    }
    if value.starts_with('-') {
        return Some("The URL cannot start with a dash");
    }
    // Bentuk `transport::alamat` (mis. `ext::`) menjalankan program pembantu; tidak pernah diterima dari sini.
    if let Some(end) = value.find("::") {
        let transport = &value[..end];
        if !transport.is_empty() && transport.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) {
            return Some("This kind of URL is not supported");
        }
    }
    if let Some(scheme) = scheme_of(value) {
        return if ["https", "http", "ssh", "git", "file"].contains(&scheme.to_ascii_lowercase().as_str()) {
            None
        } else {
            Some("Use an https://, ssh:// or git:// URL")
        };
    }
    // Bentuk scp: [user@]host:path
    if let Some((host, path)) = value.split_once(':') {
        let plain = |s: &str| !s.is_empty() && !s.contains(['/', '\\', ':']);
        let with_user = host.split_once('@').is_some_and(|(user, host)| plain(user) && !user.contains('@') && plain(host));
        let dotted = plain(host) && !host.contains('@') && host.split_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty());
        if !path.is_empty() && (with_user || dotted) {
            return None;
        }
    }
    // Path lokal absolut.
    let bytes = value.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\');
    if value.starts_with('/') || value.starts_with("~/") || drive {
        return None;
    }
    Some("Enter a full URL, for example https://github.com/owner/repo.git")
}

/// Menggabungkan folder induk dan nama untuk ditampilkan.
pub fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        return name.to_string();
    }
    let separator = if parent.contains('\\') && !parent.contains('/') { '\\' } else { '/' };
    format!("{}{separator}{name}", parent.trim_end_matches(['/', '\\']))
}

/// Path absolut sebuah berkas repositori, dengan pemisah milik OS.
pub fn repo_file(root: &str, path: &str) -> String {
    let separator = if root.contains('\\') && !root.contains('/') { "\\" } else { "/" };
    format!("{}{separator}{}", root.trim_end_matches(['/', '\\']), path.replace('/', separator))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oncode_git::{Branch, ConflictEntry, FileChange, RemoteBranch, Tag, Upstream};

    fn change(path: &str, kind: ChangeKind) -> FileChange {
        FileChange { path: path.into(), orig_path: None, kind, submodule: None }
    }

    fn snapshot() -> RepoSnapshot {
        RepoSnapshot {
            root: "/repo".into(),
            head: Head::Branch { name: "main".into() },
            head_oid: Some("0123456789abcdef".into()),
            upstream: None,
            operation: None,
            staged: Vec::new(),
            unstaged: Vec::new(),
            untracked: Vec::new(),
            conflicted: Vec::new(),
            stash_count: 0,
        }
    }

    fn branch(name: &str, is_head: bool, upstream: Option<&str>) -> Branch {
        Branch {
            name: name.into(),
            oid: "a".repeat(40),
            is_head,
            upstream: upstream.map(String::from),
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            worktree: None,
            subject: String::new(),
            time: 0.0,
        }
    }

    #[test]
    fn head_label_shortens_detached_oids() {
        let mut s = snapshot();
        assert_eq!(head_label(&s), "main");
        s.head = Head::Unborn { name: "trunk".into() };
        assert_eq!(head_label(&s), "trunk");
        s.head = Head::Detached { oid: "0123456789abcdef".into() };
        assert_eq!(head_label(&s), "01234567");
        assert_eq!(branch_status_label(&s), "01234567 (detached)");
        s.operation = Some(Operation::CherryPick { commit: "abc".into() });
        assert_eq!(branch_status_label(&s), "01234567 (detached) (cherry-pick)");
    }

    #[test]
    fn change_count_counts_each_path_once() {
        let mut s = snapshot();
        s.staged = vec![change("a.txt", ChangeKind::Modified), change("b.txt", ChangeKind::Added)];
        s.unstaged = vec![change("a.txt", ChangeKind::Modified)];
        s.untracked = vec!["c.txt".into()];
        s.conflicted = vec![ConflictEntry { path: "d.txt".into(), kind: ConflictKind::BothModified }];
        assert_eq!(change_count(&s), 4);
        assert_eq!(change_count(&snapshot()), 0);
    }

    #[test]
    fn change_rows_merge_untracked_and_sort_by_path() {
        let mut s = snapshot();
        s.unstaged = vec![change("src/z.rs", ChangeKind::Modified), change("README.md", ChangeKind::Deleted)];
        s.untracked = vec!["catatan.txt".into(), "Src/a.rs".into()];
        let rows = change_rows(&s);
        assert_eq!(rows.iter().map(|r| (r.path.as_str(), r.untracked)).collect::<Vec<_>>(), [
            ("catatan.txt", true),
            ("README.md", false),
            ("Src/a.rs", true),
            ("src/z.rs", false),
        ]);
        assert_eq!(rows[1].kind, ChangeKind::Deleted);
    }

    #[test]
    fn split_helpers() {
        assert_eq!(split_path("src/app/main.rs"), ("main.rs", "src/app"));
        assert_eq!(split_path("main.rs"), ("main.rs", ""));
        let mut s = snapshot();
        s.untracked = vec!["baru.txt".into()];
        let (tracked, untracked) = split_tracked(&s, &["a.txt".into(), "baru.txt".into()]);
        assert_eq!((tracked, untracked), (vec!["a.txt".to_string()], vec!["baru.txt".to_string()]));
    }

    #[test]
    fn discard_prompt_says_what_is_lost() {
        assert_eq!(discard_prompt(&[], &[]), None);
        let (title, message) = discard_prompt(&["a.txt".into()], &[]).unwrap();
        assert_eq!(title, "Discard changes in a.txt?");
        assert_eq!(message, "Changes to 1 tracked file will be lost permanently.");
        let (title, message) = discard_prompt(&[], &["sementara.txt".into()]).unwrap();
        assert_eq!(title, "Discard changes in sementara.txt?");
        assert_eq!(message, "1 untracked file will be moved to the trash.");
        let (title, message) = discard_prompt(&["a".into(), "b".into()], &["c".into()]).unwrap();
        assert_eq!(title, "Discard changes in 3 files?");
        assert_eq!(message, "Changes to 2 tracked files will be lost permanently.\n1 untracked file will be moved to the trash.");
    }

    #[test]
    fn commit_label_follows_amend_and_staged_count() {
        assert_eq!(commit_label(false, 0), "Commit");
        assert_eq!(commit_label(false, 1), "Commit 1 file");
        assert_eq!(commit_label(false, 3), "Commit 3 files");
        assert_eq!(commit_label(true, 3), "Amend Commit");
    }

    #[test]
    fn operation_texts() {
        let merge = operation_text(&Operation::Merge { incoming: "fitur".into() });
        assert_eq!((merge.title.as_str(), merge.detail.as_str(), merge.can_skip), ("Merge in progress", "Merging fitur", false));
        let rebase = operation_text(&Operation::Rebase { interactive: true, step: 2, total: 5, onto: "abc1234".into(), head_name: "fitur".into(), stopped_at: None });
        assert_eq!(rebase.title, "Interactive rebase in progress");
        assert_eq!(rebase.detail, "Step 2 of 5 · fitur onto abc1234");
        assert!(rebase.can_skip);
        let plain = operation_text(&Operation::Rebase { interactive: false, step: 1, total: 1, onto: "main".into(), head_name: String::new(), stopped_at: None });
        assert_eq!((plain.title.as_str(), plain.detail.as_str()), ("Rebase in progress", "Step 1 of 1 onto main"));
        assert_eq!(operation_text(&Operation::Revert { commit: "abc".into() }).detail, "Reverting abc");
        assert_eq!(operation_name(Some(&Operation::CherryPick { commit: "x".into() })), "cherry-pick");
        assert_eq!(operation_name(None), "operation");
        assert_eq!(conflicts_left(0), "No conflicts left. Continue when you are ready.");
        assert_eq!(conflicts_left(1), "1 conflicted file left to resolve.");
        assert_eq!(conflicts_left(2), "2 conflicted files left to resolve.");
    }

    #[test]
    fn sync_state_covers_publish_and_ahead_behind() {
        let mut s = snapshot();
        assert_eq!(sync_state(&s, false), SyncState::Hidden);
        assert_eq!(sync_state(&s, true), SyncState::Publish);
        s.upstream = Some(Upstream { name: "origin/main".into(), ahead: 1, behind: 2, exists: true });
        assert_eq!(sync_state(&s, true), SyncState::Sync { upstream: "origin/main".into(), ahead: 1, behind: 2 });
        // Upstream dikonfigurasi tetapi ref-nya hilang: terbitkan lagi.
        s.upstream = Some(Upstream { name: "origin/main".into(), ahead: 0, behind: 0, exists: false });
        assert_eq!(sync_state(&s, true), SyncState::Publish);
        s.head = Head::Detached { oid: "abc".into() };
        assert_eq!(sync_state(&s, true), SyncState::Hidden);
    }

    #[test]
    fn ref_names_are_validated_before_git_runs() {
        for ok in ["fitur/login", "perbaikan/typo", "v1.0.0", "a", "rilis-2", "user@host"] {
            assert_eq!(validate_ref_name(ok), None, "{ok:?}");
        }
        assert_eq!(validate_ref_name("  "), Some("Enter a name"));
        assert_eq!(validate_ref_name("-x"), Some("Names cannot start with a dash"));
        for bad in ["nama tidak sah", "a..b", "a~1", "a^", "a:b", "a?", "a*", "a[1]", "a\\b", "a@{1}", "a//b", "/a", "a/", "a.", "a.lock", "a\tb", "a\u{7f}b"] {
            assert_eq!(validate_ref_name(bad), Some("This is not a valid Git ref name"), "{bad:?}");
        }
    }

    #[test]
    fn branch_picker_lists_create_branches_remotes_and_tags() {
        let refs = Refs {
            branches: vec![branch("main", true, Some("origin/main")), branch("fitur/login", false, Some("origin/fitur/login")), branch("lokal", false, None)],
            remote_branches: vec![RemoteBranch { name: "origin/main".into(), remote: "origin".into(), oid: "b".repeat(40) }],
            tags: vec![Tag { name: "v1".into(), oid: "c".repeat(40), annotated: false, message: String::new() }],
            remotes: Vec::new(),
        };
        let items = branch_pick_items(&refs);
        assert_eq!(items.iter().map(|(label, description, _)| (label.as_str(), description.as_str())).collect::<Vec<_>>(), [
            ("+ Create new branch…", ""),
            ("main", "current"),
            ("fitur/login", "→ origin/fitur/login"),
            ("lokal", ""),
            ("origin/main", "remote"),
            ("v1", "tag"),
        ]);
        for (_, _, target) in &items {
            assert_eq!(CheckoutTarget::decode(&target.encode()).as_ref(), Some(target));
        }
        assert_eq!(CheckoutTarget::decode("branch:fitur:aneh"), Some(CheckoutTarget::Branch("fitur:aneh".into())));
        assert_eq!(CheckoutTarget::decode("lain"), None);
        assert_eq!(ref_pick_items(&refs), [("fitur/login".to_string(), "branch"), ("lokal".to_string(), "branch"), ("origin/main".to_string(), "remote branch"), ("v1".to_string(), "tag")]);
        assert_eq!(local_name_of_remote("origin/fitur/login"), "fitur/login");
        assert_eq!(local_name_of_remote("origin"), "origin");
    }

    #[test]
    fn diverged_pull_is_recognised_from_git_output() {
        assert!(is_diverged("Not possible to fast-forward, aborting.", None));
        assert!(is_diverged("git failed", Some("fatal: Need to specify how to reconcile divergent branches.")));
        assert!(is_diverged("hint: You have Diverging branches", None));
        assert!(!is_diverged("Could not resolve host", Some("fatal: unable to access")));
    }

    #[test]
    fn errors_map_to_messages_and_follow_ups() {
        assert_eq!(error_notice("cancelled", "x", "Push", false), None);
        let rejected = error_notice("pushRejected", "x", "Push", false).unwrap();
        assert_eq!(rejected.message, "Push was rejected because the remote has commits you don't have locally.");
        assert_eq!(rejected.follow_ups, [FollowUp::PullThenPush, FollowUp::ForcePush]);
        assert_eq!(error_notice("noUpstream", "x", "Pull", false).unwrap().follow_ups, [FollowUp::PublishBranch]);
        let dirty = error_notice("dirtyWorkTree", "x", "Checkout", true).unwrap();
        assert_eq!(dirty.message, "Checkout would overwrite your local changes.");
        assert_eq!(dirty.follow_ups, [FollowUp::StashAndContinue]);
        assert!(error_notice("dirtyWorkTree", "x", "Pull", false).unwrap().follow_ups.is_empty());
        let conflict = error_notice("conflict", "x", "Merge", true).unwrap();
        assert!(conflict.reveal_scm);
        assert_eq!(conflict.level, NoticeLevel::Warning);
        assert_eq!(error_notice("hookFailed", "x", "Commit", false).unwrap().follow_ups, [FollowUp::ShowOutput]);
        let auth = error_notice("authFailed", "x", "Publish", false).unwrap();
        assert_eq!(auth.message, "Publish failed: authentication was rejected.");
        assert_eq!(auth.follow_ups, [FollowUp::Accounts]);
        assert_eq!(error_notice("nothingToCommit", "x", "Commit", false).unwrap().level, NoticeLevel::Info);
        assert_eq!(error_notice("stale", "The file changed", "Stage", false).unwrap().message, "The file changed");
        assert_eq!(error_notice("unknown", "boom", "Fetch", false).unwrap().message, "Fetch failed: boom");
        assert_eq!(FollowUp::PullThenPush.label(), "Pull, then push");
    }

    #[test]
    fn relative_time_buckets() {
        let now = 1_700_000_000.0;
        assert_eq!(relative_time(now - 10.0, now), "just now");
        assert_eq!(relative_time(now - 125.0, now), "2m ago");
        assert_eq!(relative_time(now - 3.0 * 3600.0, now), "3h ago");
        assert_eq!(relative_time(now - 2.0 * 86400.0, now), "2d ago");
        assert_eq!(relative_time(0.0, now), "1970-01-01");
        assert_eq!(relative_time(951_782_400.0, now), "2000-02-29");
        assert_eq!(clock_time(3 * 3600 + 4 * 60 + 5), "03:04:05");
    }

    #[test]
    fn repo_name_from_url_matches_git_clone() {
        for (url, expected) in [
            ("https://github.com/owner/repo.git", "repo"),
            ("https://github.com/owner/repo", "repo"),
            ("https://github.com/owner/repo/", "repo"),
            ("https://github.com/owner/repo.git/", "repo"),
            ("https://github.com/owner/repo.GIT", "repo"),
            ("  https://github.com/owner/repo.git  ", "repo"),
            ("https://gitlab.com/group/sub/group/project.git", "project"),
            ("https://user@host.example:8443/scm/my.repo.git", "my.repo"),
            ("https://github.com/owner/repo.git?ref=main#readme", "repo"),
            // Bentuk scp.
            ("git@github.com:owner/repo.git", "repo"),
            ("git@github.com:owner/repo", "repo"),
            ("git@github.com:repo.git", "repo"),
            ("git@gitlab.com:group/sub/project.git/", "project"),
            ("ssh://git@github.com/owner/repo.git", "repo"),
            ("ssh://git@host.example:2222/owner/repo.git", "repo"),
            ("git://host.example/pub/thing.git", "thing"),
            ("file:///home/me/src/local.git", "local"),
            ("/home/me/src/local-repo/", "local-repo"),
            // Seperti git: menunjuk ke direktori `.git` berarti repositori induknya.
            ("/home/me/src/local-repo/.git", "local-repo"),
            ("C:\\repos\\win-repo", "win-repo"),
        ] {
            assert_eq!(repo_name_from_url(url), expected, "{url:?}");
        }
        for url in ["", "   ", "https://github.com", "https://github.com/", "ssh://git@host.example:2222", "https://host/.."] {
            assert_eq!(repo_name_from_url(url), "", "{url:?}");
        }
    }

    #[test]
    fn folder_names_are_validated() {
        for name in ["repo", "my.repo", "repo-2", "Proyek Baru", ".dotfiles", "a"] {
            assert_eq!(validate_folder_name(name), None, "{name:?}");
        }
        let long = "x".repeat(256);
        for (name, needle) in [
            ("", "Enter a folder name"),
            ("   ", "Enter a folder name"),
            ("a/b", "path separator"),
            ("a\\b", "path separator"),
            ("/abs", "path separator"),
            ("..", "reserved"),
            (".", "reserved"),
            (" repo", "space"),
            ("repo ", "space"),
            ("re\tpo", "control"),
            ("re\u{0}po", "control"),
            ("re:po", "cannot contain"),
            ("re\"po", "cannot contain"),
            ("re*po", "cannot contain"),
            ("-rf", "dash"),
            ("repo.", "dot"),
            (long.as_str(), "too long"),
        ] {
            assert!(validate_folder_name(name).is_some_and(|m| m.contains(needle)), "{name:?} → {:?}", validate_folder_name(name));
        }
    }

    #[test]
    fn clone_urls_are_validated() {
        for url in [
            "https://github.com/owner/repo.git",
            "http://git.internal/owner/repo",
            "ssh://git@github.com/owner/repo.git",
            "git://host.example/pub/thing.git",
            "file:///home/me/src/local.git",
            "git@github.com:owner/repo.git",
            "github.com:owner/repo.git",
            "/home/me/src/local-repo",
            "C:\\repos\\win-repo",
        ] {
            assert_eq!(validate_clone_url(url), None, "{url:?}");
        }
        for (url, needle) in [
            ("", "Enter a repository URL"),
            ("https://github.com/owner/my repo", "spaces"),
            ("--upload-pack=touch /tmp/pwned", "spaces"),
            ("--upload-pack=x", "dash"),
            ("-u", "dash"),
            // Transport pembantu menjalankan program; tidak boleh lolos.
            ("ext::sh -c touch% /tmp/pwned", "spaces"),
            ("ext::sh", "not supported"),
            ("fd::17", "not supported"),
            ("ftp://host/repo.git", "https"),
            ("javascript://x/y", "https"),
            ("owner/repo", "full URL"),
            ("repo", "full URL"),
        ] {
            assert!(validate_clone_url(url).is_some_and(|m| m.contains(needle)), "{url:?} → {:?}", validate_clone_url(url));
        }
    }

    #[test]
    fn paths_use_the_parent_separator() {
        assert_eq!(join_path("/home/me/src", "repo"), "/home/me/src/repo");
        assert_eq!(join_path("/home/me/src/", "repo"), "/home/me/src/repo");
        assert_eq!(join_path("C:\\Users\\me", "repo"), "C:\\Users\\me\\repo");
        assert_eq!(join_path("", "repo"), "repo");
        assert_eq!(repo_file("/repo/", "src/main.rs"), "/repo/src/main.rs");
        assert_eq!(repo_file("C:\\repo", "src/main.rs"), "C:\\repo\\src\\main.rs");
    }
}
