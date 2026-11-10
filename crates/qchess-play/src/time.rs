//! Time controls and the game clock.

use crate::Color;

/// A clock setting: initial time and per-move increment, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeControl {
    /// Initial time for each side.
    pub initial_ms: u64,
    /// Time added to the mover's clock after each (clocked) move.
    pub increment_ms: u64,
}

const fn min_sec(minutes: u64, inc_secs: u64) -> TimeControl {
    TimeControl { initial_ms: minutes * 60_000, increment_ms: inc_secs * 1000 }
}

/// The time controls offered by the site: 1+0, 2+1, 3+0, 3+2, 5+0, 5+3,
/// 10+0, 10+5, 15+10 and 30+0 (minutes + increment seconds).
pub const PRESETS: [TimeControl; 10] = [
    min_sec(1, 0),
    min_sec(2, 1),
    min_sec(3, 0),
    min_sec(3, 2),
    min_sec(5, 0),
    min_sec(5, 3),
    min_sec(10, 0),
    min_sec(10, 5),
    min_sec(15, 10),
    min_sec(30, 0),
];

impl TimeControl {
    /// Creates a time control from minutes and increment seconds.
    pub const fn new(minutes: u64, increment_secs: u64) -> Self {
        min_sec(minutes, increment_secs)
    }

    /// Rating speed category (lichess estimate `initial + 40 * increment`).
    pub fn speed(&self) -> glicko2::Speed {
        let secs = |ms: u64| u32::try_from(ms / 1000).unwrap_or(u32::MAX);
        glicko2::Speed::from_clock(secs(self.initial_ms), secs(self.increment_ms))
    }

    /// Label such as `"3+2"`. Initial times that are not whole minutes are
    /// shown in seconds, e.g. `"30s+0"`.
    pub fn label(&self) -> String {
        let inc = self.increment_ms / 1000;
        if self.initial_ms.is_multiple_of(60_000) {
            format!("{}+{}", self.initial_ms / 60_000, inc)
        } else {
            format!("{}s+{}", self.initial_ms / 1000, inc)
        }
    }

    /// Looks up a preset by its [`label`](Self::label), e.g. `"5+3"`.
    pub fn from_label(label: &str) -> Option<TimeControl> {
        PRESETS.iter().copied().find(|tc| tc.label() == label)
    }

    /// Whether this is one of the [`PRESETS`].
    pub fn is_preset(&self) -> bool {
        PRESETS.contains(self)
    }
}

/// Returned by [`Clock::press`] when the mover ran out of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag(pub Color);

/// A two-sided chess clock driven by explicit timestamps.
///
/// The clock is idle until [`start`](Clock::start) is called. The room starts
/// it only after both sides have made their first move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    white_ms: u64,
    black_ms: u64,
    increment_ms: u64,
    running: Option<Color>,
    since_ms: u64,
}

/// Clock numbers as sent to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockView {
    /// Initial time per side.
    pub initial_ms: u64,
    /// Increment per move.
    pub increment_ms: u64,
    /// White's remaining time as of `server_now_ms`.
    pub white_ms: u64,
    /// Black's remaining time as of `server_now_ms`.
    pub black_ms: u64,
    /// The side whose clock is ticking, if any.
    pub running: Option<Color>,
    /// The timestamp the numbers were computed at.
    pub server_now_ms: u64,
}

impl Clock {
    /// A stopped clock with the full initial time on both sides.
    pub fn new(tc: TimeControl) -> Self {
        Clock {
            white_ms: tc.initial_ms,
            black_ms: tc.initial_ms,
            increment_ms: tc.increment_ms,
            running: None,
            since_ms: 0,
        }
    }

    fn stored(&self, side: Color) -> u64 {
        match side {
            Color::White => self.white_ms,
            Color::Black => self.black_ms,
        }
    }

    fn stored_mut(&mut self, side: Color) -> &mut u64 {
        match side {
            Color::White => &mut self.white_ms,
            Color::Black => &mut self.black_ms,
        }
    }

    /// Starts `side`'s clock at `now_ms`.
    pub fn start(&mut self, side: Color, now_ms: u64) {
        self.running = Some(side);
        self.since_ms = now_ms;
    }

    /// The side whose clock is running.
    pub fn running(&self) -> Option<Color> {
        self.running
    }

    /// Ends the running side's turn at `now_ms`: deducts the elapsed time,
    /// adds the increment and starts the other side. A stopped clock is left
    /// alone. If the mover had no time left the clock stops with that side
    /// at zero and `Err(Flag(side))` is returned.
    pub fn press(&mut self, now_ms: u64) -> Result<(), Flag> {
        let Some(side) = self.running else { return Ok(()) };
        let elapsed = now_ms.saturating_sub(self.since_ms);
        let left = self.stored(side);
        if elapsed >= left {
            *self.stored_mut(side) = 0;
            self.running = None;
            return Err(Flag(side));
        }
        *self.stored_mut(side) = left - elapsed + self.increment_ms;
        self.running = Some(side.other());
        self.since_ms = now_ms;
        Ok(())
    }

