//! Hot exit: closing a window with unsaved changes keeps them (also in new files) instead of asking, and
//! they come back the next time the folder is opened.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_core::api::backup;
use oncode_ui::editor::{active_file, buffers, open_file, OpenFile};
use oncode_ui::testing::{TestApp, TestWindow};

fn typed(window: &TestWindow, cx: &mut TestAppContext, text: &str) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input(text, cx);
    });
    cx.run_until_parked();
}

async fn close(window: &TestWindow, cx: &mut TestAppContext) {
    let shell = window.shell.clone();
    window.update(cx, |window, cx| shell.update(cx, |shell, cx| shell.close_window(window, cx)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == window.handle.window_id())) {
        assert!(std::time::Instant::now() < deadline, "the window did not close (a question was asked?)");
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[gpui_kit::test]
async fn closing_a_window_keeps_unsaved_changes_and_reopening_restores_them(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("a.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "a.txt loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, b, _)| b.read(cx).is_ready())).await;
    typed(&window, cx, "x");
    // A new file with content.
    assert!(window.run_command(cx, "file.newUntitled"));
    cx.run_until_parked();
    typed(&window, cx, "draft");

    close(&window, cx).await;
    assert_eq!(std::fs::read_to_string(project.join("a.txt")).unwrap(), "a\n", "nothing was saved to the file");
    let core = app.core.clone();
    let root = project.to_string_lossy().into_owned();
    let kept = app.runtime.block_on(async move { backup::backup_list(&core, root).await }).unwrap();
    let mut texts: Vec<&str> = kept.iter().map(|b| b.text.as_str()).collect();
    texts.sort();
    assert_eq!(texts, ["draft", "xa\n"]);

    // Opening the folder again brings both back as unsaved changes.
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let registry = cx.update(|cx| buffers(workbench.read(cx)).unwrap());
    window
        .wait_until(cx, "unsaved changes restored", |_, cx| {
            let dirty = registry.read(cx).dirty(cx);
            dirty.len() == 2 && dirty.iter().all(|b| b.read(cx).is_ready())
        })
        .await;
    let mut restored: Vec<(String, String)> = cx.update(|cx| {
        registry.read(cx).dirty(cx).iter().map(|b| (b.read(cx).file_name().to_string(), b.read(cx).text(cx).unwrap().to_string())).collect()
    });
    restored.sort();
    assert_eq!(restored, [("Untitled-1".to_string(), "draft".to_string()), ("a.txt".to_string(), "xa\n".to_string())]);

    // Saving drops the backup.
    let a = cx.update(|cx| registry.read(cx).get(&path).unwrap());
    window.update(cx, |window, cx| a.update(cx, |buffer, cx| buffer.save(window, cx)).detach());
    window.wait_until(cx, "a.txt saved", |_, cx| !a.read(cx).is_dirty()).await;
    let (core, root) = (app.core.clone(), project.to_string_lossy().into_owned());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (core, root) = (core.clone(), root.clone());
        let left = app.runtime.block_on(async move { backup::backup_list(&core, root).await }).unwrap();
        if left.len() == 1 {
            assert_eq!(left[0].text, "draft");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the saved file's backup was not removed");
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// With hot exit off, closing asks as before (the window stays open until answered).
#[gpui_kit::test]
async fn without_hot_exit_closing_asks(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let mut settings = app.core.user_settings();
    settings.files.hot_exit = false;
    app.core.set_user_settings(settings).unwrap();
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("a.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "a.txt loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, b, _)| b.read(cx).is_ready())).await;
    typed(&window, cx, "x");
    let shell = window.shell.clone();
    window.update(cx, |window, cx| shell.update(cx, |shell, cx| shell.close_window(window, cx)));
    cx.run_until_parked();
    assert!(cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == window.handle.window_id())), "the window waits for an answer");
    let core = app.core.clone();
    let root = project.to_string_lossy().into_owned();
    assert!(app.runtime.block_on(async move { backup::backup_list(&core, root).await }).unwrap().is_empty(), "no backup without hot exit");
}

/// Backups from a session that had hot exit on are not restored once it is off, and are removed so they
/// cannot come back later with outdated content.
#[gpui_kit::test]
async fn backups_are_not_restored_with_hot_exit_off(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n")]);
    let (core, root) = (app.core.clone(), project.to_string_lossy().into_owned());
    let path = project.join("a.txt").to_string_lossy().into_owned();
    let meta = std::fs::metadata(&path).unwrap();
    let mtime_ms = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0;
    let old = backup::BufferBackup { path: path.clone(), text: "old edit\n".into(), encoding: "UTF-8".into(), bom: false, eol: "lf".into(), mtime_ms, size: meta.len() as u32 };
    app.runtime.block_on(backup::backup_write(&core, root.clone(), old)).unwrap();
    let mut settings = app.core.user_settings();
    settings.files.hot_exit = false;
    app.core.set_user_settings(settings).unwrap();

    let window = app.open(Some(&project), cx).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !app.runtime.block_on(backup::backup_list(&core, root.clone())).unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "the old backup was not removed");
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    cx.run_until_parked();
    let workbench = window.workbench(cx);
    let registry = cx.update(|cx| buffers(workbench.read(cx)).unwrap());
    assert!(cx.update(|cx| registry.read(cx).dirty(cx).is_empty()), "nothing was restored");
}
