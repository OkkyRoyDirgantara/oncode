//! Flow control for terminal output: if the emulator falls behind processing, reading the PTY in the backend is paused.

use std::sync::Mutex;

/// Pause when the queue exceeds this number (in bytes).
pub const HIGH_WATERMARK: usize = 500_000;
/// Resume when the queue drops below this number.
pub const LOW_WATERMARK: usize = 100_000;

/// Counts the bytes that have been received from the PTY but not yet finished processing by the
/// emulator, and decides when reading pauses and resumes.
///
/// Shared between the thread that delivers PTY output (`written`, so reading pauses as soon as the
/// queue is too long, even while the UI thread is busy drawing) and the UI thread (`processed`).
/// `set_paused` is called only when the state changes, and while the state is locked: a pause and a
/// resume decided at the same time on the two threads reach the backend in the order they were
/// decided, so the backend never stays paused while this says it is reading (or the other way round).
#[derive(Debug)]
pub struct FlowControl {
    state: Mutex<State>,
    high: usize,
    low: usize,
}

#[derive(Debug, Default)]
struct State {
    pending: usize,
    paused: bool,
}

impl Default for FlowControl {
    fn default() -> Self {
        Self::new(HIGH_WATERMARK, LOW_WATERMARK)
    }
}

impl FlowControl {
    /// Panics if `low > high` (inverted configuration).
    pub fn new(high: usize, low: usize) -> Self {
        assert!(low <= high, "low watermark must not be greater than the high watermark");
        Self { state: Mutex::new(State::default()), high, low }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A chunk was received and entered the queue; calls `set_paused(true)` if reading must pause now.
    pub fn written(&self, length: usize, set_paused: impl FnOnce(bool)) {
        let mut state = self.state();
        state.pending = state.pending.saturating_add(length);
        if state.pending > self.high && !state.paused {
            state.paused = true;
            set_paused(true);
        }
    }

    /// The emulator finished processing part of the queue; calls `set_paused(false)` if reading may resume now.
    pub fn processed(&self, length: usize, set_paused: impl FnOnce(bool)) {
        let mut state = self.state();
        state.pending = state.pending.saturating_sub(length);
        if state.pending < self.low && state.paused {
            state.paused = false;
            set_paused(false);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.state().paused
    }

    pub fn pending(&self) -> usize {
        self.state().pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records `setPaused` calls like the mock in the v0.2 tests.
    struct Recorder {
        flow: FlowControl,
        calls: Vec<bool>,
    }

    impl Recorder {
        fn new(high: usize, low: usize) -> Self {
            Self { flow: FlowControl::new(high, low), calls: Vec::new() }
        }

        fn written(&mut self, length: usize) {
            self.flow.written(length, |paused| self.calls.push(paused));
        }

        fn processed(&mut self, length: usize) {
            self.flow.processed(length, |paused| self.calls.push(paused));
        }
    }

    #[test]
    fn does_not_pause_while_the_queue_is_below_the_high_watermark() {
        let mut r = Recorder::new(100, 20);
        r.written(60);
        r.written(40);
        assert_eq!(r.flow.pending(), 100);
        assert!(r.calls.is_empty());
        r.processed(100);
        assert!(r.calls.is_empty());
    }

    #[test]
    fn pauses_once_above_high_and_resumes_below_low() {
        let mut r = Recorder::new(100, 20);
        r.written(80);
        r.written(80);
        assert_eq!(r.calls, [true]);
        assert!(r.flow.is_paused());
        // A chunk that arrives while paused does not trigger a second call.
        r.written(50);
        assert_eq!(r.calls.len(), 1);

        r.processed(80); // 130 left
        r.processed(80); // 50 left: still above the low watermark
        assert_eq!(r.calls.len(), 1);
        r.processed(31); // 19 left
        assert_eq!(r.calls, [true, false]);
        assert!(!r.flow.is_paused());
    }

    #[test]
    fn exactly_at_low_does_not_resume_and_the_queue_never_goes_negative() {
        let mut r = Recorder::new(100, 20);
        r.written(101);
        r.processed(81); // exactly 20
        assert!(r.flow.is_paused());
        r.processed(1000);
        assert_eq!(r.flow.pending(), 0);
        assert!(!r.flow.is_paused());
    }

    #[test]
    fn can_pause_again_after_resuming() {
        let mut r = Recorder::new(100, 20);
        r.written(150);
        r.processed(150);
        r.written(150);
        assert_eq!(r.calls, [true, false, true]);
    }

    #[test]
    fn uses_default_watermarks() {
        let mut r = Recorder { flow: FlowControl::default(), calls: Vec::new() };
        r.written(HIGH_WATERMARK);
        assert!(r.calls.is_empty());
        r.written(1);
        assert_eq!(r.calls.last(), Some(&true));
        r.processed(HIGH_WATERMARK + 1 - LOW_WATERMARK + 1);
        assert_eq!(r.calls.last(), Some(&false));
    }

    /// Output arriving on one thread while the emulator drains on another: the backend sees strictly
    /// alternating pause/resume calls, and ends in the same state as the counter.
    #[test]
    fn the_backend_follows_the_state_under_concurrent_use() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let flow = Arc::new(FlowControl::new(1_000, 200));
        let backend = Arc::new(Mutex::new((false, 0usize)));
        let apply = |backend: &Arc<Mutex<(bool, usize)>>| {
            let backend = backend.clone();
            move |paused: bool| {
                let mut backend = backend.lock().unwrap();
                assert_ne!(backend.0, paused, "pause and resume calls alternate");
                *backend = (paused, backend.1 + 1);
            }
        };
        let done = Arc::new(AtomicBool::new(false));
        let reader = {
            let (flow, backend, done) = (flow.clone(), backend.clone(), done.clone());
            std::thread::spawn(move || {
                for _ in 0..20_000 {
                    flow.written(97, apply(&backend));
                }
                done.store(true, Ordering::Release);
            })
        };
        // The emulator processes what has been delivered, in steps.
        let mut delivered = 0usize;
        loop {
            let finished = done.load(Ordering::Acquire);
            let available = 20_000 * 97 - delivered;
            let step = if finished { available } else { available.min(flow.pending()).min(300) };
            flow.processed(step, apply(&backend));
            delivered += step;
            if finished && delivered == 20_000 * 97 {
                break;
            }
        }
        reader.join().unwrap();
        assert_eq!(flow.pending(), 0);
        assert!(!flow.is_paused());
        let (paused, calls) = *backend.lock().unwrap();
        assert!(!paused, "the backend reads again at the end");
        assert!(calls > 0 && calls % 2 == 0, "{calls} calls");
    }

    #[test]
    #[should_panic(expected = "low watermark")]
    fn rejects_inverted_configuration() {
        let _ = FlowControl::new(10, 20);
    }
}
