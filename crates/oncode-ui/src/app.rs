//! Bridge between `oncode-core` (tokio) and GPUI.
//!
//! Backend crates use tokio, while GPUI has its own executor. The rule: backend work
//! runs on the tokio runtime via [`spawn_core`], and its result is awaited from a GPUI
//! task. Events from the core are forwarded to the main thread and broadcast via [`CoreBus`].

use std::future::Future;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global, Task};
use oncode_core::{CoreEvent, Shared};
use tokio::sync::mpsc::UnboundedReceiver;

/// Global application state.
pub struct OnCode {
    pub core: Shared,
    /// The tokio runtime where all backend work runs; lives as long as the application.
    pub runtime: Arc<tokio::runtime::Runtime>,
    /// Events from the core, fed by the sink installed when `Core` is created.
    pub events: Option<UnboundedReceiver<CoreEvent>>,
}

struct Globals {
    core: Shared,
    runtime: Arc<tokio::runtime::Runtime>,
    bus: Entity<CoreBus>,
}

impl Global for Globals {}

/// Broadcasts [`CoreEvent`] on the main thread. Subscribe with `cx.subscribe(&core_bus(cx), ..)`.
pub struct CoreBus;

impl EventEmitter<CoreEvent> for CoreBus {}

pub(crate) fn install(mut app: OnCode, cx: &mut App) {
    let bus = cx.new(|_| CoreBus);
    if let Some(mut events) = app.events.take() {
        let bus = bus.downgrade();
        cx.spawn(async move |cx| {
            while let Some(event) = events.recv().await {
                if bus.update(cx, |_, cx| cx.emit(event)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }
    cx.set_global(Globals { core: app.core, runtime: app.runtime, bus });
}

pub fn core(cx: &App) -> Shared {
    cx.global::<Globals>().core.clone()
}

pub fn core_bus(cx: &App) -> Entity<CoreBus> {
    cx.global::<Globals>().bus.clone()
}

/// Runs backend work on the tokio runtime and returns a GPUI task for its result.
///
/// ```ignore
/// let task = spawn_core(cx, async move { api::git::git_stage(&core, &root, paths).await });
/// cx.spawn(async move |this, cx| { let result = task.await; this.update(cx, |this, cx| ..) }).detach();
/// ```
pub fn spawn_core<T, F>(cx: &App, future: F) -> Task<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let handle = cx.global::<Globals>().runtime.spawn(future);
    cx.background_spawn(async move { handle.await.expect("backend task panicked") })
}
