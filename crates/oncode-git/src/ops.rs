//! Operasi Git sehari-hari sampai tingkat lanjut. Setiap operasi tulis mengambil
//! giliran lewat [`Repo::lock`] sehingga tidak ada dua operasi tulis yang bersamaan.

use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::error::{GitError, GitErrorCode, Result};
use crate::helper::{EditorMode, OpContext};
use crate::log::{parse_commits, parse_raw, FORMAT};
use crate::model::*;
use crate::repo::Repo;
use crate::runner::{check_path, check_ref, Cmd, Git, ProgressFn};
use crate::status::detect_operation;

fn check_paths(paths: &[String]) -> Result<Vec<&str>> {
    paths.iter().map(|p| check_path(p)).collect()
}

fn is_hex_oid(value: &str) -> bool {
    value.len() >= 7 && value.len() <= 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

impl Repo {
    fn ctx(&self, editor: EditorMode) -> OpContext {
        OpContext { repo: Some(self.root().to_path_buf()), todo: None, editor }
    }

    async fn has_head(&self) -> bool {
        self.run_raw(Cmd::new(["rev-parse", "--verify", "--quiet", "HEAD"])).await.map(|o| o.code == 0).unwrap_or(false)
    }

    pub async fn rev_parse(&self, rev: &str) -> Result<String> {
        let rev = check_ref(rev)?;
        Ok(self.run(Cmd::new(["rev-parse", "--verify", "--end-of-options", rev])).await?.trimmed())
    }

    /// Leluhur bersama terdekat dua revisi, atau `None` bila riwayatnya tidak berhubungan.
    pub async fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let (a, b) = (check_ref(a)?, check_ref(b)?);
        let out = self.run(Cmd::new(["merge-base", "--end-of-options", a, b]).ok_codes(&[0, 1])).await?;
        Ok(Some(out.trimmed()).filter(|oid| !oid.is_empty()))
    }

    // ----- Staging -----

    pub async fn stage(&self, paths: &[String]) -> Result<()> {
        let paths = check_paths(paths)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["add", "-A"]).paths(paths).write()).await.map(drop)
    }

    pub async fn stage_all(&self) -> Result<()> {
        let _guard = self.lock().await;
        self.run(Cmd::new(["add", "-A"]).write()).await.map(drop)
    }

    pub async fn unstage(&self, paths: &[String]) -> Result<()> {
        let paths = check_paths(paths)?;
        let _guard = self.lock().await;
        let cmd = if self.has_head().await {
            Cmd::new(["restore", "--staged"])
        } else {
            Cmd::new(["rm", "--cached", "-r", "-q"])
        };
        self.run(cmd.paths(paths).write()).await.map(drop)
    }

    pub async fn unstage_all(&self) -> Result<()> {
        let _guard = self.lock().await;
        let cmd = if self.has_head().await {
            Cmd::new(["reset", "-q"])
        } else {
            Cmd::new(["rm", "--cached", "-r", "-q", "--", "."])
        };
        self.run(cmd.write()).await.map(drop)
    }

    /// Mengembalikan berkas terlacak di working tree ke isi index. Berkas untracked
    /// tidak disentuh; pemanggil memindahkannya ke tempat sampah.
    pub async fn discard(&self, paths: &[String]) -> Result<()> {
        let paths = check_paths(paths)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["restore", "--worktree"]).paths(paths).write()).await.map(drop)
    }

    // ----- Commit -----

    /// Membuat commit dari isi index dan mengembalikan hash-nya.
    pub async fn commit(&self, message: &str, options: &CommitOptions) -> Result<String> {
        if message.trim().is_empty() {
            return Err(GitError::invalid("Commit message is empty"));
        }
        let _guard = self.lock().await;
        let cmd = Cmd::new(["commit", "-F", "-"])
            .arg_if(options.amend, "--amend")
            .arg_if(options.signoff, "--signoff")
            .arg_if(options.gpg_sign, "-S")
            .arg_if(options.no_verify, "--no-verify")
            .arg_if(options.allow_empty, "--allow-empty")
            .stdin(message.as_bytes().to_vec())
            .write();
        self.run(cmd).await?;
        Ok(self.run(Cmd::new(["rev-parse", "HEAD"])).await?.trimmed())
    }

    /// Membatalkan commit terakhir; perubahannya kembali ke index.
    pub async fn undo_last_commit(&self) -> Result<()> {
        let _guard = self.lock().await;
        self.run(Cmd::new(["reset", "--soft", "HEAD~1"]).write()).await.map(drop)
    }

    /// Pesan commit terakhir (untuk mengisi kotak commit saat amend).
    pub async fn last_commit_message(&self) -> Result<String> {
        let out = self.run(Cmd::new(["log", "-1", "--format=%B"]).ok_codes(&[0, 128])).await?;
        Ok(out.text().trim_end().to_string())
    }

    /// Subjek `limit` commit terakhir, terbaru lebih dulu; kosong pada repositori tanpa commit.
    pub async fn recent_subjects(&self, limit: u32) -> Result<Vec<String>> {
        let out = self.run(Cmd::new(["log".to_string(), format!("--max-count={limit}"), "--format=%s".to_string()]).ok_codes(&[0, 128])).await?;
        Ok(out.text().lines().map(str::trim).filter(|line| !line.is_empty()).map(String::from).collect())
    }

    /// Perubahan yang di-stage (untuk menulis pesan commit): ringkasan `--stat` dan patch-nya.
    /// Patch yang lebih panjang dari `max_bytes` dipotong di batas karakter dan `truncated` diisi.
    /// Pengaturan diff milik pengguna (alat diff eksternal, warna) tidak dipakai.
    pub async fn staged_diff(&self, max_bytes: usize) -> Result<StagedDiff> {
        let base = ["diff", "--cached", "--no-color", "--no-ext-diff", "-M"];
        let stat = self.run(Cmd::new(base).args(["--stat"])).await?.text();
        let mut patch = self.run(Cmd::new(base).args(["-U3"])).await?.text();
        let truncated = patch.len() > max_bytes;
        if truncated {
            let mut end = max_bytes;
            while !patch.is_char_boundary(end) {
                end -= 1;
            }
            patch.truncate(end);
        }
        Ok(StagedDiff { stat: stat.trim_end().to_string(), patch, truncated })
    }

    // ----- Ref -----

    pub async fn refs(&self) -> Result<Refs> {
        let format = "--format=%(refname)%1f%(objectname)%1f%(*objectname)%1f%(upstream:short)%1f%(upstream:track)%1f%(HEAD)%1f%(worktreepath)%1f%(contents:subject)%1f%(creatordate:unix)%1f%(objecttype)";
        let out = self.run(Cmd::new(["for-each-ref", format, "refs/heads", "refs/remotes", "refs/tags"])).await?;
        let root = self.root_string();
        let mut refs = Refs { branches: vec![], remote_branches: vec![], tags: vec![], remotes: vec![] };

        for line in out.text().lines() {
            let f: Vec<&str> = line.split('\x1f').collect();
            if f.len() < 10 {
                continue;
            }
            let (refname, oid, peeled, upstream, track, head, worktree, subject, date, objtype) =
                (f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7], f[8], f[9]);
            if let Some(name) = refname.strip_prefix("refs/heads/") {
                let (ahead, behind, gone) = parse_track(track);
                refs.branches.push(Branch {
                    name: name.to_string(),
                    oid: oid.to_string(),
                    is_head: head == "*",
                    upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                    ahead,
                    behind,
                    upstream_gone: gone,
                    worktree: (!worktree.is_empty() && worktree != root).then(|| worktree.to_string()),
                    subject: subject.to_string(),
                    time: date.parse::<i64>().unwrap_or(0) as Timestamp,
                });
            } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
                if name.ends_with("/HEAD") {
                    continue;
                }
                let remote = name.split('/').next().unwrap_or("").to_string();
                refs.remote_branches.push(RemoteBranch { name: name.to_string(), remote, oid: oid.to_string() });
            } else if let Some(name) = refname.strip_prefix("refs/tags/") {
                let annotated = objtype == "tag";
                refs.tags.push(Tag {
                    name: name.to_string(),
                    oid: if peeled.is_empty() { oid.to_string() } else { peeled.to_string() },
                    annotated,
                    message: if annotated { subject.to_string() } else { String::new() },
                });
            }
        }
        refs.remotes = self.remotes().await?;
        Ok(refs)
    }

    pub async fn remotes(&self) -> Result<Vec<Remote>> {
        let out = self.run(Cmd::new(["remote", "-v"])).await?;
        let mut remotes: Vec<Remote> = Vec::new();
        for line in out.text().lines() {
            let Some((name, rest)) = line.split_once('\t') else { continue };
            let (url, kind) = match rest.rsplit_once(' ') {
                Some((url, kind)) => (url.to_string(), kind),
                None => (rest.to_string(), "(fetch)"),
            };
            let index = match remotes.iter().position(|r| r.name == name) {
                Some(index) => index,
                None => {
                    remotes.push(Remote { name: name.to_string(), fetch_url: String::new(), push_url: String::new() });
                    remotes.len() - 1
                }
            };
            if kind == "(push)" {
                remotes[index].push_url = url;
            } else {
                remotes[index].fetch_url = url;
            }
        }
        Ok(remotes)
    }

    pub async fn switch(&self, branch: &str) -> Result<()> {
        let branch = check_ref(branch)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["switch", "--end-of-options", branch]).write()).await.map(drop)
    }

    /// Checkout sebuah commit atau tag tanpa branch (HEAD terlepas).
    pub async fn switch_detached(&self, rev: &str) -> Result<()> {
        let rev = check_ref(rev)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["switch", "--detach", "--end-of-options", rev]).write()).await.map(drop)
    }

    /// Membuat branch lokal yang melacak branch remote (mis. `origin/fitur` → `fitur`) lalu pindah ke sana.
    pub async fn switch_tracking(&self, remote_branch: &str) -> Result<()> {
        let remote_branch = check_ref(remote_branch)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["switch", "--track", "--end-of-options", remote_branch]).write()).await.map(drop)
    }

    pub async fn create_branch(&self, name: &str, start: Option<&str>, checkout: bool) -> Result<()> {
        let name = check_ref(name)?;
        let start = start.map(check_ref).transpose()?;
        let _guard = self.lock().await;
        let mut cmd = if checkout { Cmd::new(["switch", "-c", name]) } else { Cmd::new(["branch", "--end-of-options", name]) };
        if let Some(start) = start {
            cmd = cmd.arg(start);
        }
        self.run(cmd.write()).await.map(drop)
    }

    pub async fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        let (old, new) = (check_ref(old)?, check_ref(new)?);
        let _guard = self.lock().await;
        self.run(Cmd::new(["branch", "-m", "--end-of-options", old, new]).write()).await.map(drop)
    }

    /// Menghapus branch lokal. Tanpa `force`, branch yang belum di-merge ditolak (`BranchNotMerged`).
    pub async fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let name = check_ref(name)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["branch", if force { "-D" } else { "-d" }, "--end-of-options", name]).write()).await.map(drop)
    }

    /// Jumlah commit yang hanya ada di branch ini (tidak di HEAD maupun remote mana pun).
    pub async fn unmerged_count(&self, branch: &str) -> Result<u32> {
        let branch = check_ref(branch)?;
        let out = self.run(Cmd::new(["rev-list", "--count", "--not", "HEAD", "--remotes", "--not", "--end-of-options", branch])).await?;
        Ok(out.trimmed().parse().unwrap_or(0))
    }

    pub async fn set_upstream(&self, branch: &str, upstream: Option<&str>) -> Result<()> {
        let branch = check_ref(branch)?;
        let _guard = self.lock().await;
        let cmd = match upstream {
            Some(upstream) => Cmd::new([format!("--set-upstream-to={}", check_ref(upstream)?).as_str(), "--end-of-options", branch]),
            None => Cmd::new(["--unset-upstream", "--end-of-options", branch]),
        };
        let mut full = Cmd::new(["branch"]);
        full.args.extend(cmd.args);
        self.run(full.write()).await.map(drop)
    }

    // ----- Merge, rebase, dan operasi berlanjut -----

    pub async fn merge(&self, rev: &str, mode: MergeMode) -> Result<()> {
        let rev = check_ref(rev)?;
        let _guard = self.lock().await;
        let cmd = Cmd::new(["merge", "--no-edit"]);
        let cmd = match mode {
            MergeMode::Default => cmd,
            MergeMode::NoFf => cmd.arg("--no-ff"),
            MergeMode::FfOnly => cmd.arg("--ff-only"),
            MergeMode::Squash => cmd.arg("--squash"),
        };
        self.run(cmd.args(["--end-of-options", rev]).write().context(self.ctx(EditorMode::AcceptDefault))).await.map(drop)
    }

    /// Rebase branch aktif ke atas `onto`.
    pub async fn rebase(&self, onto: &str) -> Result<()> {
        let onto = check_ref(onto)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["rebase", "--end-of-options", onto]).write().context(self.ctx(EditorMode::AcceptDefault)))
            .await
            .map(drop)
    }

    /// Daftar commit yang akan disusun ulang bila rebase interaktif dimulai dari `base`
    /// (`None` = dari commit pertama).
    pub async fn rebase_plan(&self, base: Option<&str>) -> Result<RebasePlan> {
        let range = match base {
            Some(base) => format!("{}..HEAD", check_ref(base)?),
            None => "HEAD".to_string(),
        };
        let out = self.run(Cmd::new(["log", "-z", "--reverse", "--no-merges", FORMAT, "--end-of-options", &range])).await?;
        let commits = parse_commits(&out.stdout);
        let unpublished = self
            .run(Cmd::new(["rev-list", "--count", "--no-merges", "--not", "--remotes", "--not", "--end-of-options", &range]))
            .await?
            .trimmed()
            .parse::<u32>()
            .unwrap_or(0);
        let total = commits.len() as u32;
        Ok(RebasePlan {
            items: commits
                .into_iter()
                .map(|c| RebaseTodoItem { action: RebaseAction::Pick, oid: c.oid, subject: c.subject })
                .collect(),
            published: total.saturating_sub(unpublished),
        })
    }

    /// Menjalankan rebase interaktif dengan susunan dari UI.
    pub async fn rebase_interactive(&self, base: Option<&str>, items: &[RebaseTodoItem]) -> Result<()> {
        let todo = build_todo(items)?;
        let base = base.map(check_ref).transpose()?;
        let _guard = self.lock().await;
        let mut cmd = Cmd::new(["-c", "rebase.abbreviateCommands=false", "-c", "core.commentChar=#", "rebase", "-i"]);
        cmd = match base {
            Some(base) => cmd.args(["--end-of-options", base]),
            None => cmd.arg("--root"),
        };
        let context = OpContext { repo: Some(self.root().to_path_buf()), todo: Some(todo), editor: EditorMode::Prompt };
        self.run(cmd.write().context(context)).await.map(drop)
    }

    /// Melanjutkan, melewati, atau membatalkan operasi yang sedang berhenti.
    pub async fn operation(&self, action: OperationAction) -> Result<()> {
        let Some(operation) = detect_operation(self.git_dir()) else {
            return Err(GitError::invalid("No operation is in progress"));
        };
        let _guard = self.lock().await;
        let flag = match action {
            OperationAction::Continue => "--continue",
            OperationAction::Skip => "--skip",
            OperationAction::Abort => "--abort",
        };
        let (command, editor) = match operation {
            Operation::Merge { .. } => {
                if action == OperationAction::Skip {
                    return Err(GitError::invalid("A merge cannot be skipped"));
                }
                ("merge", EditorMode::AcceptDefault)
            }
            Operation::Rebase { interactive, .. } => {
                ("rebase", if interactive { EditorMode::Prompt } else { EditorMode::AcceptDefault })
            }
            Operation::CherryPick { .. } => ("cherry-pick", EditorMode::AcceptDefault),
            Operation::Revert { .. } => ("revert", EditorMode::AcceptDefault),
        };
        self.run(Cmd::new([command, flag]).write().context(self.ctx(editor))).await.map(drop)
    }

    // ----- Remote -----

    pub async fn fetch(&self, remote: Option<&str>, cancel: Option<CancellationToken>, progress: Option<ProgressFn>) -> Result<()> {
        let cmd = Cmd::new(["fetch", "--prune", "--progress"]);
        let cmd = match remote {
            Some(remote) => cmd.args(["--end-of-options", check_ref(remote)?]),
            None => cmd.arg("--all"),
        };
        // Fetch tidak mengubah working tree, jadi tidak perlu menunggu antrean tulis.
        let mut cmd = cmd.cancel(cancel).progress(progress);
        cmd.log = true;
        self.run(cmd).await.map(drop)
    }

    /// Mengambil satu refspec dari remote, mis. `refs/pull/12/head:pr/12` untuk checkout pull request.
    pub async fn fetch_refspec(&self, remote: &str, refspec: &str, cancel: Option<CancellationToken>, progress: Option<ProgressFn>) -> Result<()> {
        let (remote, refspec) = (check_ref(remote)?, check_ref(refspec)?);
        let mut cmd = Cmd::new(["fetch", "--progress", "--end-of-options", remote, refspec]).cancel(cancel).progress(progress);
        cmd.log = true;
        self.run(cmd).await.map(drop)
    }

    pub async fn pull(&self, strategy: PullStrategy, cancel: Option<CancellationToken>, progress: Option<ProgressFn>) -> Result<()> {
        let _guard = self.lock().await;
        let flag = match strategy {
            PullStrategy::FfOnly => "--ff-only",
            PullStrategy::Merge => "--no-rebase",
            PullStrategy::Rebase => "--rebase",
        };
        let cmd = Cmd::new(["pull", "--progress", "--no-edit", flag])
            .write()
            .cancel(cancel)
            .progress(progress)
            .context(self.ctx(EditorMode::AcceptDefault));
        self.run(cmd).await.map(drop)
    }

    pub async fn push(&self, options: &PushOptions, cancel: Option<CancellationToken>, progress: Option<ProgressFn>) -> Result<()> {
        let mut cmd = Cmd::new(["push", "--progress"])
            .arg_if(options.set_upstream, "--set-upstream")
            .arg_if(options.force_with_lease, "--force-with-lease")
            .arg_if(options.tags, "--tags");
        let remote = match &options.remote {
            Some(remote) => Some(check_ref(remote)?.to_string()),
            None if options.set_upstream || options.branch.is_some() => {
                Some(self.remotes().await?.first().map(|r| r.name.clone()).ok_or_else(|| {
                    GitError::new(GitErrorCode::NoUpstream, "This repository has no remote yet")
                })?)
            }
            None => None,
        };
        if let Some(remote) = remote {
            cmd = cmd.args(["--end-of-options".to_string(), remote]);
            match &options.branch {
                Some(branch) => cmd = cmd.arg(check_ref(branch)?),
                None if options.set_upstream => cmd = cmd.arg("HEAD"),
                None => {}
            }
        }
        let mut cmd = cmd.cancel(cancel).progress(progress);
        cmd.log = true;
        self.run(cmd).await.map(drop)
    }

    pub async fn delete_remote_branch(&self, remote: &str, branch: &str) -> Result<()> {
        let (remote, branch) = (check_ref(remote)?, check_ref(branch)?);
        let mut cmd = Cmd::new(["push", "--delete", "--end-of-options", remote, branch]);
        cmd.log = true;
        self.run(cmd).await.map(drop)
    }

    pub async fn remote_add(&self, name: &str, url: &str) -> Result<()> {
        let (name, url) = (check_ref(name)?, check_ref(url)?);
        let _guard = self.lock().await;
        self.run(Cmd::new(["remote", "add", "--end-of-options", name, url]).write()).await.map(drop)
    }

    pub async fn remote_remove(&self, name: &str) -> Result<()> {
        let name = check_ref(name)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["remote", "remove", "--end-of-options", name]).write()).await.map(drop)
    }

    pub async fn remote_rename(&self, old: &str, new: &str) -> Result<()> {
        let (old, new) = (check_ref(old)?, check_ref(new)?);
        let _guard = self.lock().await;
        self.run(Cmd::new(["remote", "rename", "--end-of-options", old, new]).write()).await.map(drop)
    }

    pub async fn remote_set_url(&self, name: &str, url: &str) -> Result<()> {
        let (name, url) = (check_ref(name)?, check_ref(url)?);
        let _guard = self.lock().await;
        self.run(Cmd::new(["remote", "set-url", "--end-of-options", name, url]).write()).await.map(drop)
    }

    // ----- Stash -----

    pub async fn stash_list(&self) -> Result<Vec<Stash>> {
        let out = self.run(Cmd::new(["stash", "list", "-z", "--format=%gd%x1f%H%x1f%gs%x1f%ct"])).await?;
        Ok(out
            .text()
            .split('\0')
            .filter(|r| !r.trim().is_empty())
            .enumerate()
            .filter_map(|(index, record)| {
                let f: Vec<&str> = record.trim_start_matches('\n').split('\x1f').collect();
                (f.len() >= 4).then(|| Stash {
                    index: index as u32,
                    name: f[0].to_string(),
                    oid: f[1].to_string(),
                    message: f[2].to_string(),
                    time: f[3].trim().parse::<i64>().unwrap_or(0) as Timestamp,
                })
            })
            .collect())
    }

    pub async fn stash_push(&self, options: &StashOptions) -> Result<()> {
        let paths = check_paths(&options.paths)?;
        let _guard = self.lock().await;
        let mut cmd = Cmd::new(["stash", "push"])
            .arg_if(options.include_untracked, "--include-untracked")
            .arg_if(options.staged_only, "--staged");
        if let Some(message) = options.message.as_deref().filter(|m| !m.trim().is_empty()) {
            cmd = cmd.args(["-m", message]);
        }
        if !paths.is_empty() {
            cmd = cmd.paths(paths);
        }
        self.run(cmd.write()).await.map(drop)
    }

    /// Menerapkan stash; `pop` juga menghapusnya bila berhasil.
    pub async fn stash_apply(&self, name: &str, pop: bool) -> Result<()> {
        let name = check_ref(name)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["stash", if pop { "pop" } else { "apply" }, "--end-of-options", name]).write()).await.map(drop)
    }

    pub async fn stash_drop(&self, name: &str) -> Result<()> {
        let name = check_ref(name)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["stash", "drop", "--end-of-options", name]).write()).await.map(drop)
    }

    pub async fn stash_branch(&self, name: &str, branch: &str) -> Result<()> {
        let (name, branch) = (check_ref(name)?, check_ref(branch)?);
        let _guard = self.lock().await;
        self.run(Cmd::new(["stash", "branch", "--end-of-options", branch, name]).write()).await.map(drop)
    }

    pub async fn stash_files(&self, name: &str) -> Result<Vec<CommitFile>> {
        let name = check_ref(name)?;
        let out = self
            .run(Cmd::new(["stash", "show", "--include-untracked", "--raw", "-z", "--abbrev=40", "--end-of-options", name]))
            .await?;
        Ok(parse_raw(&out.stdout))
    }

    // ----- Tag -----

    /// Membuat tag; dengan `message` menjadi annotated tag.
    pub async fn tag_create(&self, name: &str, rev: Option<&str>, message: Option<&str>) -> Result<()> {
        let name = check_ref(name)?;
        let rev = rev.map(check_ref).transpose()?;
        let _guard = self.lock().await;
        let mut cmd = Cmd::new(["tag"]);
        if let Some(message) = message.filter(|m| !m.trim().is_empty()) {
            cmd = cmd.args(["-a", "-m", message]);
        }
        cmd = cmd.args(["--end-of-options", name]);
        if let Some(rev) = rev {
            cmd = cmd.arg(rev);
        }
        self.run(cmd.write()).await.map(drop)
    }

    pub async fn tag_delete(&self, name: &str) -> Result<()> {
        let name = check_ref(name)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["tag", "-d", "--end-of-options", name]).write()).await.map(drop)
    }

    pub async fn tag_push(&self, remote: &str, name: &str) -> Result<()> {
        let (remote, name) = (check_ref(remote)?, check_ref(name)?);
        let mut cmd = Cmd::new(["push", "--end-of-options", remote, &format!("refs/tags/{name}")]);
        cmd.log = true;
        self.run(cmd).await.map(drop)
    }

    pub async fn tag_delete_remote(&self, remote: &str, name: &str) -> Result<()> {
        let (remote, name) = (check_ref(remote)?, check_ref(name)?);
        let mut cmd = Cmd::new(["push", "--delete", "--end-of-options", remote, &format!("refs/tags/{name}")]);
        cmd.log = true;
        self.run(cmd).await.map(drop)
    }

    // ----- Cherry-pick, revert, reset -----

    async fn is_merge_commit(&self, oid: &str) -> bool {
        self.run(Cmd::new(["rev-list", "--parents", "-n", "1", "--end-of-options", oid]))
            .await
            .map(|o| o.trimmed().split(' ').count() > 2)
            .unwrap_or(false)
    }

    /// Menerapkan satu atau beberapa commit ke branch aktif, dari yang pertama diberikan.
    pub async fn cherry_pick(&self, oids: &[String]) -> Result<()> {
        if oids.is_empty() {
            return Err(GitError::invalid("No commits selected"));
        }
        let mut cmd = Cmd::new(["cherry-pick"]);
        for oid in oids {
            check_ref(oid)?;
        }
        if oids.len() == 1 && self.is_merge_commit(&oids[0]).await {
            cmd = cmd.args(["-m", "1"]);
        }
        let _guard = self.lock().await;
        cmd = cmd.arg("--end-of-options").args(oids.iter().map(String::as_str));
        self.run(cmd.write().context(self.ctx(EditorMode::AcceptDefault))).await.map(drop)
    }

    pub async fn revert(&self, oid: &str) -> Result<()> {
        let oid = check_ref(oid)?;
        let mut cmd = Cmd::new(["revert", "--no-edit"]);
        if self.is_merge_commit(oid).await {
            cmd = cmd.args(["-m", "1"]);
        }
        let _guard = self.lock().await;
        self.run(cmd.args(["--end-of-options", oid]).write().context(self.ctx(EditorMode::AcceptDefault))).await.map(drop)
    }

    pub async fn reset(&self, rev: &str, mode: ResetMode) -> Result<()> {
        let rev = check_ref(rev)?;
        let _guard = self.lock().await;
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.run(Cmd::new(["reset", flag, "--end-of-options", rev]).write()).await.map(drop)
    }

    // ----- Worktree -----

    pub async fn worktrees(&self) -> Result<Vec<Worktree>> {
        let out = self.run(Cmd::new(["worktree", "list", "--porcelain", "-z"])).await?;
        Ok(parse_worktrees(&out.stdout, &self.root_string()))
    }

    /// Menambah worktree di `path`. `new_branch` membuat branch baru dari `rev`;
    /// tanpa itu `rev` (branch yang ada) di-checkout.
    pub async fn worktree_add(&self, path: &Path, new_branch: Option<&str>, rev: Option<&str>) -> Result<()> {
        let rev = rev.map(check_ref).transpose()?;
        let _guard = self.lock().await;
        let mut cmd = Cmd::new(["worktree", "add"]);
        if let Some(branch) = new_branch {
            cmd = cmd.args(["-b", check_ref(branch)?]);
        }
        cmd = cmd.arg("--end-of-options").arg(path.as_os_str());
        if let Some(rev) = rev {
            cmd = cmd.arg(rev);
        }
        self.run(cmd.write()).await.map(drop)
    }

    pub async fn worktree_remove(&self, path: &Path, force: bool) -> Result<()> {
        let _guard = self.lock().await;
        let cmd = Cmd::new(["worktree", "remove"]).arg_if(force, "--force").arg("--end-of-options").arg(path.as_os_str());
        self.run(cmd.write()).await.map(drop)
    }

    pub async fn worktree_prune(&self) -> Result<()> {
        let _guard = self.lock().await;
        self.run(Cmd::new(["worktree", "prune"]).write()).await.map(drop)
    }

    // ----- Submodule -----

    pub async fn submodules(&self) -> Result<Vec<Submodule>> {
        if !self.root().join(".gitmodules").exists() {
            return Ok(Vec::new());
        }
        let out = self.run(Cmd::new(["submodule", "status"])).await?;
        Ok(parse_submodules(&out.text()))
    }

    /// `git submodule update`; daftar path kosong berarti semua submodule.
    pub async fn submodule_update(&self, paths: &[String], init: bool, cancel: Option<CancellationToken>, progress: Option<ProgressFn>) -> Result<()> {
        let paths = check_paths(paths)?;
        let _guard = self.lock().await;
        let mut cmd = Cmd::new(["submodule", "update", "--recursive", "--progress"]).arg_if(init, "--init");
        if !paths.is_empty() {
            cmd = cmd.paths(paths);
        }
        self.run(cmd.write().cancel(cancel).progress(progress)).await.map(drop)
    }

    pub async fn submodule_sync(&self) -> Result<()> {
        let _guard = self.lock().await;
        self.run(Cmd::new(["submodule", "sync", "--recursive"]).write()).await.map(drop)
    }

    // ----- Reflog dan konfigurasi -----

    pub async fn reflog(&self, limit: u32) -> Result<Vec<ReflogEntry>> {
        let out = self
            .run(Cmd::new(["reflog", "-z", &format!("--max-count={limit}"), "--format=%H%x1f%gd%x1f%gs%x1f%ct"]).ok_codes(&[0, 128]))
            .await?;
        Ok(out
            .text()
            .split('\0')
            .filter_map(|record| {
                let f: Vec<&str> = record.trim_start_matches('\n').split('\x1f').collect();
                (f.len() >= 4).then(|| ReflogEntry {
                    oid: f[0].to_string(),
                    selector: f[1].to_string(),
                    message: f[2].to_string(),
                    time: f[3].trim().parse::<i64>().unwrap_or(0) as Timestamp,
                })
            })
            .collect())
    }

    async fn config_get(&self, key: &str) -> Option<String> {
        let out = self.run_raw(Cmd::new(["config", "--get", key])).await.ok()?;
        (out.code == 0).then(|| out.trimmed()).filter(|v| !v.is_empty())
    }

    pub async fn identity(&self) -> Identity {
        Identity { name: self.config_get("user.name").await, email: self.config_get("user.email").await }
    }

    pub async fn set_identity(&self, name: &str, email: &str, global: bool) -> Result<()> {
        let _guard = self.lock().await;
        for (key, value) in [("user.name", name), ("user.email", email)] {
            let cmd = Cmd::new(["config"]).arg_if(global, "--global").args([key, value]);
            self.run(cmd.write()).await?;
        }
        Ok(())
    }
}