    /// Time left for `side` at `now_ms`, accounting for a running clock.
    pub fn remaining(&self, side: Color, now_ms: u64) -> u64 {
        let base = self.stored(side);
        if self.running == Some(side) {
            base.saturating_sub(now_ms.saturating_sub(self.since_ms))
        } else {
            base
        }
    }

    /// The side that has run out of time at `now_ms`, if any.
    pub fn flagged(&self, now_ms: u64) -> Option<Color> {
        self.running.filter(|&s| self.remaining(s, now_ms) == 0)
    }

    /// Stops the clock at `now_ms`, charging the running side for the time
    /// elapsed (no increment).
    pub fn stop(&mut self, now_ms: u64) {
        if let Some(side) = self.running {
            *self.stored_mut(side) = self.remaining(side, now_ms);
        }
        self.running = None;
    }

    /// The epoch time at which the running side will flag, if the clock runs.
    pub fn flag_at_ms(&self) -> Option<u64> {
        self.running.map(|s| self.since_ms.saturating_add(self.stored(s)))
    }

    /// A snapshot of the numbers for clients.
    pub fn view(&self, tc: TimeControl, now_ms: u64) -> ClockView {
        ClockView {
            initial_ms: tc.initial_ms,
            increment_ms: tc.increment_ms,
            white_ms: self.remaining(Color::White, now_ms),
            black_ms: self.remaining(Color::Black, now_ms),
            running: self.running,
            server_now_ms: now_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_presets() {
        let labels: Vec<_> = PRESETS.iter().map(|t| t.label()).collect();
        assert_eq!(labels, ["1+0", "2+1", "3+0", "3+2", "5+0", "5+3", "10+0", "10+5", "15+10", "30+0"]);
        assert_eq!(TimeControl::from_label("3+2"), Some(TimeControl::new(3, 2)));
        assert_eq!(TimeControl::from_label("4+4"), None);
        assert!(TimeControl::new(5, 3).is_preset());
        assert!(!TimeControl::new(7, 0).is_preset());
        assert_eq!(TimeControl { initial_ms: 30_000, increment_ms: 0 }.label(), "30s+0");
    }

    #[test]
    fn speeds() {
        use glicko2::Speed::*;
        assert_eq!(TimeControl::new(1, 0).speed(), Bullet);
        assert_eq!(TimeControl::new(3, 0).speed(), Blitz);
        assert_eq!(TimeControl::new(3, 2).speed(), Blitz);
        assert_eq!(TimeControl::new(10, 0).speed(), Rapid);
        assert_eq!(TimeControl::new(30, 0).speed(), Classical);
        assert_eq!(TimeControl::new(15, 10).speed(), Rapid);
    }

    #[test]
    fn press_deducts_and_adds_increment() {
        let mut c = Clock::new(TimeControl::new(3, 2));
        assert_eq!(c.running(), None);
        assert_eq!(c.remaining(Color::White, 99), 180_000);
        c.start(Color::White, 1000);
        assert_eq!(c.remaining(Color::White, 4000), 177_000);
        assert_eq!(c.remaining(Color::Black, 4000), 180_000);
        c.press(4000).unwrap();
        assert_eq!(c.remaining(Color::White, 9999), 179_000);
        assert_eq!(c.running(), Some(Color::Black));
        c.press(10_000).unwrap();
        assert_eq!(c.remaining(Color::Black, 10_000), 180_000 - 6000 + 2000);
    }

    #[test]
    fn flagging() {
        let mut c = Clock::new(TimeControl::new(1, 0));
        c.start(Color::White, 0);
        assert_eq!(c.flagged(59_999), None);
        assert_eq!(c.flagged(60_000), Some(Color::White));
        assert_eq!(c.flag_at_ms(), Some(60_000));
        assert_eq!(c.press(70_000), Err(Flag(Color::White)));
        assert_eq!(c.running(), None);
        assert_eq!(c.remaining(Color::White, 80_000), 0);
    }

    #[test]
    fn stop_freezes() {
        let mut c = Clock::new(TimeControl::new(1, 5));
        c.start(Color::Black, 100);
        c.stop(10_100);
        assert_eq!(c.remaining(Color::Black, 1_000_000), 50_000);
        assert_eq!(c.flagged(1_000_000), None);
        assert_eq!(c.flag_at_ms(), None);
        assert_eq!(c.press(5), Ok(()));
    }
}
