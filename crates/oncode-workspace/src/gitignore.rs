//! The gitignore rules shared by the explorer, watcher, search, and index.
//!
//! A single source of truth: [`IgnoreRules`]. The explorer and watcher ask per path, while
//! tree walks (search, index) use `ignore::WalkBuilder` filtered with the same
//! rules through [`walk_builder`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::{Match, WalkBuilder};

/// The root of the repository enclosing `dir` (the nearest folder that has `.git`), if any.
pub(crate) fn find_repo_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".git").symlink_metadata().is_ok()).map(Path::to_path_buf)
}

/// The shared Git directory for a worktree's `.git`.
///
/// `.git` can be a folder (a regular repository) or a pointer file `gitdir: …`
/// (linked worktrees and submodules).
pub(crate) fn common_dir_of_dot_git(dot_git: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    let text = std::fs::read_to_string(dot_git).ok()?;
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    let git_dir = dot_git.parent()?.join(target);
    Some(read_common_dir(&git_dir).unwrap_or(git_dir))
}

/// The contents of `<git_dir>/commondir` as an absolute path; `None` if it is not a linked worktree.
pub(crate) fn read_common_dir(git_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    Some(git_dir.join(line))
}

/// A tree walker that uses [`IgnoreRules`]: hidden files are included, `.git` and ignored
/// entries are skipped, symlinks are not followed.
pub(crate) fn walk_builder(root: &Path) -> WalkBuilder {
    let rules = Arc::new(IgnoreRules::new(root));
    let mut builder = WalkBuilder::new(root);
    builder.standard_filters(false).follow_links(false).filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        if entry.file_name() == ".git" {
            return false;
        }
        let path = entry.path();
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        match path.parent() {
            Some(parent) => !rules.is_entry_ignored(parent, path, is_dir),
            None => true,
        }
    });
    builder
}

/// The rules that apply to the direct children of a folder, already sorted by priority.
struct Chain {
    /// `.gitignore` files from this folder upward to the nearest repository root.
    gitignores: Vec<Arc<Gitignore>>,
    /// The nearest repository's `.git/info/exclude`.
    exclude: Option<Arc<Gitignore>>,
    /// The nearest repository root; the base for matching the global gitignore.
    base: PathBuf,
}

/// Answers whether a path is ignored by Git.
///
/// The order follows Git: `.gitignore` from the nearest folder up to the repository root, then
/// `.git/info/exclude`, then the user's global gitignore. A nested repository uses its own
/// rules. If `root` is not inside a repository, `.gitignore` files from `root` downward
/// are still honored, but those above `root` are not.
pub(crate) struct IgnoreRules {
    root: PathBuf,
    /// The topmost folder whose `.gitignore` still applies: the repository root enclosing
    /// `root`, or `root` itself if it is not inside a repository.
    top: PathBuf,
    global: Gitignore,
    chains: RwLock<HashMap<PathBuf, Arc<Chain>>>,
}

impl IgnoreRules {
    /// `root` must be absolute; the paths queried must use the same prefix.
    pub(crate) fn new(root: &Path) -> Self {
        // Unit tests must not depend on the global gitignore of the machine running them.
        let global = if cfg!(test) { Gitignore::empty() } else { Gitignore::global().0 };
        Self::with_global(root, global)
    }

    /// Like [`IgnoreRules::new`], with the given global gitignore (`core.excludesFile`).
    pub(crate) fn with_global(root: &Path, global: Gitignore) -> Self {
        let top = find_repo_root(root).unwrap_or_else(|| root.to_path_buf());
        Self { root: root.to_path_buf(), top, global, chains: RwLock::new(HashMap::new()) }
    }

    /// Discards the rules that have been loaded; called when a `.gitignore` changes.
    pub(crate) fn invalidate(&self) {
        if let Ok(mut chains) = self.chains.write() {
            chains.clear();
        }
    }

