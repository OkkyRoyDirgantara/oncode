//! Property test for partial staging: for a random selection of changes, the index content after
//! `apply_splice` must equal the independently computed result.

mod common;

use common::TestRepo;
use oncode_git::*;
use proptest::prelude::*;

/// A single edit to the base file: replace `remove` lines starting at `at` with the `insert` lines.
#[derive(Debug, Clone)]
struct Change {
    at: usize,
    remove: usize,
    insert: Vec<String>,
    selected: bool,
}

fn scenario() -> impl Strategy<Value = (Vec<String>, Vec<Change>, bool, bool)> {
    (4usize..24).prop_flat_map(|len| {
        let base: Vec<String> = (0..len).map(|i| format!("line {i}")).collect();
        let changes = prop::collection::vec((0usize..len, 0usize..3, prop::collection::vec("[a-z]{1,6}", 0..3), any::<bool>()), 1..5).prop_map(
            move |raw| {
                // Build changes that do not overlap and are not adjacent, in order.
                let mut changes: Vec<Change> = Vec::new();
                let mut next_free = 0usize;
                let mut sorted = raw;
                sorted.sort_by_key(|r| r.0);
                for (at, remove, insert, selected) in sorted {
                    let at = at.max(next_free);
                    if at >= len {
                        break;
                    }
                    let remove = remove.min(len - at);
                    if remove == 0 && insert.is_empty() {
                        continue;
                    }
                    let insert = insert.into_iter().enumerate().map(|(i, w)| format!("{w}-{at}-{i}")).collect();
                    changes.push(Change { at, remove, insert, selected });
                    next_free = at + remove + 1;
                }
                changes
            },
        );
        (Just(base), changes, any::<bool>(), any::<bool>())
    })
}

/// Applies the selected changes to the base lines (a reference computation, without using the code under test).
fn apply(base: &[String], changes: &[Change], pick: impl Fn(&Change) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0;
    for change in changes {
        out.extend_from_slice(&base[cursor..change.at]);
        if pick(change) {
            out.extend(change.insert.iter().cloned());
        } else {
            out.extend_from_slice(&base[change.at..change.at + change.remove]);
        }
        cursor = change.at + change.remove;
    }
    out.extend_from_slice(&base[cursor..]);
    out
}

fn text(lines: &[String], final_newline: bool) -> String {
    let mut s = lines.join("\n");
    if final_newline && !lines.is_empty() {
        s.push('\n');
    }
    s
}

/// Line range of each change on side A (base) and side B (all changes applied).
fn ops(changes: &[Change], only_selected: bool) -> Vec<SpliceOp> {
    let mut shift: isize = 0;
    let mut out = Vec::new();
    for c in changes {
        let b_from = (c.at as isize + shift) as u32;
        if !only_selected || c.selected {
            out.push(SpliceOp { a_from: c.at as u32, a_to: (c.at + c.remove) as u32, b_from, b_to: b_from + c.insert.len() as u32 });
        }
        shift += c.insert.len() as isize - c.remove as isize;
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn staged_content_matches_independent_model((base, changes, nl_a, nl_b) in scenario()) {
        prop_assume!(!changes.is_empty());
        // Without a trailing newline, the last line must not be touched so the reference model stays simple.
        let touches_end = changes.iter().any(|c| c.at + c.remove >= base.len());
        let _ = nl_b;
        let (nl_a, nl_b) = if touches_end { (true, true) } else { (nl_a, nl_a) };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let t = TestRepo::with_commit().await;
            let original = text(&base, nl_a);
            let modified = text(&apply(&base, &changes, |_| true), nl_b);
            t.commit_file("f.txt", &original, "Base");
            t.write("f.txt", &modified);

            // Stage only the selected changes.
            let d = t.repo.diff_contents("f.txt", None, &DiffSource::Worktree).await.unwrap();
            t.repo.apply_splice(SpliceTarget::Stage, "f.txt", &ops(&changes, true), &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
            let expected = text(&apply(&base, &changes, |c| c.selected), nl_a);
            prop_assert_eq!(t.index("f.txt"), expected.clone());
            prop_assert_eq!(t.read("f.txt"), modified.clone());

            // Unstaging everything again restores the index to the HEAD content.
            let d = t.repo.diff_contents("f.txt", None, &DiffSource::Staged).await.unwrap();
            let staged_only: Vec<Change> = changes.iter().filter(|c| c.selected).cloned().map(|mut c| { c.selected = true; c }).collect();
            // Positions on side B (index) are computed from the changes that were actually staged.
            let mut shift: isize = 0;
            let mut undo = Vec::new();
            for c in &staged_only {
                let b_from = (c.at as isize + shift) as u32;
                undo.push(SpliceOp { a_from: c.at as u32, a_to: (c.at + c.remove) as u32, b_from, b_to: b_from + c.insert.len() as u32 });
                shift += c.insert.len() as isize - c.remove as isize;
            }
            t.repo.apply_splice(SpliceTarget::Unstage, "f.txt", &undo, &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
            prop_assert_eq!(t.index("f.txt"), original.clone());

            // Discarding the selected changes from the working tree leaves the unselected changes.
            let d = t.repo.diff_contents("f.txt", None, &DiffSource::Worktree).await.unwrap();
            t.repo.apply_splice(SpliceTarget::Discard, "f.txt", &ops(&changes, true), &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
            let remaining = text(&apply(&base, &changes, |c| !c.selected), nl_b);
            prop_assert_eq!(t.read("f.txt"), remaining);
            Ok(())
        })?;
    }
}
