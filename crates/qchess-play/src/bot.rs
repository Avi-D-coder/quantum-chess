//! Computer opponents: difficulty levels and how long each may think.

use std::time::Duration;

use qchess::ai::Config;

/// A difficulty level of the computer opponent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Depth 1, with random blunders.
    Easy,
    /// Depth 2.
    Medium,
    /// Up to depth 6, with pruning.
    Hard,
}

impl Level {
    /// Every level, easiest first.
    pub const ALL: [Level; 3] = [Level::Easy, Level::Medium, Level::Hard];

    /// Parses [`key`](Self::key).
    pub fn parse(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.key() == s)
    }

    /// `"easy"`, `"medium"` or `"hard"`.
    pub fn key(self) -> &'static str {
        match self {
            Level::Easy => "easy",
            Level::Medium => "medium",
            Level::Hard => "hard",
        }
    }

    /// Username of this level's account.
    pub fn username(self) -> &'static str {
        match self {
            Level::Easy => "QBot-Easy",
            Level::Medium => "QBot-Medium",
            Level::Hard => "QBot-Hard",
        }
    }

    /// Search settings with `remaining_ms` on the bot's clock.
    pub fn config(self, remaining_ms: u64, increment_ms: u64) -> Config {
        // Pruning searches about ten times fewer nodes but is a little
        // weaker at the same depth, so only Hard uses it, to search deeper
        // (to depth 6, time permitting). Easy and Medium play as they did.
        let (depth, margin, cap_ms, pruning) = match self {
            // Shallow, and picks at random among moves up to 1.5 pawns
            // worse than its best, so it blunders now and then.
            Level::Easy => (1, 150, 500, false),
            Level::Medium => (2, 30, 1_500, false),
            Level::Hard => (6, 0, 4_000, true),
        };
        // Spend about a thirtieth of the clock plus most of the increment.
        let budget = (remaining_ms / 30 + increment_ms * 3 / 4).clamp(50, cap_ms);
        Config { depth, time: Some(Duration::from_millis(budget)), margin, pruning, ..Config::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_round_trip_and_respect_the_clock() {
        for l in Level::ALL {
            assert_eq!(Level::parse(l.key()), Some(l));
        }
        assert_eq!(Level::Hard.config(10 * 60_000, 0).time, Some(Duration::from_millis(4_000)));
        assert_eq!(Level::Hard.config(3_000, 0).time, Some(Duration::from_millis(100)));
        assert_eq!(Level::Easy.config(0, 0).time, Some(Duration::from_millis(50)));
    }
}
