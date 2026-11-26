//! Pure logic of the Graph view: the log query from toolbar choices and the commit selection rules.

use oncode_git::{GraphRow, LogQuery, Refs};

/// Scope choice values: `"all"`, `"current"`, or `"ref:<name>"`.
pub fn build_query(scope: &str, grep: &str, author: &str, path: &str) -> LogQuery {
    let mut query = LogQuery::default();
    if scope == "all" {
        query.all = true;
    } else if let Some(name) = scope.strip_prefix("ref:") {
        query.revs = vec![name.to_string()];
    }
    if !grep.trim().is_empty() {
        query.grep = Some(grep.trim().to_string());
    }
    if !author.trim().is_empty() {
        query.author = Some(author.trim().to_string());
    }
    if !path.trim().is_empty() {
        query.path = Some(path.trim().to_string());
    }
    query
}

/// The list is filtered (message, author, or path), so ancestry cannot be inferred from the rows.
pub fn is_filtered(query: &LogQuery) -> bool {
    query.grep.is_some() || query.author.is_some() || query.path.is_some()
}

/// A `ref:<name>` scope still points to an existing ref.
pub fn scope_exists(scope: &str, refs: Option<&Refs>) -> bool {
    let (Some(name), Some(refs)) = (scope.strip_prefix("ref:"), refs) else { return true };
    refs.branches.iter().any(|b| b.name == name) || refs.remote_branches.iter().any(|b| b.name == name) || refs.tags.iter().any(|t| t.name == name)
}

/// The scope label for the toolbar button.
pub fn scope_label(scope: &str) -> String {
    match scope {
        "all" => "All branches".into(),
        "current" => "Current branch".into(),
        other => other.strip_prefix("ref:").unwrap_or(other).to_string(),
    }
}

/// The next selection after a commit is clicked. A plain click selects one commit;
/// Ctrl/Cmd-click adds or removes a second commit (at most two, for comparison).
pub fn next_selection(previous: &[String], oid: &str, additive: bool) -> Vec<String> {
    if !additive {
        return vec![oid.to_string()];
    }
    if previous.iter().any(|o| o == oid) {
        return previous.iter().filter(|o| *o != oid).cloned().collect();
    }
    match previous.first() {
        None => vec![oid.to_string()],
        Some(first) => vec![first.clone(), oid.to_string()],
    }
}

/// Orders the selected commits as [old, .., new] by their position in the list
/// (a lower row = older). If any of them is not loaded yet, the selection order is used.
pub fn order_oldest_first(rows: &[GraphRow], oids: &[String]) -> Vec<String> {
    let mut indexed: Vec<(usize, &String)> = Vec::with_capacity(oids.len());
    for oid in oids {
        match rows.iter().position(|r| &r.commit.oid == oid) {
            Some(index) => indexed.push((index, oid)),
            None => return oids.to_vec(),
        }
    }
    indexed.sort_by(|a, b| b.0.cmp(&a.0));
    indexed.into_iter().map(|(_, oid)| oid.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{branch, linear_rows, oid};
    use oncode_git::{RemoteBranch, Tag};

    #[test]
    fn scope_maps_to_all_head_or_one_ref() {
        assert_eq!(build_query("all", "", "", ""), LogQuery { all: true, ..Default::default() });
        assert_eq!(build_query("current", "", "", ""), LogQuery::default());
        assert_eq!(build_query("ref:feature/login", "", "", ""), LogQuery { revs: vec!["feature/login".into()], ..Default::default() });
    }

    #[test]
    fn filters_are_trimmed_and_only_added_when_given() {
        assert_eq!(build_query("all", "  fix ", " ana ", ""), LogQuery { all: true, grep: Some("fix".into()), author: Some("ana".into()), ..Default::default() });
        assert_eq!(build_query("current", "   ", "", "  "), LogQuery::default());
        assert_eq!(build_query("current", "", "", " src/main.rs "), LogQuery { path: Some("src/main.rs".into()), ..Default::default() });
        assert!(!is_filtered(&build_query("all", "", "", "")));
        assert!(is_filtered(&build_query("all", "x", "", "")));
        assert!(is_filtered(&build_query("all", "", "", "a.txt")));
    }

    #[test]
    fn scope_exists_checks_every_kind_of_ref() {
        let refs = Refs {
            branches: vec![branch("main")],
            remote_branches: vec![RemoteBranch { name: "origin/main".into(), remote: "origin".into(), oid: oid(0) }],
            tags: vec![Tag { name: "v1".into(), oid: oid(0), annotated: false, message: String::new() }],
            remotes: vec![],
        };
        for scope in ["all", "current", "ref:main", "ref:origin/main", "ref:v1"] {
            assert!(scope_exists(scope, Some(&refs)), "{scope}");
        }
        assert!(!scope_exists("ref:gone", Some(&refs)));
        assert!(scope_exists("ref:gone", None), "before refs are loaded, the scope is assumed to exist");
        assert_eq!(scope_label("all"), "All branches");
        assert_eq!(scope_label("current"), "Current branch");
        assert_eq!(scope_label("ref:origin/main"), "origin/main");
    }

    #[test]
    fn selection_rules() {
        let (a, b, c) = (oid(1), oid(2), oid(3));
        // A plain click selects exactly one commit.
        assert_eq!(next_selection(&[], &a, false), [a.clone()]);
        assert_eq!(next_selection(&[a.clone(), b.clone()], &c, false), [c.clone()]);
        assert_eq!(next_selection(&[a.clone()], &a, false), [a.clone()]);
        // Ctrl/Cmd-click adds a second commit.
        assert_eq!(next_selection(&[], &a, true), [a.clone()]);
        assert_eq!(next_selection(&[a.clone()], &b, true), [a.clone(), b.clone()]);
        // Ctrl/Cmd-click on an already selected one deselects it.
        assert_eq!(next_selection(&[a.clone(), b.clone()], &b, true), [a.clone()]);
        assert_eq!(next_selection(&[a.clone(), b.clone()], &a, true), [b.clone()]);
        assert!(next_selection(&[a.clone()], &a, true).is_empty());
        // Never more than two: the third replaces the second.
        assert_eq!(next_selection(&[a.clone(), b], &c, true), [a, c]);
    }

    #[test]
    fn order_oldest_first_follows_list_position() {
        let rows = linear_rows(0, 10); // row 0 = newest
        assert_eq!(order_oldest_first(&rows, &[oid(2), oid(7)]), [oid(7), oid(2)]);
        assert_eq!(order_oldest_first(&rows, &[oid(7), oid(2)]), [oid(7), oid(2)]);
        // A commit that is not loaded yet: the selection order is kept.
        assert_eq!(order_oldest_first(&rows, &[oid(2), oid(99)]), [oid(2), oid(99)]);
    }
}
