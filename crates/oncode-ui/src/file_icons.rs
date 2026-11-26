//! Colored icons per language and file type (Explorer, tabs, Quick Open, Source Control).
//!
//! The icons come from Material Icon Theme (MIT, see `assets/file-icons/LICENSE`), embedded
//! in the binary and loaded through [`Assets`]. They are shown with `img()` so their colors stay
//! intact (GPUI's `svg()` element only paints a single color).

use std::borrow::Cow;

use gpui_kit::{img, px, AssetSource, Img, Result, SharedString, Styled as _};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/file-icons/", $name, ".svg")) as &[u8])),*]
    };
}

/// All embedded icons: (name without `.svg`, file content).
const ICONS: &[(&str, &[u8])] = icons![
    "android", "audio", "c", "certificate", "changelog", "cmake", "console", "cpp", "csharp", "css", "dart", "database", "diff", "docker", "document",
    "editorconfig", "elixir", "eslint", "exe", "folder-android", "folder-api", "folder-app", "folder-base", "folder-components", "folder-config",
    "folder-database", "folder-dist", "folder-docs", "folder-flutter", "folder-git", "folder-github", "folder-images", "folder-ios", "folder-lib",
    "folder-node", "folder-public", "folder-resource", "folder-scripts", "folder-src", "folder-target", "folder-test", "folder-utils", "folder-vscode",
    "font", "git", "go", "gradle", "graphql", "groovy", "h", "hpp", "html", "http", "image", "java", "javascript", "json", "key", "kotlin", "less",
    "license", "lock", "log", "lua", "makefile", "markdown", "nodejs", "npm", "pdf", "php", "powershell", "prettier", "proto", "python", "react",
    "react_ts", "readme", "ruby", "rust", "sass", "scala", "settings", "svelte", "svg", "swift", "tailwindcss", "toml", "tsconfig", "tune",
    "typescript-def", "typescript", "video", "vite", "vue", "xml", "yaml", "zig", "zip",
];

/// Path prefix of the file icon assets.
const PREFIX: &str = "file-icons/";

/// The application's asset source: OnCode's file icons, then gpui-kit's bundled assets (Lucide icons, etc.).
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = path.strip_prefix(PREFIX) {
            let name = file.strip_suffix(".svg").unwrap_or(file);
            return Ok(ICONS.iter().find(|(known, _)| *known == name).map(|(_, bytes)| Cow::Borrowed(*bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut list = gpui_kit::assets::AllAssets.list(path)?;
        list.extend(ICONS.iter().map(|(name, _)| format!("{PREFIX}{name}.svg")).filter(|asset| asset.starts_with(path)).map(SharedString::from));
        Ok(list)
    }
}

fn asset(name: &str) -> SharedString {
    format!("{PREFIX}{name}.svg").into()
}

/// The icon name for a specific file name (without folder), before looking at the extension.
fn icon_for_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "package.json" => "nodejs",
        "package-lock.json" | ".npmrc" | ".npmignore" => "npm",
        "tsconfig.json" | "jsconfig.json" => "tsconfig",
        ".editorconfig" => "editorconfig",
        "dockerfile" | ".dockerignore" | "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml" => "docker",
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => "git",
        "readme" | "readme.md" | "readme.txt" => "readme",
        "license" | "license.md" | "license.txt" | "licence" | "copying" => "license",
        "changelog" | "changelog.md" => "changelog",
        "makefile" | "gnumakefile" => "makefile",
        "cmakelists.txt" => "cmake",
        "cargo.toml" => "rust",
        "pubspec.yaml" | "pubspec.lock" | "analysis_options.yaml" => "dart",
        "build.gradle" | "settings.gradle" | "build.gradle.kts" | "settings.gradle.kts" | "gradlew" | "gradlew.bat" => "gradle",
        "androidmanifest.xml" => "android",
        "go.mod" | "go.sum" => "go",
        "requirements.txt" | "pyproject.toml" | "pipfile" => "python",
        "gemfile" | "rakefile" => "ruby",
        _ => {
            if name.starts_with(".env") {
                "tune"
            } else if name.starts_with("vite.config.") {
                "vite"
            } else if name.starts_with("tailwind.config.") {
                "tailwindcss"
            } else if name.starts_with(".prettierrc") || name.starts_with("prettier.config.") {
                "prettier"
            } else if name.starts_with(".eslintrc") || name.starts_with("eslint.config.") {
                "eslint"
            } else if name.ends_with(".d.ts") {
                "typescript-def"
            } else {
                return None;
            }
        }
    })
}

