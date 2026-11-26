//! Flow control for terminal output: if the emulator falls behind processing, reading the PTY in the backend is paused.

/// Pause when the queue exceeds this number (in bytes).
pub const HIGH_WATERMARK: usize = 500_000;
/// Resume when the queue drops below this number.
pub const LOW_WATERMARK: usize = 100_000;

/// Counts the bytes that have been received from the PTY but not yet finished processing by the
/// emulator. `written` and `processed` return `Some(paused)` only when the state changes, so the
/// backend is not called on every chunk.
#[derive(Debug)]
pub struct FlowControl {
    pending: usize,
    paused: bool,
    high: usize,
    low: usize,
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
        Self { pending: 0, paused: false, high, low }
    }

    /// A chunk was received and entered the queue.
    pub fn written(&mut self, length: usize) -> Option<bool> {
        self.pending += length;
        if !self.paused && self.pending > self.high {
            self.paused = true;
            return Some(true);
        }
        None
    }

    /// The emulator finished processing part of the queue.
    pub fn processed(&mut self, length: usize) -> Option<bool> {
        self.pending = self.pending.saturating_sub(length);
        if self.paused && self.pending < self.low {
            self.paused = false;
            return Some(false);
        }
        None
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn pending(&self) -> usize {
        self.pending
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
            self.calls.extend(self.flow.written(length));
        }

        fn processed(&mut self, length: usize) {
            self.calls.extend(self.flow.processed(length));
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

    #[test]
    #[should_panic(expected = "low watermark")]
    fn rejects_inverted_configuration() {
        let _ = FlowControl::new(10, 20);
    }
}
