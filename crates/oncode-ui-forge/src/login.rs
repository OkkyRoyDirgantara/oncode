//! Device flow login polling as a pure state machine. Kept separate from the dialog so it can be
//! tested without a window and without real time.

use std::time::Duration;

use oncode_core::api::forge::LoginPoll;
use oncode_core::AppError;
use oncode_forge::{Account, ForgeKind};

/// The server asks for the interval to grow by five seconds each time it answers `slow_down` (RFC 8628 §3.5).
pub const SLOW_DOWN_SECONDS: u32 = 5;
const MIN_INTERVAL_SECONDS: u32 = 1;

/// The end of a login attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginOutcome {
    Done(Account),
    Denied,
    Expired,
    Error(AppError),
}

/// What to do after one polling answer.
#[derive(Debug, Clone, PartialEq)]
pub enum PollNext {
    /// Wait one interval, then poll again. `retrying`: the last poll failed because of the network.
    Continue { retrying: bool },
    Finish(LoginOutcome),
}

/// Polling state for one device code.
///
/// Usage: wait [`DevicePoller::next_delay`], check [`DevicePoller::is_expired`], poll,
/// then hand the answer to [`DevicePoller::on_answer`]. The first poll happens
/// after one interval, not immediately (per the device flow rules).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePoller {
    interval_seconds: u32,
    expires_in: Duration,
    attempts: u32,
}

impl DevicePoller {
    /// `interval`: the initial polling interval from the server (seconds). `expires_in`: how long the code is valid (seconds).
    pub fn new(interval: u32, expires_in: u32) -> Self {
        Self { interval_seconds: interval.max(MIN_INTERVAL_SECONDS), expires_in: Duration::from_secs(expires_in as u64), attempts: 0 }
    }

    /// The current polling interval, in seconds.
    pub fn interval_seconds(&self) -> u32 {
        self.interval_seconds
    }

    /// Number of polls that have been answered.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Whether the code has expired once `elapsed` has passed since the login started.
    pub fn is_expired(&self, elapsed: Duration) -> bool {
        elapsed >= self.expires_in
    }

    /// How long to wait before the next poll; `None` if the code has expired.
    /// Never goes past the expiry deadline.
    pub fn next_delay(&self, elapsed: Duration) -> Option<Duration> {
        let remaining = self.expires_in.checked_sub(elapsed).filter(|r| !r.is_zero())?;
        Some(remaining.min(Duration::from_secs(self.interval_seconds as u64)))
    }

    /// Handles one polling answer.
    pub fn on_answer(&mut self, answer: Result<LoginPoll, AppError>) -> PollNext {
        self.attempts += 1;
        match answer {
            Ok(LoginPoll::Pending) => PollNext::Continue { retrying: false },
            Ok(LoginPoll::SlowDown) => {
                self.interval_seconds += SLOW_DOWN_SECONDS;
                PollNext::Continue { retrying: false }
            }
            Ok(LoginPoll::Denied) => PollNext::Finish(LoginOutcome::Denied),
            Ok(LoginPoll::Expired) => PollNext::Finish(LoginOutcome::Expired),
            Ok(LoginPoll::Done { account }) => PollNext::Finish(LoginOutcome::Done(account)),
            // A network error does not abort the login; try again at the next interval.
            Err(error) if error.code == "network" => PollNext::Continue { retrying: true },
            Err(error) => PollNext::Finish(LoginOutcome::Error(error)),
        }
    }
}

/// The token scopes needed, to show in the login dialog.
pub fn token_scopes(kind: ForgeKind) -> &'static [&'static str] {
    match kind {
        ForgeKind::GitHub => &["repo", "read:user"],
        ForgeKind::GitLab => &["api", "read_user"],
    }
}