/// The icon name for an extension (lowercase, without the dot).
fn icon_for_extension(extension: &str) -> &'static str {
    match extension {
        "rs" => "rust",
        "py" | "pyi" | "pyw" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "react_ts",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "react",
        "go" => "go",
        "kt" | "kts" => "kotlin",
        "dart" => "dart",
        "java" | "class" | "jar" => "java",
        "cs" | "csx" => "csharp",
        "php" => "php",
        "lua" => "lua",
        "swift" => "swift",
        "rb" => "ruby",
        "c" => "c",
        "h" => "h",
        "cc" | "cpp" | "cxx" => "cpp",
        "hh" | "hpp" | "hxx" => "hpp",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "sass",
        "less" => "less",
        "json" | "jsonc" | "json5" | "jsonl" | "ndjson" | "arb" => "json",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" | "mdx" => "markdown",
        "sh" | "bash" | "zsh" | "fish" => "console",
        "ps1" | "psm1" | "psd1" => "powershell",
        "sql" | "psql" | "mysql" | "cql" | "sqlite" | "db" | "mongodb" => "database",
        "graphql" | "gql" => "graphql",
        "proto" => "proto",
        "vue" => "vue",
        "svelte" => "svelte",
        "xml" | "plist" | "xaml" => "xml",
        "svg" => "svg",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "avif" => "image",
        "pdf" => "pdf",
        "zip" | "tar" | "gz" | "tgz" | "rar" | "7z" | "bz2" | "xz" => "zip",
        "lock" => "lock",
        "log" => "log",
        "ini" | "cfg" | "conf" | "properties" => "settings",
        "diff" | "patch" => "diff",
        "ttf" | "otf" | "woff" | "woff2" => "font",
        "mp4" | "mov" | "webm" | "mkv" | "avi" => "video",
        "mp3" | "wav" | "ogg" | "flac" | "m4a" => "audio",
        "exe" | "msi" | "dll" => "exe",
        "pem" | "crt" | "cer" => "certificate",
        "key" | "pub" => "key",
        "http" | "rest" => "http",
        "zig" => "zig",
        "ex" | "exs" => "elixir",
        "scala" | "sc" | "sbt" => "scala",
        "gradle" => "gradle",
        "groovy" => "groovy",
        "cmake" => "cmake",
        "mk" => "makefile",
        _ => "document",
    }
}

fn file_icon_name(path: &str) -> &'static str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_lowercase();
    if let Some(icon) = icon_for_name(&name) {
        return icon;
    }
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => icon_for_extension(extension),
        _ => "document",
    }
}

fn folder_icon_name(path: &str) -> &'static str {
    let name = path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path).to_lowercase();
    match name.as_str() {
        "src" | "source" | "sources" => "folder-src",
        "test" | "tests" | "__tests__" | "spec" | "specs" => "folder-test",
        ".git" => "folder-git",
        ".github" => "folder-github",
        "node_modules" => "folder-node",
        "docs" | "doc" | "documentation" => "folder-docs",
        "images" | "img" | "icons" | "screenshots" => "folder-images",
        "assets" | "res" | "resources" => "folder-resource",
        "config" | "configs" | ".config" => "folder-config",
        "dist" | "out" | "build" => "folder-dist",
        "lib" | "libs" => "folder-lib",
        "components" => "folder-components",
        ".vscode" => "folder-vscode",
        "public" | "static" | "www" => "folder-public",
        "scripts" | "bin" => "folder-scripts",
        "app" | "apps" => "folder-app",
        "android" => "folder-android",
        "ios" => "folder-ios",
        "target" => "folder-target",
        "utils" | "util" | "helpers" => "folder-utils",
        "api" => "folder-api",
        "db" | "database" | "migrations" => "folder-database",
        ".dart_tool" | "flutter" => "folder-flutter",
        _ => "folder-base",
    }
}

/// The icon asset path for a file (name or full path).
pub fn file_icon_path(path: &str) -> SharedString {
    asset(file_icon_name(path))
}

/// The icon asset path for a folder (name or full path).
pub fn folder_icon_path(path: &str) -> SharedString {
    asset(folder_icon_name(path))
}

/// A ready-to-use file icon (16 px).
pub fn file_icon(path: &str) -> Img {
    img(file_icon_path(path)).size(px(16.)).flex_none()
}

