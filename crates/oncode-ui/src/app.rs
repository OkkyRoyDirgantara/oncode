//! Jembatan antara `oncode-core` (tokio) dan GPUI.
//!
//! Crate backend memakai tokio, sedangkan GPUI punya eksekutornya sendiri. Aturannya:
//! pekerjaan backend dijalankan di runtime tokio lewat [`spawn_core`], hasilnya ditunggu
//! dari tugas GPUI. Event dari core diteruskan ke thread utama dan disiarkan lewat [`CoreBus`].

use std::future::Future;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global, Task};
use oncode_core::{CoreEvent, Shared};
use tokio::sync::mpsc::UnboundedReceiver;

/// State global aplikasi.
pub struct OnCode {
    pub core: Shared,
    /// Runtime tokio tempat semua pekerjaan backend berjalan; hidup selama aplikasi.
    pub runtime: Arc<tokio::runtime::Runtime>,
    /// Event dari core, diisi oleh sink yang dipasang saat `Core` dibuat.
    pub events: Option<UnboundedReceiver<CoreEvent>>,
}

struct Globals {
    core: Shared,
    runtime: Arc<tokio::runtime::Runtime>,
    bus: Entity<CoreBus>,
}

impl Global for Globals {}

/// Menyiarkan [`CoreEvent`] di thread utama. Berlangganan dengan `cx.subscribe(&core_bus(cx), ..)`.
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

/// Menjalankan pekerjaan backend di runtime tokio dan mengembalikan tugas GPUI untuk hasilnya.
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
    cx.background_spawn(async move { handle.await.expect("tugas backend panik") })
}
