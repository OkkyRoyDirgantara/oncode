//! Logika murni panel Problems: saring, kelompokkan per berkas, dan urutkan.

use std::collections::HashSet;

use crate::diagnostics::{Problem, ProblemCounts, Severity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemFilter {
    pub text: String,
    pub errors: bool,
    pub warnings: bool,
    /// Mencakup `info` dan `hint`.
    pub infos: bool,
}

impl Default for ProblemFilter {
    fn default() -> Self {
        Self { text: String::new(), errors: true, warnings: true, infos: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemGroup {
    /// Path absolut.
    pub path: String,
    /// Path relatif terhadap folder proyek (dengan garis miring), untuk ditampilkan.
    pub relative: String,
    pub name: String,
    pub dir: String,
    pub counts: ProblemCounts,
    pub items: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemRow {
    /// Judul berkas: indeks kelompoknya.
    File(usize),
    /// Satu masalah: indeks kelompok dan indeks di dalam kelompok.
    Problem(usize, usize),
}

fn count(items: &[Problem]) -> ProblemCounts {
    let mut counts = ProblemCounts::default();
    for item in items {
        counts.add(item.severity);
    }
    counts
}

/// Jumlah seluruh masalah, tanpa penyaring.
pub fn count_problems<'a>(files: impl IntoIterator<Item = &'a [Problem]>) -> ProblemCounts {
    let mut total = ProblemCounts::default();
    for items in files {
        for item in items {
            total.add(item.severity);
        }
    }
    total
}

fn severity_shown(severity: Severity, filter: &ProblemFilter) -> bool {
    match severity {
        Severity::Error => filter.errors,
        Severity::Warning => filter.warnings,
        Severity::Info | Severity::Hint => filter.infos,
    }
}

fn matches_text(item: &Problem, needle: &str) -> bool {
    item.message.to_lowercase().contains(needle)
        || item.source.as_ref().is_some_and(|source| source.to_lowercase().contains(needle))
        || item.code.as_ref().is_some_and(|code| code.to_lowercase().contains(needle))
}

/// Path relatif terhadap folder proyek; path di luar folder dikembalikan apa adanya.
pub fn relative_path(root: Option<&str>, path: &str) -> String {
    let Some(root) = root.map(|root| root.trim_end_matches(['/', '\\'])).filter(|root| !root.is_empty()) else { return path.to_string() };
    match path.strip_prefix(root) {
        Some(rest) if rest.starts_with(['/', '\\']) => rest[1..].to_string(),
        _ => path.to_string(),
    }
}

/// Kelompok per berkas. Di dalam berkas: severity (galat dulu) lalu posisi. Antarberkas: yang punya
/// galat dulu, lalu peringatan, lalu menurut path. Teks penyaring dicocokkan dengan pesan, sumber,
/// kode, atau path berkas (bila path cocok, semua masalah berkas itu lolos).
pub fn build_groups<'a>(files: impl IntoIterator<Item = (&'a str, &'a [Problem])>, filter: &ProblemFilter, relative: impl Fn(&str) -> String) -> Vec<ProblemGroup> {
    let needle = filter.text.trim().to_lowercase();
    let mut groups: Vec<ProblemGroup> = Vec::new();
    for (path, all) in files {
        let shown = relative(path).replace('\\', "/");
        let path_matches = !needle.is_empty() && shown.to_lowercase().contains(&needle);
        let mut items: Vec<Problem> =
            all.iter().filter(|item| severity_shown(item.severity, filter) && (needle.is_empty() || path_matches || matches_text(item, &needle))).cloned().collect();
        if items.is_empty() {
            continue;
        }
        items.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.line.cmp(&b.line)).then(a.column.cmp(&b.column)));
        let (dir, name) = match shown.rfind('/') {
            Some(slash) => (if slash > 0 { shown[..slash].to_string() } else { String::new() }, shown[slash + 1..].to_string()),
            None => (String::new(), shown.clone()),
        };
        groups.push(ProblemGroup { path: path.to_string(), relative: shown, name, dir, counts: count(&items), items });
    }
    let rank = |group: &ProblemGroup| if group.counts.errors > 0 { 0 } else if group.counts.warnings > 0 { 1 } else { 2 };
    groups.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.relative.to_lowercase().cmp(&b.relative.to_lowercase())).then_with(|| a.relative.cmp(&b.relative)));
    groups
}