/// A sentence listing the scopes, e.g. "scopes repo and read:user".
pub fn scopes_sentence(kind: ForgeKind) -> String {
    let scopes = token_scopes(kind);
    let mut text = String::from(if scopes.len() == 1 { "scope " } else { "scopes " });
    for (index, scope) in scopes.iter().enumerate() {
        if index > 0 {
            text.push_str(if index == scopes.len() - 1 { " and " } else { ", " });
        }
        text.push_str(scope);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn account() -> Account {
        Account {
            id: "github:github.com:octocat".into(),
            kind: ForgeKind::GitHub,
            host: "https://github.com".into(),
            username: "octocat".into(),
            display_name: "The Octocat".into(),
            avatar_url: None,
        }
    }

    /// Runs polling with a fake clock until finished or until `until` seconds.
    /// Answers come from a queue; once it runs out, always `Pending`. Returns when each
    /// poll happened (seconds) and the final result.
    fn run(poller: &mut DevicePoller, answers: Vec<Result<LoginPoll, AppError>>, until: u64) -> (Vec<u64>, Option<LoginOutcome>) {
        let mut queue: VecDeque<_> = answers.into();
        let mut now = Duration::ZERO;
        let mut polls = Vec::new();
        loop {
            let Some(delay) = poller.next_delay(now) else { return (polls, Some(LoginOutcome::Expired)) };
            if (now + delay).as_secs() > until {
                return (polls, None);
            }
            now += delay;
            if poller.is_expired(now) {
                return (polls, Some(LoginOutcome::Expired));
            }
            polls.push(now.as_secs());
            match poller.on_answer(queue.pop_front().unwrap_or(Ok(LoginPoll::Pending))) {
                PollNext::Continue { .. } => {}
                PollNext::Finish(outcome) => return (polls, Some(outcome)),
            }
        }
    }

    #[test]
    fn waits_one_interval_before_the_first_poll_and_finishes_on_done() {
        let mut poller = DevicePoller::new(5, 900);
        assert_eq!(poller.next_delay(Duration::ZERO), Some(Duration::from_secs(5)));
        let (polls, outcome) = run(&mut poller, vec![Ok(LoginPoll::Pending), Ok(LoginPoll::Done { account: account() })], 60);
        assert_eq!(polls, [5, 10]);
        assert_eq!(outcome, Some(LoginOutcome::Done(account())));
        assert_eq!(poller.attempts(), 2);
    }

    #[test]
    fn slow_down_adds_five_seconds_to_every_following_wait() {
        let mut poller = DevicePoller::new(5, 900);
        let (polls, outcome) = run(&mut poller, vec![Ok(LoginPoll::SlowDown), Ok(LoginPoll::Pending), Ok(LoginPoll::SlowDown)], 40);
        // 5 s, then 10 s twice, then 15 s.
        assert_eq!(polls, [5, 15, 25, 40]);
        assert_eq!(outcome, None);
        assert_eq!(poller.interval_seconds(), 5 + 2 * SLOW_DOWN_SECONDS);
    }

    #[test]
    fn stops_when_denied_or_when_the_server_says_expired() {
        let (polls, outcome) = run(&mut DevicePoller::new(5, 900), vec![Ok(LoginPoll::Denied)], 600);
        assert_eq!((polls, outcome), (vec![5], Some(LoginOutcome::Denied)));
        let (polls, outcome) = run(&mut DevicePoller::new(5, 900), vec![Ok(LoginPoll::Pending), Ok(LoginPoll::Expired)], 600);
        assert_eq!((polls, outcome), (vec![5, 10], Some(LoginOutcome::Expired)));
    }

    #[test]
    fn expires_by_itself_without_a_poll_past_the_deadline() {
        let mut poller = DevicePoller::new(5, 12);
        // Deadline 12 s: polls at 5 and 10, then waits only the remaining 2 s and stops.
        assert_eq!(poller.next_delay(Duration::from_secs(10)), Some(Duration::from_secs(2)));
        assert_eq!(poller.next_delay(Duration::from_secs(12)), None);
        assert!(poller.is_expired(Duration::from_secs(12)) && !poller.is_expired(Duration::from_secs(11)));
        let (polls, outcome) = run(&mut poller, vec![], 600);
        assert_eq!((polls, outcome), (vec![5, 10], Some(LoginOutcome::Expired)));
    }

    #[test]
    fn network_errors_are_retried_and_other_errors_stop() {
        let mut poller = DevicePoller::new(5, 900);
        assert_eq!(poller.on_answer(Err(AppError::new("network", "offline"))), PollNext::Continue { retrying: true });
        assert_eq!(poller.interval_seconds(), 5);
        let error = AppError::new("unauthorized", "bad client");
        assert_eq!(poller.on_answer(Err(error.clone())), PollNext::Finish(LoginOutcome::Error(error)));
        assert_eq!(poller.attempts(), 2);
    }

    #[test]
    fn interval_is_at_least_one_second() {
        let poller = DevicePoller::new(0, 60);
        assert_eq!(poller.interval_seconds(), 1);
        assert_eq!(poller.next_delay(Duration::ZERO), Some(Duration::from_secs(1)));
        assert_eq!(DevicePoller::new(5, 0).next_delay(Duration::ZERO), None);
    }

    #[test]
    fn scopes_are_listed_per_provider() {
        assert_eq!(token_scopes(ForgeKind::GitHub), ["repo", "read:user"]);
        assert_eq!(scopes_sentence(ForgeKind::GitHub), "scopes repo and read:user");
        assert_eq!(scopes_sentence(ForgeKind::GitLab), "scopes api and read_user");
    }
}