impl Git {
    /// Meng-clone `url` ke direktori `dest` (harus belum ada atau kosong).
    pub async fn clone_repo(
        self: &Arc<Self>,
        url: &str,
        dest: &Path,
        recurse_submodules: bool,
        cancel: Option<CancellationToken>,
        progress: Option<ProgressFn>,
    ) -> Result<()> {
        let url = check_ref(url)?;
        let parent = dest.parent().ok_or_else(|| GitError::invalid("Invalid destination location"))?;
        let mut cmd = Cmd::new(["clone", "--progress"])
            .arg_if(recurse_submodules, "--recurse-submodules")
            .arg("--end-of-options")
            .arg(url)
            .arg(dest.as_os_str())
            .cancel(cancel)
            .progress(progress);
        cmd.log = true;
        cmd.write = true;
        self.run(parent, cmd).await.map(drop)
    }

    /// Menjadikan `path` repositori baru.
    pub async fn init(self: &Arc<Self>, path: &Path) -> Result<()> {
        self.run(path, Cmd::new(["init"]).write()).await.map(drop)
    }
}

/// Mengurai `%(upstream:track)`: `[ahead 1, behind 2]`, `[gone]`, atau kosong.
fn parse_track(track: &str) -> (u32, u32, bool) {
    let inner = track.trim().trim_start_matches('[').trim_end_matches(']');
    if inner == "gone" {
        return (0, 0, true);
    }
    let mut ahead = 0;
    let mut behind = 0;
    for part in inner.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    (ahead, behind, false)
}