/// Baris datar untuk daftar yang divirtualisasi; berkas yang dilipat hanya menyisakan judulnya.
pub fn flatten_rows(groups: &[ProblemGroup], collapsed: &HashSet<String>) -> Vec<ProblemRow> {
    let mut rows = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        rows.push(ProblemRow::File(index));
        if collapsed.contains(&group.path) {
            continue;
        }
        rows.extend((0..group.items.len()).map(|item| ProblemRow::Problem(index, item)));
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn d(severity: Severity, line: u32, column: u32, message: &str) -> Problem {
        Problem { line, column, end_line: line, end_column: column + 1, severity, message: message.into(), source: None, code: None }
    }

    fn with(mut problem: Problem, source: &str, code: Option<&str>) -> Problem {
        problem.source = Some(source.into());
        problem.code = code.map(String::from);
        problem
    }

    fn relative(path: &str) -> String {
        path.strip_prefix("/proj/").unwrap_or(path).to_string()
    }

    fn files() -> BTreeMap<String, Vec<Problem>> {
        use Severity::*;
        BTreeMap::from([
            ("/proj/src/warn_only.rs".to_string(), vec![with(d(Warning, 3, 0, "unused import"), "rustc", Some("unused_imports"))]),
            (
                "/proj/src/main.rs".to_string(),
                vec![
                    d(Warning, 1, 0, "unused variable"),
                    with(d(Error, 20, 4, "mismatched types"), "rustc", Some("E0308")),
                    d(Hint, 2, 0, "consider borrowing"),
                    d(Error, 5, 9, "cannot find value"),
                    d(Error, 5, 2, "expected `;`"),
                    d(Info, 1, 0, "note"),
                ],
            ),
            ("/proj/a/info_only.ts".to_string(), vec![with(d(Info, 1, 0, "todo comment"), "eslint", None)]),
            ("/proj/lib/errors.go".to_string(), vec![with(d(Error, 7, 1, "undefined: foo"), "compiler", None)]),
        ])
    }

    fn groups(files: &BTreeMap<String, Vec<Problem>>, filter: &ProblemFilter) -> Vec<ProblemGroup> {
        build_groups(files.iter().map(|(path, items)| (path.as_str(), items.as_slice())), filter, relative)
    }

    fn relatives(groups: &[ProblemGroup]) -> Vec<&str> {
        groups.iter().map(|group| group.relative.as_str()).collect()
    }

    #[test]
    fn counts_errors_warnings_and_infos_with_hints_as_infos() {
        let files = files();
        assert_eq!(count_problems(files.values().map(Vec::as_slice)), ProblemCounts { errors: 4, warnings: 2, infos: 3 });
        assert_eq!(count_problems(std::iter::empty()), ProblemCounts::default());
    }

    #[test]
    fn files_with_errors_first_then_warnings_then_the_rest_by_path() {
        let files = files();
        let groups = groups(&files, &ProblemFilter::default());
        assert_eq!(relatives(&groups), ["lib/errors.go", "src/main.rs", "src/warn_only.rs", "a/info_only.ts"]);
        let main = &groups[1];
        assert_eq!((main.path.as_str(), main.name.as_str(), main.dir.as_str()), ("/proj/src/main.rs", "main.rs", "src"));
        assert_eq!(main.counts, ProblemCounts { errors: 3, warnings: 1, infos: 2 });
    }

    #[test]
    fn within_a_file_by_severity_then_line_then_column() {
        let files = files();
        let groups = groups(&files, &ProblemFilter::default());
        let main = groups.iter().find(|group| group.name == "main.rs").unwrap();
        let order: Vec<String> = main.items.iter().map(|item| format!("{:?}:{}:{}", item.severity, item.line, item.column)).collect();
        assert_eq!(order, ["Error:5:2", "Error:5:9", "Error:20:4", "Warning:1:0", "Info:1:0", "Hint:2:0"]);
        // Daftar di store tidak ikut diurutkan.
        assert_eq!(files["/proj/src/main.rs"][0].severity, Severity::Warning);
    }

    #[test]
    fn severity_toggles_hide_their_kind_and_files_left_empty() {
        let files = files();
        let errors_only = groups(&files, &ProblemFilter { warnings: false, infos: false, ..Default::default() });
        assert_eq!(relatives(&errors_only), ["lib/errors.go", "src/main.rs"]);
        assert_eq!(errors_only[1].items.len(), 3);
        assert_eq!(errors_only[1].counts, ProblemCounts { errors: 3, warnings: 0, infos: 0 });

        // Tanpa galat, main.rs diurutkan sebagai berkas peringatan.
        let no_errors = groups(&files, &ProblemFilter { errors: false, ..Default::default() });
        assert_eq!(relatives(&no_errors), ["src/main.rs", "src/warn_only.rs", "a/info_only.ts"]);
        assert!(groups(&files, &ProblemFilter { text: String::new(), errors: false, warnings: false, infos: false }).is_empty());
    }

    #[test]
    fn filter_text_matches_message_source_code_or_path_case_insensitively() {
        let files = files();
        let by = |text: &str| -> Vec<String> {
            groups(&files, &ProblemFilter { text: text.into(), ..Default::default() }).iter().map(|group| format!("{}={}", group.relative, group.items.len())).collect()
        };
        assert_eq!(by("UNUSED"), ["src/main.rs=1", "src/warn_only.rs=1"]);
        assert_eq!(by("e0308"), ["src/main.rs=1"]);
        assert_eq!(by("eslint"), ["a/info_only.ts=1"]);
        // Path cocok: semua masalah berkas itu lolos.
        assert_eq!(by("main.rs"), ["src/main.rs=6"]);
        assert_eq!(by("  compiler  "), ["lib/errors.go=1"]);
        assert!(by("nothing matches this").is_empty());
    }

    #[test]
    fn paths_outside_the_project_and_windows_paths_still_split_into_name_and_folder() {
        let files = BTreeMap::from([
            ("/elsewhere/x.rs".to_string(), vec![d(Severity::Error, 1, 0, "e")]),
            ("C:\\proj\\src\\w.rs".to_string(), vec![d(Severity::Error, 1, 0, "e")]),
            ("/proj/top.rs".to_string(), vec![d(Severity::Error, 1, 0, "e")]),
        ]);
        let groups = groups(&files, &ProblemFilter::default());
        let split: Vec<(&str, &str)> = groups.iter().map(|group| (group.name.as_str(), group.dir.as_str())).collect();
        assert_eq!(split, [("x.rs", "/elsewhere"), ("w.rs", "C:/proj/src"), ("top.rs", "")]);
    }

    #[test]
    fn file_titles_are_followed_by_their_problems_unless_collapsed() {
        let files = files();
        let groups = groups(&files, &ProblemFilter { infos: false, ..Default::default() });
        let rows = flatten_rows(&groups, &HashSet::from(["/proj/src/main.rs".to_string()]));
        let shown: Vec<String> = rows
            .iter()
            .map(|row| match row {
                ProblemRow::File(group) => format!("# {}", groups[*group].name),
                ProblemRow::Problem(group, item) => groups[*group].items[*item].message.clone(),
            })
            .collect();
        assert_eq!(shown, ["# errors.go", "undefined: foo", "# main.rs", "# warn_only.rs", "unused import"]);
    }

    #[test]
    fn relative_paths_are_taken_from_the_project_root() {
        assert_eq!(relative_path(Some("/proj"), "/proj/src/a.rs"), "src/a.rs");
        assert_eq!(relative_path(Some("/proj/"), "/proj/a.rs"), "a.rs");
        assert_eq!(relative_path(Some("/proj"), "/project/a.rs"), "/project/a.rs");
        assert_eq!(relative_path(Some("C:\\proj"), "C:\\proj\\src\\a.rs"), "src\\a.rs");
        assert_eq!(relative_path(None, "/x/a.rs"), "/x/a.rs");
    }
}