    /// True if `path` or one of the folders above it (up to `root`) is ignored.
    pub(crate) fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else { return false };
        let mut current = self.root.clone();
        let mut components = rel.components().peekable();
        while let Some(component) = components.next() {
            let parent = current.clone();
            current.push(component);
            let dir = components.peek().is_some() || is_dir;
            if self.is_entry_ignored(&parent, &current, dir) {
                return true;
            }
        }
        false
    }

    /// True if `path` (a direct child of `parent`) is ignored, assuming `parent` itself is not.
    pub(crate) fn is_entry_ignored(&self, parent: &Path, path: &Path, is_dir: bool) -> bool {
        let chain = self.chain_for(parent);
        for gitignore in chain.gitignores.iter().chain(chain.exclude.iter()) {
            match gitignore.matched(path, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        if self.global.is_empty() {
            return false;
        }
        let rel = path.strip_prefix(&chain.base).unwrap_or(path);
        matches!(self.global.matched(rel, is_dir), Match::Ignore(_))
    }

    fn chain_for(&self, dir: &Path) -> Arc<Chain> {
        if let Ok(chains) = self.chains.read() {
            if let Some(chain) = chains.get(dir) {
                return Arc::clone(chain);
            }
        }
        let own = load_file(dir, &dir.join(".gitignore"));
        let dot_git = dir.join(".git");
        let has_git = dot_git.symlink_metadata().is_ok();
        let parent = if has_git || dir == self.top { None } else { dir.parent() };
        let chain = match parent {
            // A regular folder: its own rules, then the rules of the folder above it.
            Some(parent) => {
                let above = self.chain_for(parent);
                let mut gitignores = Vec::with_capacity(above.gitignores.len() + 1);
                gitignores.extend(own);
                gitignores.extend(above.gitignores.iter().cloned());
                Chain { gitignores, exclude: above.exclude.clone(), base: above.base.clone() }
            }
            // The repository root (or the upper bound): the chain stops here.
            None => {
                let exclude = if has_git {
                    common_dir_of_dot_git(&dot_git).and_then(|common| load_file(dir, &common.join("info").join("exclude")))
                } else {
                    None
                };
                Chain { gitignores: own.into_iter().collect(), exclude, base: dir.to_path_buf() }
            }
        };
        let chain = Arc::new(chain);
        if let Ok(mut chains) = self.chains.write() {
            chains.insert(dir.to_path_buf(), Arc::clone(&chain));
        }
        chain
    }
}

/// Loads a single pattern file with `dir` as the base for anchored patterns. Invalid lines
/// are skipped; `None` if the file does not exist or contains no patterns.
fn load_file(dir: &Path, file: &Path) -> Option<Arc<Gitignore>> {
    if !file.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(dir);
    let _ = builder.add(file);
    let gitignore = builder.build().ok()?;
    if gitignore.is_empty() {
        return None;
    }
    Some(Arc::new(gitignore))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// All files that are not ignored according to `IgnoreRules`, relative to `root`.
    fn walk_with_rules(root: &Path) -> BTreeSet<String> {
        let rules = IgnoreRules::new(root);
        let mut out = BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name() == ".git" {
                    continue;
                }
                let path = entry.path();
                let is_dir = entry.file_type().unwrap().is_dir();
                assert_eq!(
                    rules.is_entry_ignored(&dir, &path, is_dir),
                    rules.is_ignored(&path, is_dir),
                    "{}",
                    path.display()
                );
                if rules.is_ignored(&path, is_dir) {
                    continue;
                }
                if is_dir {
                    stack.push(path);
                } else {
                    out.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        out
    }

    fn walk_with_builder(root: &Path) -> BTreeSet<String> {
        walk_builder(root)
            .build()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| e.path().strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect()
    }

    fn sample_tree(root: &Path) {
        write(&root.join(".gitignore"), "*.log\n/build\nnode_modules/\n!keep.log\n");
        write(&root.join("a.txt"), "");
        write(&root.join("a.log"), "");
        write(&root.join("keep.log"), "");
        write(&root.join("build/out.bin"), "");
        write(&root.join("src/build/kept.rs"), "");
        write(&root.join("src/main.rs"), "");
        write(&root.join("src/.gitignore"), "/generated\n!debug.log\n");
        write(&root.join("src/generated/x.rs"), "");
        write(&root.join("src/debug.log"), "");
        write(&root.join("src/deep/generated/y.rs"), "");
        write(&root.join("src/deep/z.log"), "");
        write(&root.join("web/node_modules/pkg/index.js"), "");
        write(&root.join("web/app.js"), "");
        write(&root.join(".hidden/file"), "");
    }

    const EXPECTED: &[&str] = &[
        ".gitignore",
        ".hidden/file",
        "a.txt",
        "keep.log",
        "src/.gitignore",
        "src/build/kept.rs",
        "src/debug.log",
        "src/deep/generated/y.rs",
        "src/main.rs",
        "web/app.js",
    ];

    #[test]
    fn rules_and_walker_agree_without_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        sample_tree(&root);
        let expected: BTreeSet<String> = EXPECTED.iter().map(|s| s.to_string()).collect();
        assert_eq!(walk_with_rules(&root), expected);
        assert_eq!(walk_with_builder(&root), expected);
    }

    #[test]
    fn rules_and_walker_agree_inside_repo_with_exclude() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        sample_tree(&root);
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join(".git/info/exclude"), "a.txt\n/web/app.js\n");
        let expected: BTreeSet<String> =
            EXPECTED.iter().filter(|s| **s != "a.txt" && **s != "web/app.js").map(|s| s.to_string()).collect();
        assert_eq!(walk_with_rules(&root), expected);
        assert_eq!(walk_with_builder(&root), expected);
    }

    #[test]
    fn parent_gitignore_applies_only_inside_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap();
        write(&top.join(".gitignore"), "*.tmp\n");
        write(&top.join("proj/x.tmp"), "");
        write(&top.join("proj/y.rs"), "");
        let root = top.join("proj");

        let all: BTreeSet<String> = ["x.tmp", "y.rs"].iter().map(|s| s.to_string()).collect();
        assert_eq!(walk_with_rules(&root), all);
        assert_eq!(walk_with_builder(&root), all);

        write(&top.join(".git/HEAD"), "ref: refs/heads/main\n");
        let filtered: BTreeSet<String> = ["y.rs"].iter().map(|s| s.to_string()).collect();
        assert_eq!(walk_with_rules(&root), filtered);
        assert_eq!(walk_with_builder(&root), filtered);
    }

    #[test]
    fn nested_repo_uses_its_own_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join(".git/HEAD"), "");
        write(&root.join(".gitignore"), "*.gen\n");
        write(&root.join("a.gen"), "");
        write(&root.join("sub/.git/HEAD"), "");
        write(&root.join("sub/b.gen"), "");
        write(&root.join("sub/c.txt"), "");
        let expected: BTreeSet<String> = [".gitignore", "sub/b.gen", "sub/c.txt"].iter().map(|s| s.to_string()).collect();
        assert_eq!(walk_with_rules(&root), expected);
        assert_eq!(walk_with_builder(&root), expected);
    }

    #[test]
    fn everything_under_an_ignored_folder_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        sample_tree(&root);
        let rules = IgnoreRules::new(&root);
        assert!(rules.is_ignored(&root.join("web/node_modules/pkg/index.js"), false));
        assert!(rules.is_ignored(&root.join("build"), true));
        assert!(!rules.is_ignored(&root.join("src/build"), true));
        // The `node_modules/` pattern only matches folders.
        assert!(!rules.is_ignored(&root.join("node_modules"), false));
        assert!(rules.is_ignored(&root.join("node_modules"), true));
    }

    #[test]
    fn global_gitignore_has_the_lowest_priority() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join(".git/HEAD"), "");
        write(&root.join(".gitignore"), "!keep.swp
");
        write(&root.join("sub/.git/HEAD"), "");
        let mut builder = GitignoreBuilder::new("");
        builder.add_line(None, "*.swp").unwrap();
        builder.add_line(None, "/scratch").unwrap();
        let rules = IgnoreRules::with_global(&root, builder.build().unwrap());

        assert!(rules.is_ignored(&root.join("a.swp"), false));
        assert!(rules.is_ignored(&root.join("deep/dir/a.swp"), false));
        assert!(!rules.is_ignored(&root.join("keep.swp"), false));
        assert!(!rules.is_ignored(&root.join("a.txt"), false));
        // Anchored patterns are matched from the nearest repository root.
        assert!(rules.is_ignored(&root.join("scratch"), true));
        assert!(!rules.is_ignored(&root.join("deep/scratch"), true));
        assert!(rules.is_ignored(&root.join("sub/scratch"), true));
        assert!(rules.is_ignored(&root.join("sub/keep.swp"), false));
    }

    #[test]
    fn invalidate_reloads_changed_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join(".gitignore"), "a\n");
        let rules = IgnoreRules::new(&root);
        assert!(rules.is_ignored(&root.join("a"), false));
        write(&root.join(".gitignore"), "b\n");
        assert!(rules.is_ignored(&root.join("a"), false));
        rules.invalidate();
        assert!(!rules.is_ignored(&root.join("a"), false));
        assert!(rules.is_ignored(&root.join("b"), false));
    }
}