/// Menyusun isi berkas todo dari pilihan pengguna dan menolak susunan yang tidak sah.
pub fn build_todo(items: &[RebaseTodoItem]) -> Result<String> {
    let mut todo = String::new();
    let mut has_base = false;
    for item in items {
        if !is_hex_oid(&item.oid) {
            return Err(GitError::invalid(format!("Invalid commit hash: {:?}", item.oid)));
        }
        match item.action {
            RebaseAction::Squash | RebaseAction::Fixup if !has_base => {
                return Err(GitError::invalid("Squash/fixup needs a preceding commit to combine with"));
            }
            RebaseAction::Drop => {}
            RebaseAction::Squash | RebaseAction::Fixup => {}
            _ => has_base = true,
        }
        todo.push_str(&format!("{} {}\n", item.action.as_str(), item.oid));
    }
    if !has_base {
        return Err(GitError::invalid("All commits are dropped; nothing is left to rebase"));
    }
    Ok(todo)
}

/// Mengurai `git worktree list --porcelain -z`.
pub fn parse_worktrees(raw: &[u8], current_root: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in raw.split(|b| *b == 0) {
        let line = String::from_utf8_lossy(field);
        if line.is_empty() {
            if let Some(done) = current.take() {
                worktrees.push(done);
            }
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line.as_ref(), ""));
        if key == "worktree" {
            if let Some(done) = current.take() {
                worktrees.push(done);
            }
            current = Some(Worktree {
                path: value.to_string(),
                head: None,
                branch: None,
                is_main: false,
                is_current: value == current_root,
                locked: false,
                prunable: false,
                bare: false,
            });
        } else if let Some(wt) = current.as_mut() {
            match key {
                "HEAD" => wt.head = Some(value.to_string()),
                "branch" => wt.branch = Some(value.trim_start_matches("refs/heads/").to_string()),
                "locked" => wt.locked = true,
                "prunable" => wt.prunable = true,
                "bare" => wt.bare = true,
                _ => {}
            }
        }
    }
    if let Some(done) = current {
        worktrees.push(done);
    }
    if let Some(first) = worktrees.first_mut() {
        first.is_main = true;
    }
    worktrees
}

