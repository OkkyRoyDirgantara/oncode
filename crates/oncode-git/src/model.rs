//! Struktur data yang dikirim ke UI. Semua field memakai camelCase di sisi TypeScript.

use serde::{Deserialize, Serialize};

/// Detik sejak epoch Unix. `f64` agar aman diekspor sebagai `number` di TypeScript.
pub type Timestamp = f64;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSnapshot {
    /// Path akar worktree.
    pub root: String,
    pub head: Head,
    pub head_oid: Option<String>,
    pub upstream: Option<Upstream>,
    pub operation: Option<Operation>,
    pub staged: Vec<FileChange>,
    pub unstaged: Vec<FileChange>,
    pub untracked: Vec<String>,
    pub conflicted: Vec<ConflictEntry>,
    pub stash_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Head {
    Branch { name: String },
    Detached { oid: String },
    /// Repo baru tanpa commit.
    Unborn { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    pub name: String,
    pub ahead: u32,
    pub behind: u32,
    /// `false` bila upstream dikonfigurasi tetapi ref-nya sudah tidak ada.
    pub exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmoduleState {
    pub commit_changed: bool,
    pub modified: bool,
    pub untracked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub orig_path: Option<String>,
    pub kind: ChangeKind,
    pub submodule: Option<SubmoduleState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    BothModified,
    BothAdded,
    BothDeleted,
    AddedByUs,
    AddedByThem,
    DeletedByUs,
    DeletedByThem,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictEntry {
    pub path: String,
    pub kind: ConflictKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Operation {
    Merge { incoming: String },
    #[serde(rename_all = "camelCase")]
    Rebase { interactive: bool, step: u32, total: u32, onto: String, head_name: String, stopped_at: Option<String> },
    CherryPick { commit: String },
    Revert { commit: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub time: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    Head,
    Branch,
    Remote,
    Tag,
    Stash,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    pub kind: RefKind,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: Signature,
    pub committer: Signature,
    pub subject: String,
    pub refs: Vec<RefLabel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EdgeKind {
    /// Dari jalur di tepi atas baris menuju titik commit.
    Top,
    /// Dari titik commit menuju jalur di tepi bawah baris.
    Bottom,
    /// Jalur yang hanya lewat, dari atas ke bawah.
    Through,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from: u16,
    pub to: u16,
    pub color: u8,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    pub commit: Commit,
    pub column: u16,
    pub color: u8,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub rows: Vec<GraphRow>,
    /// `true` bila tidak ada commit lagi.
    pub done: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogQuery {
    /// Kosong = HEAD. Diabaikan bila `all` bernilai true.
    pub revs: Vec<String>,
    pub all: bool,
    pub grep: Option<String>,
    pub author: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Batasi ke riwayat satu berkas (dengan `--follow`).
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFile {
    pub path: String,
    pub orig_path: Option<String>,
    pub kind: ChangeKind,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

/// Perubahan yang di-stage, sebagai teks untuk menulis pesan commit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedDiff {
    /// Keluaran `git diff --cached --stat`; kosong bila tidak ada yang di-stage.
    pub stat: String,
    pub patch: String,
    /// `patch` dipotong karena terlalu panjang.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    pub commit: Commit,
    pub body: String,
    pub files: Vec<CommitFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub oid: String,
    pub is_head: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub upstream_gone: bool,
    /// Path worktree lain yang sedang memakai branch ini.
    pub worktree: Option<String>,
    pub subject: String,
    pub time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteBranch {
    /// Nama lengkap, mis. `origin/main`.
    pub name: String,
    pub remote: String,
    pub oid: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub name: String,
    /// Commit yang ditunjuk (sudah di-peel untuk annotated tag).
    pub oid: String,
    pub annotated: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refs {
    pub branches: Vec<Branch>,
    pub remote_branches: Vec<RemoteBranch>,
    pub tags: Vec<Tag>,
    pub remotes: Vec<Remote>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stash {
    pub index: u32,
    /// Nama ref, mis. `stash@{0}`.
    pub name: String,
    pub oid: String,
    pub message: String,
    pub time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_main: bool,
    pub is_current: bool,
    pub locked: bool,
    pub prunable: bool,
    pub bare: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SubmoduleStatus {
    Uninitialized,
    UpToDate,
    /// Commit yang di-checkout berbeda dari yang dicatat superproject.
    Modified,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Submodule {
    pub path: String,
    pub oid: String,
    pub status: SubmoduleStatus,
    pub describe: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflogEntry {
    pub oid: String,
    /// Mis. `HEAD@{3}`.
    pub selector: String,
    pub message: String,
    pub time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameRange {
    pub oid: String,
    /// Baris pertama (mulai dari 1) pada berkas saat ini.
    pub start_line: u32,
    pub line_count: u32,
    pub author: String,
    pub author_email: String,
    pub time: Timestamp,
    pub summary: String,
    /// `true` untuk baris yang belum di-commit.
    pub uncommitted: bool,
}

/// Satu sisi tampilan diff.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSide {
    /// `None` bila sisi ini tidak ada (berkas baru/terhapus) atau biner/terlalu besar.
    pub text: Option<String>,
    pub exists: bool,
    pub binary: bool,
    pub too_large: bool,
    pub size: u32,
    /// Sidik isi; dikirim balik saat staging parsial untuk mendeteksi diff yang sudah usang.
    pub fingerprint: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffContents {
    pub path: String,
    pub orig_path: Option<String>,
    pub a: DiffSide,
    pub b: DiffSide,
}

/// Sumber diff yang diminta UI.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffSource {
    /// Index → worktree.
    Worktree,
    /// HEAD → index.
    Staged,
    /// Induk pertama → commit (atau `parent` bila diisi).
    Commit { oid: String, parent: Option<String> },
    /// Dua revisi sembarang.
    Range { from: String, to: String },
    /// Isi sebuah stash terhadap induknya.
    Stash { name: String },
}

/// Satu operasi sambung-potong baris: ganti `A[a_from..a_to)` dengan `B[b_from..b_to)`.
/// Nomor baris mulai dari 0, batas akhir eksklusif.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpliceOp {
    pub a_from: u32,
    pub a_to: u32,
    pub b_from: u32,
    pub b_to: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpliceTarget {
    /// A = index, B = worktree; perubahan terpilih masuk ke index.
    Stage,
    /// A = HEAD, B = index; perubahan terpilih dikeluarkan dari index.
    Unstage,
    /// A = index, B = worktree; perubahan terpilih dibuang dari worktree.
    Discard,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommitOptions {
    pub amend: bool,
    pub signoff: bool,
    pub gpg_sign: bool,
    pub no_verify: bool,
    pub allow_empty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeMode {
    Default,
    NoFf,
    FfOnly,
    Squash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PullStrategy {
    FfOnly,
    Merge,
    Rebase,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PushOptions {
    pub remote: Option<String>,
    /// Branch lokal; kosong = branch aktif.
    pub branch: Option<String>,
    pub set_upstream: bool,
    pub force_with_lease: bool,
    pub tags: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StashOptions {
    pub message: Option<String>,
    pub include_untracked: bool,
    pub staged_only: bool,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RebaseAction {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    Drop,
}

impl RebaseAction {
    pub fn as_str(self) -> &'static str {
        match self {
            RebaseAction::Pick => "pick",
            RebaseAction::Reword => "reword",
            RebaseAction::Edit => "edit",
            RebaseAction::Squash => "squash",
            RebaseAction::Fixup => "fixup",
            RebaseAction::Drop => "drop",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebaseTodoItem {
    pub action: RebaseAction,
    pub oid: String,
    #[serde(default)]
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebasePlan {
    /// Commit dari yang tertua ke yang terbaru.
    pub items: Vec<RebaseTodoItem>,
    /// Jumlah commit dalam rencana yang sudah ada di remote (perlu force push).
    pub published: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationAction {
    Continue,
    Skip,
    Abort,
}

/// Satu segmen hasil penggabungan tiga arah untuk editor konflik.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MergeSegment {
    /// Sudah tergabung tanpa konflik.
    Resolved { text: String },
    Conflict { current: String, base: String, incoming: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDetail {
    pub path: String,
    pub kind: ConflictKind,
    pub binary: bool,
    pub current_label: String,
    pub incoming_label: String,
    pub base: Option<String>,
    pub current: Option<String>,
    pub incoming: Option<String>,
    pub segments: Vec<MergeSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictSide {
    Current,
    Incoming,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Mis. "Receiving objects".
    pub phase: String,
    pub percent: Option<u8>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLogEntry {
    pub command: String,
    pub cwd: String,
    pub duration_ms: u32,
    pub exit_code: i32,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub path: String,
    pub version: String,
    pub supported: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub name: Option<String>,
    pub email: Option<String>,
}