/// A ready-to-use folder icon (16 px).
pub fn folder_icon(path: &str) -> Img {
    img(folder_icon_path(path)).size(px(16.)).flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_of(path: &str) -> &str {
        path.strip_prefix(PREFIX).and_then(|p| p.strip_suffix(".svg")).unwrap()
    }

    #[test]
    fn languages_and_special_files_get_their_logo() {
        for (file, icon) in [
            ("src/main.rs", "rust"),
            ("C:\\project\\app.py", "python"),
            ("web/App.tsx", "react_ts"),
            ("types/index.d.ts", "typescript-def"),
            ("lib/main.dart", "dart"),
            ("Main.kt", "kotlin"),
            ("Program.cs", "csharp"),
            ("query.sql", "database"),
            ("export.jsonl", "json"),
            ("package.json", "nodejs"),
            ("Cargo.toml", "rust"),
            ("pubspec.yaml", "dart"),
            ("Dockerfile", "docker"),
            (".gitignore", "git"),
            ("README.md", "readme"),
            (".env.local", "tune"),
            ("vite.config.ts", "vite"),
            ("notes", "document"),
            (".bashrc", "document"),
            ("archive.unknown", "document"),
        ] {
            assert_eq!(name_of(&file_icon_path(file)), icon, "{file}");
        }
    }

    #[test]
    fn special_folders_and_the_default_folder() {
        assert_eq!(name_of(&folder_icon_path("/p/src")), "folder-src");
        assert_eq!(name_of(&folder_icon_path("node_modules")), "folder-node");
        assert_eq!(name_of(&folder_icon_path(".github/")), "folder-github");
        assert_eq!(name_of(&folder_icon_path("Tests")), "folder-test");
        assert_eq!(name_of(&folder_icon_path("features")), "folder-base");
    }

    /// Every icon that might be chosen is really embedded, and gpui-kit's assets can still be read.
    #[test]
    fn every_mapped_icon_is_embedded_and_other_assets_still_load() {
        let names: Vec<&str> = ICONS.iter().map(|(name, _)| *name).collect();
        let probe = [
            "a.rs", "a.py", "a.ts", "a.tsx", "a.js", "a.jsx", "a.go", "a.kt", "a.dart", "a.java", "a.cs", "a.php", "a.lua", "a.swift", "a.rb", "a.c", "a.h",
            "a.cpp", "a.hpp", "a.html", "a.css", "a.scss", "a.less", "a.json", "a.yml", "a.toml", "a.md", "a.sh", "a.ps1", "a.sql", "a.graphql",
            "a.proto", "a.vue", "a.svelte", "a.xml", "a.svg", "a.png", "a.pdf", "a.zip", "a.lock", "a.log", "a.ini", "a.diff", "a.ttf", "a.mp4", "a.mp3",
            "a.exe", "a.pem", "a.key", "a.http", "a.zig", "a.ex", "a.scala", "a.gradle", "a.groovy", "a.cmake", "a.mk", "a.d.ts", "package.json",
            "package-lock.json", "tsconfig.json", ".editorconfig", "Dockerfile", ".gitignore", "README.md", "LICENSE", "CHANGELOG.md", "Makefile",
            "CMakeLists.txt", "AndroidManifest.xml", ".env", "vite.config.js", "tailwind.config.js", ".prettierrc", "eslint.config.js",
        ];
        for file in probe {
            let icon = file_icon_name(file);
            assert!(names.contains(&icon), "{file} → {icon} is not embedded");
            assert!(Assets.load(&file_icon_path(file)).unwrap().is_some_and(|bytes| bytes.starts_with(b"<svg")), "{file}");
        }
        for folder in ["src", "tests", ".git", ".github", "node_modules", "docs", "images", "assets", "config", "dist", "lib", "components", ".vscode", "public", "scripts", "app", "android", "ios", "target", "utils", "api", "db", ".dart_tool", "other"] {
            assert!(names.contains(&folder_icon_name(folder)), "{folder}");
        }
        assert_eq!(Assets.load("file-icons/does-not-exist.svg").unwrap(), None);
        // gpui-kit's Lucide icons are still loaded through the same source.
        let lucide = crate::Lucide::File.path();
        assert!(Assets.load(&lucide).unwrap().is_some(), "{lucide}");
        assert!(Assets.list(PREFIX).unwrap().len() >= ICONS.len());
    }
}
