//! The interactive rebase plan: validation (same as `build_todo` in the backend) and reordering.

use oncode_git::{RebaseAction, RebaseTodoItem};

pub const REBASE_ACTIONS: [RebaseAction; 6] = [RebaseAction::Pick, RebaseAction::Reword, RebaseAction::Edit, RebaseAction::Squash, RebaseAction::Fixup, RebaseAction::Drop];

pub fn action_hint(action: RebaseAction) -> &'static str {
    match action {
        RebaseAction::Pick => "Keep the commit as it is",
        RebaseAction::Reword => "Keep the changes, edit the message",
        RebaseAction::Edit => "Stop at this commit so you can amend it",
        RebaseAction::Squash => "Combine with the commit above, merging both messages",
        RebaseAction::Fixup => "Combine with the commit above, discarding this message",
        RebaseAction::Drop => "Remove the commit",
    }
}

/// The error message if the plan is invalid, or `None` if it may run.
/// The rules follow the backend: squash/fixup need a preceding pick/reword/edit,
/// and at least one commit must remain.
pub fn validate_plan(items: &[RebaseTodoItem]) -> Option<String> {
    if items.is_empty() {
        return Some("There are no commits to rebase.".into());
    }
    let mut has_base = false;
    for item in items {
        match item.action {
            RebaseAction::Squash | RebaseAction::Fixup if !has_base => {
                return Some(format!(
                    "\"{}\" on {} needs an earlier commit to combine with. Put a pick, reword or edit above it.",
                    item.action.as_str(),
                    crate::format::short_oid(&item.oid)
                ));
            }
            RebaseAction::Pick | RebaseAction::Reword | RebaseAction::Edit => has_base = true,
            _ => {}
        }
    }
    if !has_base {
        return Some("Every commit is dropped; nothing would be left to rebase.".into());
    }
    None
}

/// Moves the item from index `from` to index `to`; out-of-range indexes change nothing.
pub fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from == to || from >= items.len() || to >= items.len() {
        return;
    }
    let moved = items.remove(from);
    items.insert(to, moved);
}

pub fn set_action(items: &mut [RebaseTodoItem], index: usize, action: RebaseAction) {
    if let Some(item) = items.get_mut(index) {
        item.action = action;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::oid;
    use RebaseAction::*;

    fn plan(actions: &[RebaseAction]) -> Vec<RebaseTodoItem> {
        actions.iter().enumerate().map(|(i, action)| RebaseTodoItem { action: *action, oid: oid(i + 1), subject: format!("c{}", i + 1) }).collect()
    }

    // This table mirrors `build_todo` in crates/oncode-git/src/ops.rs:
    // squash/fixup need a preceding pick/reword/edit; at least one commit remains.
    #[test]
    fn accepts_valid_plans() {
        let valid: &[&[RebaseAction]] = &[
            &[Pick],
            &[Pick, Pick, Pick],
            &[Reword],
            &[Edit],
            &[Pick, Squash],
            &[Pick, Fixup, Fixup],
            &[Reword, Squash],
            &[Edit, Fixup],
            &[Drop, Pick],
            &[Drop, Pick, Squash],
            // A dropped commit in between does not remove the base that already exists.
            &[Pick, Drop, Squash],
            &[Pick, Drop, Drop],
            &[Pick, Squash, Drop, Fixup, Reword],
        ];
        for actions in valid {
            assert_eq!(validate_plan(&plan(actions)), None, "{actions:?}");
        }
    }

    #[test]
    fn rejects_invalid_plans() {
        let invalid: &[(&[RebaseAction], &str)] = &[
            (&[], "no commits"),
            (&[Squash], "\"squash\""),
            (&[Fixup], "\"fixup\""),
            (&[Squash, Pick], "\"squash\""),
            (&[Fixup, Pick], "\"fixup\""),
            // Drop does not count as a preceding commit.
            (&[Drop, Squash], "\"squash\""),
            (&[Drop, Drop, Fixup, Pick], "\"fixup\""),
            (&[Drop], "Every commit is dropped"),
            (&[Drop, Drop, Drop], "Every commit is dropped"),
        ];
        for (actions, expected) in invalid {
            let message = validate_plan(&plan(actions)).unwrap_or_else(|| panic!("{actions:?} should be rejected"));
            assert!(message.contains(expected), "{actions:?}: {message}");
            if expected.starts_with('"') {
                assert!(message.contains("needs an earlier commit"), "{message}");
            }
        }
    }

    #[test]
    fn names_the_offending_commit() {
        let message = validate_plan(&plan(&[Drop, Squash])).unwrap();
        assert!(message.contains(&oid(2)[..8]), "{message}");
    }

    #[test]
    fn offers_exactly_the_six_git_actions() {
        assert_eq!(REBASE_ACTIONS.map(|a| a.as_str()), ["pick", "reword", "edit", "squash", "fixup", "drop"]);
        for action in REBASE_ACTIONS {
            assert!(!action_hint(action).is_empty());
        }
    }

    #[test]
    fn move_item_reorders() {
        for (from, to, expected) in [(0, 1, "bacd"), (0, 3, "bcda"), (3, 0, "dabc"), (2, 1, "acbd"), (1, 1, "abcd")] {
            let mut items = vec!['a', 'b', 'c', 'd'];
            move_item(&mut items, from, to);
            assert_eq!(items.iter().collect::<String>(), expected, "{from} → {to}");
        }
    }

    #[test]
    fn move_item_ignores_indexes_outside_the_list() {
        for (from, to) in [(0, 4), (9, 0), (4, 4)] {
            let mut items = vec!['a', 'b', 'c', 'd'];
            move_item(&mut items, from, to);
            assert_eq!(items, ['a', 'b', 'c', 'd']);
        }
    }

    #[test]
    fn set_action_changes_one_item() {
        let mut items = plan(&[Pick, Pick, Pick]);
        let oids: Vec<String> = items.iter().map(|i| i.oid.clone()).collect();
        set_action(&mut items, 1, Squash);
        set_action(&mut items, 9, Drop);
        assert_eq!(items.iter().map(|i| i.action).collect::<Vec<_>>(), [Pick, Squash, Pick]);
        assert_eq!(items.iter().map(|i| i.oid.clone()).collect::<Vec<_>>(), oids);
    }
}
