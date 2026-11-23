//! Penyaringan dan pengelompokan ref untuk panel Referensi, serta ringkasan lencana ref pada baris commit.

use std::collections::{HashMap, HashSet};

use oncode_git::{Branch, GraphRow, RefLabel, Refs, RemoteBranch, Stash, Tag};

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteGroup {
    pub remote: String,
    pub branches: Vec<RemoteBranch>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RefGroups {
    pub branches: Vec<Branch>,
    pub remotes: Vec<RemoteGroup>,
    pub tags: Vec<Tag>,
    pub stashes: Vec<Stash>,
}

impl RefGroups {
    pub fn remote_branch_count(&self) -> usize {
        self.remotes.iter().map(|g| g.branches.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.branches.is_empty() && self.remote_branch_count() == 0 && self.tags.is_empty() && self.stashes.is_empty()
    }
}

/// Nama branch remote tanpa awalan nama remote, mis. `origin/feature/x` → `feature/x`.
pub fn remote_short_name(branch: &RemoteBranch) -> &str {
    branch.name.strip_prefix(&branch.remote).and_then(|rest| rest.strip_prefix('/')).unwrap_or(&branch.name)
}

/// Ref yang cocok dengan `filter` (substring, tidak peka huruf besar), dikelompokkan untuk ditampilkan.
/// Branch remote dikelompokkan per remote mengikuti urutan `refs.remotes`; `<remote>/HEAD` dibuang.
pub fn group_refs(refs: Option<&Refs>, stashes: &[Stash], filter: &str) -> RefGroups {
    let needle = filter.trim().to_lowercase();
    let matches = |texts: &[&str]| needle.is_empty() || texts.iter().any(|t| t.to_lowercase().contains(&needle));

    let mut groups: Vec<RemoteGroup> = Vec::new();
    if let Some(refs) = refs {
        for remote in &refs.remotes {
            groups.push(RemoteGroup { remote: remote.name.clone(), branches: Vec::new() });
        }
        for branch in &refs.remote_branches {
            if remote_short_name(branch) == "HEAD" || !matches(&[&branch.name]) {
                continue;
            }
            match groups.iter_mut().find(|g| g.remote == branch.remote) {
                Some(group) => group.branches.push(branch.clone()),
                None => groups.push(RemoteGroup { remote: branch.remote.clone(), branches: vec![branch.clone()] }),
            }
        }
    }
    // Saat menyaring, remote tanpa branch yang cocok disembunyikan.
    groups.retain(|g| !g.branches.is_empty() || needle.is_empty());

    RefGroups {
        branches: refs.map(|r| r.branches.iter().filter(|b| matches(&[&b.name])).cloned().collect()).unwrap_or_default(),
        remotes: groups,
        tags: refs.map(|r| r.tags.iter().filter(|t| matches(&[&t.name])).cloned().collect()).unwrap_or_default(),
        stashes: stashes.iter().filter(|s| matches(&[&s.name, &s.message])).cloned().collect(),
    }
}

/// Satu simpul pohon nama ref yang dipisah `/` (mis. `feature/login` di bawah folder `feature`).
#[derive(Debug, Clone, PartialEq)]
pub enum NameNode {
    /// Folder: nama segmen, path lengkapnya (untuk status buka/tutup), dan isinya.
    Folder { name: String, path: String, children: Vec<NameNode> },
    /// Daun: nama segmen terakhir dan indeks item aslinya.
    Leaf { name: String, index: usize },
}

/// Menyusun nama-nama ref menjadi pohon menurut `/`. Urutan kemunculan pertama dipertahankan.
pub fn name_tree<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<NameNode> {
    fn insert(nodes: &mut Vec<NameNode>, prefix: &str, segments: &[&str], index: usize) {
        if segments.len() == 1 {
            nodes.push(NameNode::Leaf { name: segments[0].to_string(), index });
            return;
        }
        let path = if prefix.is_empty() { segments[0].to_string() } else { format!("{prefix}/{}", segments[0]) };
        let position = nodes.iter().position(|n| matches!(n, NameNode::Folder { path: p, .. } if *p == path));
        let position = position.unwrap_or_else(|| {
            nodes.push(NameNode::Folder { name: segments[0].to_string(), path: path.clone(), children: Vec::new() });
            nodes.len() - 1
        });
        if let NameNode::Folder { children, .. } = &mut nodes[position] {
            insert(children, &path, &segments[1..], index);
        }
    }
    let mut roots = Vec::new();
    for (index, name) in names.into_iter().enumerate() {
        let segments: Vec<&str> = name.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            roots.push(NameNode::Leaf { name: name.to_string(), index });
        } else {
            insert(&mut roots, "", &segments, index);
        }
    }
    roots
}

/// Membagi lencana ref menjadi yang ditampilkan dan yang diringkas sebagai "+N".
pub fn summarize_labels(labels: &[RefLabel], max: usize) -> (&[RefLabel], &[RefLabel]) {
    if labels.len() <= max {
        (labels, &[])
    } else {
        labels.split_at(max)
    }
}

/// Apakah `oid` leluhur dari `head_oid`, menurut baris yang sudah dimuat.
/// Urutan topologis menjamin semua commit di antara HEAD dan leluhurnya muncul lebih dulu,
/// sehingga jawabannya pasti selama daftar tidak disaring. Mengembalikan `None` bila tidak bisa
/// ditentukan (HEAD tidak muncul sebelum commit itu, atau commit tidak ada di daftar).
pub fn is_ancestor_of_head(rows: &[GraphRow], head_oid: Option<&str>, oid: &str) -> Option<bool> {
    let head_oid = head_oid?;
    if oid == head_oid {
        return Some(true);
    }
    let mut parents: HashMap<&str, &[String]> = HashMap::new();
    let (mut seen_head, mut seen_target) = (false, false);
    for row in rows {
        parents.insert(&row.commit.oid, &row.commit.parents);
        if row.commit.oid == head_oid {
            seen_head = true;
        }
        if row.commit.oid == oid {
            seen_target = true;
            break;
        }
    }
    if !seen_target || !seen_head {
        return None;
    }
    let mut queue = vec![head_oid];
    let mut visited: HashSet<&str> = queue.iter().copied().collect();
    while let Some(current) = queue.pop() {
        for parent in parents.get(current).copied().unwrap_or_default() {
            if parent == oid {
                return Some(true);
            }
            if visited.insert(parent) {
                queue.push(parent);
            }
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{branch, oid, row};
    use oncode_git::{RefKind, Remote};

    fn remote_branch(name: &str, remote: &str, n: usize) -> RemoteBranch {
        RemoteBranch { name: name.into(), remote: remote.into(), oid: oid(n) }
    }

    fn refs() -> Refs {
        let mut main = branch("main");
        main.is_head = true;
        Refs {
            branches: vec![main, branch("feature/login"), branch("fix/typo")],
            remote_branches: vec![
                remote_branch("origin/HEAD", "origin", 0),
                remote_branch("origin/main", "origin", 0),
                remote_branch("origin/feature/login", "origin", 1),
                remote_branch("upstream/main", "upstream", 2),
                remote_branch("fork/wip", "fork", 3),
            ],
            tags: vec![
                Tag { name: "v1.0.0".into(), oid: oid(4), annotated: true, message: "First release".into() },
                Tag { name: "v1.1.0-login".into(), oid: oid(5), annotated: false, message: String::new() },
            ],
            remotes: ["upstream", "origin", "empty"].iter().map(|n| Remote { name: n.to_string(), fetch_url: n.to_string(), push_url: n.to_string() }).collect(),
        }
    }

    fn stashes() -> Vec<Stash> {
        vec![
            Stash { index: 0, name: "stash@{0}".into(), oid: oid(6), message: "WIP on main: login form".into(), time: 0.0 },
            Stash { index: 1, name: "stash@{1}".into(), oid: oid(7), message: "On fix/typo: spelling".into(), time: 0.0 },
        ]
    }

    fn names(branches: &[Branch]) -> Vec<&str> {
        branches.iter().map(|b| b.name.as_str()).collect()
    }

    fn remotes(groups: &RefGroups) -> Vec<&str> {
        groups.remotes.iter().map(|g| g.remote.as_str()).collect()
    }

    #[test]
    fn without_a_filter_everything_is_returned_grouped_by_remote_in_configured_order() {
        let groups = group_refs(Some(&refs()), &stashes(), "");
        assert_eq!(names(&groups.branches), ["main", "feature/login", "fix/typo"]);
        // Urutan mengikuti `refs.remotes`; remote yang tidak terdaftar (fork) menyusul di belakang.
        assert_eq!(remotes(&groups), ["upstream", "origin", "empty", "fork"]);
        let origin = groups.remotes.iter().find(|g| g.remote == "origin").unwrap();
        assert_eq!(origin.branches.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["origin/main", "origin/feature/login"]);
        assert_eq!(groups.tags.len(), 2);
        assert_eq!(groups.stashes.len(), 2);
        assert_eq!(groups.remote_branch_count(), 4);
        assert!(!groups.is_empty());
    }

    #[test]
    fn remote_head_is_never_listed() {
        let groups = group_refs(Some(&refs()), &stashes(), "");
        assert!(!groups.remotes.iter().flat_map(|g| &g.branches).any(|b| b.name == "origin/HEAD"));
    }

    #[test]
    fn every_group_is_filtered_by_case_insensitive_substring() {
        let groups = group_refs(Some(&refs()), &stashes(), "  LOGIN ");
        assert_eq!(names(&groups.branches), ["feature/login"]);
        assert_eq!(groups.remotes, [RemoteGroup { remote: "origin".into(), branches: vec![remote_branch("origin/feature/login", "origin", 1)] }]);
        assert_eq!(groups.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["v1.1.0-login"]);
        // Stash dicocokkan lewat pesannya juga.
        assert_eq!(groups.stashes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["stash@{0}"]);
    }

    #[test]
    fn remote_name_and_stash_ref_name_match_too() {
        assert_eq!(remotes(&group_refs(Some(&refs()), &stashes(), "upstream")), ["upstream"]);
        assert_eq!(group_refs(Some(&refs()), &stashes(), "stash@{1}").stashes.iter().map(|s| s.oid.clone()).collect::<Vec<_>>(), [oid(7)]);
    }

    #[test]
    fn remotes_without_matches_are_hidden_only_while_filtering() {
        assert!(remotes(&group_refs(Some(&refs()), &stashes(), "")).contains(&"empty"));
        assert_eq!(remotes(&group_refs(Some(&refs()), &stashes(), "main")), ["upstream", "origin"]);
    }

    #[test]
    fn nothing_matches_or_refs_not_loaded() {
        assert!(group_refs(Some(&refs()), &stashes(), "zzz").is_empty());
        assert_eq!(group_refs(None, &[], ""), RefGroups::default());
    }

    #[test]
    fn remote_short_name_strips_only_the_remote_prefix() {
        assert_eq!(remote_short_name(&remote_branch("origin/feature/login", "origin", 0)), "feature/login");
        assert_eq!(remote_short_name(&remote_branch("my/remote/main", "my/remote", 0)), "main");
        assert_eq!(remote_short_name(&remote_branch("odd", "origin", 0)), "odd");
    }

    #[test]
    fn name_tree_groups_by_slash_and_keeps_order() {
        let tree = name_tree(["main", "feature/login", "fix/typo", "feature/ui/button", "feature/api"]);
        let leaf = |name: &str, index: usize| NameNode::Leaf { name: name.into(), index };
        assert_eq!(
            tree,
            [
                leaf("main", 0),
                NameNode::Folder {
                    name: "feature".into(),
                    path: "feature".into(),
                    children: vec![
                        leaf("login", 1),
                        NameNode::Folder { name: "ui".into(), path: "feature/ui".into(), children: vec![leaf("button", 3)] },
                        leaf("api", 4),
                    ],
                },
                NameNode::Folder { name: "fix".into(), path: "fix".into(), children: vec![leaf("typo", 2)] },
            ]
        );
    }

    #[test]
    fn summarize_labels_keeps_the_first_and_hides_the_rest() {
        let label = |kind, name: &str| RefLabel { kind, name: name.into() };
        let labels = [label(RefKind::Head, "HEAD"), label(RefKind::Branch, "main"), label(RefKind::Remote, "origin/main"), label(RefKind::Tag, "v1"), label(RefKind::Tag, "v2")];
        let (visible, hidden) = summarize_labels(&labels[..3], 3);
        assert_eq!((visible.len(), hidden.len()), (3, 0));
        let (visible, hidden) = summarize_labels(&labels, 3);
        assert_eq!(visible.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["HEAD", "main", "origin/main"]);
        assert_eq!(hidden.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["v1", "v2"]);
    }

    // Riwayat (terbaru di atas):  side(1) ─┐
    //                             head(2)  │   head → 3 → 4 ; side → 4
    fn history() -> Vec<GraphRow> {
        vec![row(1, &[4]), row(2, &[3]), row(3, &[4]), row(4, &[5]), row(5, &[])]
    }

    #[test]
    fn ancestor_is_true_for_head_and_commits_reachable_through_parents() {
        let rows = history();
        assert_eq!(is_ancestor_of_head(&rows, Some(&oid(2)), &oid(2)), Some(true));
        assert_eq!(is_ancestor_of_head(&rows, Some(&oid(2)), &oid(3)), Some(true));
        assert_eq!(is_ancestor_of_head(&rows, Some(&oid(2)), &oid(5)), Some(true));
    }

    #[test]
    fn ancestor_follows_every_parent_of_a_merge() {
        let mut merged = vec![row(0, &[2, 1])];
        merged.extend(history());
        assert_eq!(is_ancestor_of_head(&merged, Some(&oid(0)), &oid(1)), Some(true));
    }

    #[test]
    fn ancestor_is_false_for_another_branch_listed_after_head() {
        let other = vec![row(2, &[3]), row(1, &[4]), row(3, &[4]), row(4, &[])];
        assert_eq!(is_ancestor_of_head(&other, Some(&oid(2)), &oid(1)), Some(false));
    }

    #[test]
    fn ancestor_is_unknown_when_it_cannot_be_decided() {
        let rows = history();
        assert_eq!(is_ancestor_of_head(&rows, Some(&oid(2)), &oid(1)), None);
        assert_eq!(is_ancestor_of_head(&rows, Some(&oid(9)), &oid(3)), None);
        assert_eq!(is_ancestor_of_head(&rows, Some(&oid(2)), &oid(8)), None);
        assert_eq!(is_ancestor_of_head(&rows, None, &oid(3)), None);
    }
}
