//! Data structures sent to the UI. All fields use camelCase on the TypeScript side.

use serde::{Deserialize, Serialize};

/// Seconds since the Unix epoch. `f64` so it can be safely exported as a `number` in TypeScript.
pub type Timestamp = f64;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSnapshot {
    /// Root path of the worktree.
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
    /// A new repo with no commits.
    Unborn { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    pub name: String,
    pub ahead: u32,
    pub behind: u32,
    /// `false` if the upstream is configured but its ref no longer exists.
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
    /// From the lane at the top edge of the row to the commit dot.
    Top,
    /// From the commit dot to the lane at the bottom edge of the row.
    Bottom,
    /// A lane that only passes through, from top to bottom.
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
    /// `true` if there are no more commits.
    pub done: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogQuery {
    /// Empty = HEAD. Ignored if `all` is true.
    pub revs: Vec<String>,
    pub all: bool,
    pub grep: Option<String>,
    pub author: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Limit to the history of a single file (with `--follow`).
    pub path: Option<String>,
}

impl LogQuery {
    /// Only some commits are listed (a message search, an author, or one file's history). git then reports
    /// each commit's real parents, which are mostly not in the list, so no graph can be drawn from them.
    pub fn is_filtered(&self) -> bool {
        let set = |value: &Option<String>| value.as_deref().is_some_and(|value| !value.is_empty());
        set(&self.grep) || set(&self.author) || self.path.is_some()
    }
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

/// Staged changes, as text for writing a commit message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedDiff {
    /// Output of `git diff --cached --stat`; empty if nothing is staged.
    pub stat: String,
    pub patch: String,
    /// `patch` was truncated because it was too long.
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
    /// Path of another worktree that is currently using this branch.
    pub worktree: Option<String>,
    pub subject: String,
    pub time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteBranch {
    /// Full name, e.g. `origin/main`.
    pub name: String,
    pub remote: String,
    pub oid: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub name: String,
    /// The commit pointed to (already peeled for annotated tags).
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
    /// Ref name, e.g. `stash@{0}`.
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
    /// The checked-out commit differs from the one recorded by the superproject.
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
    /// E.g. `HEAD@{3}`.
    pub selector: String,
    pub message: String,
    pub time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameRange {
    pub oid: String,
    /// First line (1-based) in the current file.
    pub start_line: u32,
    pub line_count: u32,
    pub author: String,
    pub author_email: String,
    pub time: Timestamp,
    pub summary: String,
    /// `true` for lines that are not committed yet.
    pub uncommitted: bool,
}

/// One side of the diff view.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSide {
    /// `None` if this side does not exist (new/deleted file) or is binary/too large.
    pub text: Option<String>,
    pub exists: bool,
    pub binary: bool,
    pub too_large: bool,
    pub size: u32,
    /// Content fingerprint; sent back during partial staging to detect a stale diff.
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

/// The diff source requested by the UI.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffSource {
    /// Index → worktree.
    Worktree,
    /// HEAD → index.
    Staged,
    /// First parent → commit (or `parent` if given).
    Commit { oid: String, parent: Option<String> },
    /// Any two revisions.
    Range { from: String, to: String },
    /// The contents of a stash against its parent.
    Stash { name: String },
}

/// A single line splice operation: replace `A[a_from..a_to)` with `B[b_from..b_to)`.
/// Line numbers are 0-based, and the end bound is exclusive.
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
    /// A = index, B = worktree; the selected changes go into the index.
    Stage,
    /// A = HEAD, B = index; the selected changes are removed from the index.
    Unstage,
    /// A = index, B = worktree; the selected changes are discarded from the worktree.
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
    /// Local branch; empty = the active branch.
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
    /// Commits from oldest to newest.
    pub items: Vec<RebaseTodoItem>,
    /// Number of commits in the plan that already exist on the remote (requires a force push).
    pub published: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationAction {
    Continue,
    Skip,
    Abort,
}

/// One segment of a three-way merge result for the conflict editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MergeSegment {
    /// Already merged without conflict.
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
    /// How the file is encoded; the resolved file is written back the same way.
    pub format: TextFormat,
    /// The file in the working tree when the detail was read (`None` if it is missing or binary). Writing
    /// the result checks that it is still the same, so changes made meanwhile are not lost.
    pub worktree: Option<String>,
    /// The segments come from the working tree, not from a fresh merge of the three versions: it was
    /// already edited (some conflicts resolved by hand, by rerere, or by a merge driver).
    pub from_worktree: bool,
}

/// How a text file is stored: its encoding (a WHATWG label, e.g. `UTF-8`, `windows-1252`), whether it
/// starts with a byte order mark, and whether its lines mostly end with `\r\n`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFormat {
    pub encoding: String,
    pub bom: bool,
    pub crlf: bool,
}

impl Default for TextFormat {
    fn default() -> Self {
        Self { encoding: "UTF-8".into(), bom: false, crlf: false }
    }
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
    /// E.g. "Receiving objects".
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
