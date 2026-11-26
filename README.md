# OnCode

A **native** desktop code editor that is simpler than Visual Studio Code, with a **full-featured Git client** as its main feature. Written in Rust and drawn directly on the GPU with [GPUI](https://github.com/zed-industries/zed) (the UI framework behind the Zed editor), with no WebView and no JavaScript.

The full design is in [docs/DESIGN.md](docs/DESIGN.md); the development guide is in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

> The previous version (v0.2) was built with Tauri + React. Its code is kept under the git tag `tauri-v0.2` and its design in [docs/archive/DESIGN-v0.2-tauri.md](docs/archive/DESIGN-v0.2-tauri.md).

## Features

**Source control**

- Change status, stage / unstage / discard per file, **per hunk, and per line**
- Commit (amend, sign-off), branch, merge, fetch / pull / push with an in-app credential dialog
- Commit history graph with search, commit details, and comparison of two commits
- Visual **interactive rebase** (pick, reword, edit, squash, fixup, drop, reorder)
- **Three-pane conflict editor** (Current | Result | Incoming)
- Stash, tag, cherry-pick, revert, reset, blame, file history, reflog
- **Inline blame** like GitLens: the author, time, and commit message of the cursor line appear faintly at the end of that line (can be turned off via Git → Toggle Inline Blame or Settings → Editor)
- Worktrees and submodules
- Every Git command that is run is recorded in the **Git Output** panel
- **Generate commit message** with AI (optional): writes a message from the staged changes, following the repository's commit style. Providers (Settings → AI): Claude Code CLI (Claude Code login, including a Claude subscription; no API key), Anthropic API, or a custom endpoint (OpenAI-compatible such as OpenAI, OpenRouter, Groq, Ollama, LM Studio, or an Anthropic-format gateway). The staged diff is sent only when the button is pressed

**Editor and workspace**

- Explorer with Git status markers, editor tabs and splits (tabs can be dragged to reorder, move between groups, or be dropped on a group's edge to split left/right/up/down), drag and drop in the Explorer (drag a file or folder onto another folder to move it, onto the empty background to move it to the project root, or onto the editor area to open the file as a tab or split the group; files dropped from the OS file manager onto the Explorer are copied into the folder under the pointer, and those dropped onto the editor area are opened), quick open (`Ctrl+P`), command palette (`Ctrl+Shift+P`), and File/Edit/View/Go/Git/Terminal/Help menus in the title bar
- New untitled file (`Ctrl+N`, saved via Save As), move line (`Ctrl+↑/↓` or `Alt+↑/↓`)
- Syntax highlighting (tree-sitter) including Kotlin, Dart/Flutter, C#, PHP, Lua, Swift, GraphQL; find/replace, multi-cursor, code folding
- Search and replace across the whole project (respects `.gitignore`)
- Encoding and line-ending detection, detection of external changes, tab restoration when a folder is reopened
- The **`oncode .`** command in the terminal, like `code .` in VS Code: opens the current folder. If OnCode is already running, the folder is opened in the same process (a new window, or the window that already has that folder open is focused), so no second process eats memory. Installed automatically by the Windows installer (`bin\oncode.cmd` + user PATH) and the `.deb` package (`/usr/bin/oncode`); on macOS, AppImage, or a self-built binary, use the command palette → **Shell Command: Install 'oncode' Command in PATH**
- Integrated terminal with multiple sessions: side-by-side splits in the panel (`Cmd+\` on macOS, `Ctrl+Shift+5` on Linux/Windows), terminals as tabs in the editor area (can sit next to code), and drag and drop of terminals between the panel list and the editor area. Drawn per cell: columns stay straight, box-drawing lines and powerline prompts connect seamlessly, no ligatures; the terminal font and line height can be set in Settings → Terminal
- LSP: completion, hover, diagnostics, go to definition, references, rename, format (language servers are installed by you)

**Extensions** (built in, each can be turned off in Settings → Extensions)

- **Markdown Preview** (`Ctrl+Shift+V`): live preview next to the editor
- **Formatters** (`Shift+Alt+F`, optional Format on Save): Prettier, rustfmt, gofmt, Ruff/Black, dart format, ktlint, sql-formatter, clang-format, and others when installed; otherwise formatting comes from the language server
- **Extra languages**: highlighting for additional languages
- **Go to Definition without a language server**: `Ctrl+click`/`F12` searches for definitions across the whole project
- GitHub and GitLab: login, clone from your account, list / view / check out / create pull requests

**Deliberately not included:** an extension marketplace, debugger, remote development, notebooks, settings sync, an AI assistant in the editor.

## Running

Prerequisites: Rust (stable), Git 2.40+, a C compiler, and the GPUI system libraries (see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#prerequisites)). At runtime a GPU with Vulkan (Linux), DirectX 11 (Windows), or Metal (macOS) is required.

```bash
cargo build -p oncode-helper        # companion binary for the credential dialog and interactive rebase
cargo run -p oncode                 # development mode
cargo run -p oncode -- /path/project # open a folder directly
```

To try `oncode .` from a self-built binary: run the app, then command palette → **Shell Command: Install 'oncode' Command in PATH** (it uses `oncode-helper`, so build both). On Windows, open a new terminal afterwards.

## Testing

```bash
cargo test --workspace  # backend (against real git), UI logic, and headless view tests
```

## Structure

```
app/            The `oncode-app` binary: entry point
crates/
  oncode-core/      UI-free application state and the entire API
  oncode-ui/        UI framework: window, tabs, commands, keybindings, editor
  oncode-ui-*/      One feature per crate: explorer, search, diff, scm, graph, terminal, lsp, forge, settings
  oncode-git/ oncode-workspace/ oncode-terminal/ oncode-lsp/ oncode-forge/ oncode-helper/   Backend
docs/           Design and development guide
```

## Security

A folder opened for the first time runs in **restricted mode**: Git, language servers, and project settings are disabled until you trust the folder, because an unknown repository can run code through its Git or language server configuration. This includes a repository you just cloned; its submodules are fetched only after you trust it (Update Submodules). Even in a trusted folder, `.oncode/settings.json` cannot choose which programs OnCode runs (language servers, shell, Git) or change the AI and account settings. Account tokens are stored only in the operating system's keychain.

## License

MIT