/// Mengurai `git submodule status`: `<tanda><oid> <path> [(<describe>)]`.
pub fn parse_submodules(text: &str) -> Vec<Submodule> {
    text.lines()
        .filter(|l| l.len() > 2)
        .filter_map(|line| {
            let status = match line.as_bytes()[0] {
                b'-' => SubmoduleStatus::Uninitialized,
                b'+' => SubmoduleStatus::Modified,
                b'U' => SubmoduleStatus::Conflict,
                _ => SubmoduleStatus::UpToDate,
            };
            let rest = &line[1..];
            let (oid, rest) = rest.split_once(' ')?;
            let (path, describe) = match rest.rfind(" (") {
                Some(pos) if rest.ends_with(')') => (&rest[..pos], &rest[pos + 2..rest.len() - 1]),
                _ => (rest, ""),
            };
            Some(Submodule { path: path.to_string(), oid: oid.to_string(), status, describe: describe.to_string() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(action: RebaseAction, oid: &str) -> RebaseTodoItem {
        RebaseTodoItem { action, oid: oid.into(), subject: String::new() }
    }

    #[test]
    fn builds_todo_and_rejects_invalid_plans() {
        let todo = build_todo(&[item(RebaseAction::Pick, "abc1234"), item(RebaseAction::Squash, "def5678"), item(RebaseAction::Drop, "0123abc")]).unwrap();
        assert_eq!(todo, "pick abc1234\nsquash def5678\ndrop 0123abc\n");
        assert!(build_todo(&[item(RebaseAction::Squash, "abc1234")]).is_err());
        assert!(build_todo(&[item(RebaseAction::Drop, "abc1234"), item(RebaseAction::Fixup, "def5678")]).is_err());
        assert!(build_todo(&[item(RebaseAction::Drop, "abc1234")]).is_err());
        assert!(build_todo(&[item(RebaseAction::Pick, "abc; rm -rf")]).is_err());
    }

    #[test]
    fn parses_track() {
        assert_eq!(parse_track("[ahead 2, behind 3]"), (2, 3, false));
        assert_eq!(parse_track("[behind 1]"), (0, 1, false));
        assert_eq!(parse_track("[gone]"), (0, 0, true));
        assert_eq!(parse_track(""), (0, 0, false));
    }

    #[test]
    fn parses_worktree_list() {
        let raw = b"worktree /r/main\0HEAD aaa\0branch refs/heads/main\0\0worktree /r/wt two\0HEAD bbb\0detached\0locked reason\0\0";
        let list = parse_worktrees(raw, "/r/wt two");
        assert_eq!(list.len(), 2);
        assert!(list[0].is_main && !list[0].is_current);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].path, "/r/wt two");
        assert!(list[1].is_current && list[1].locked && list[1].branch.is_none());
    }

    #[test]
    fn parses_submodule_status() {
        let text = " aaa111 vendor/lib (v1.2.0)\n-bbb222 not init\n+ccc333 changed dir (heads/main)\n";
        let subs = parse_submodules(text);
        assert_eq!(subs[0], Submodule { path: "vendor/lib".into(), oid: "aaa111".into(), status: SubmoduleStatus::UpToDate, describe: "v1.2.0".into() });
        assert_eq!((subs[1].path.as_str(), subs[1].status), ("not init", SubmoduleStatus::Uninitialized));
        assert_eq!((subs[2].path.as_str(), subs[2].status), ("changed dir", SubmoduleStatus::Modified));
    }
}
