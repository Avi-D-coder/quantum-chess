//! Room actions as data, so a host can log them and rebuild a room by replay.
//!
//! [`Room`] is deterministic given the time of each call, so a room is fully
//! described by its [`RoomInit`](crate::RoomInit), its creation time and the
//! list of [`Logged`] actions that changed it. Hosts that cannot keep a room
//! in memory (a Cloudflare Durable Object that hibernates, a browser tab that
//! reloads) store that list and replay it with [`Room::replay`].

use crate::room::{Event, Room, RoomError};

/// Something a player (or the clock) does to a room.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "k", rename_all = "snake_case"))]
pub enum Action {
    /// [`Room::play`].
    Move {
        /// Player id.
        u: i64,
        /// Move notation, possibly ending in a collapse choice.
        n: String,
    },
    /// [`Room::resign`].
    Resign {
        /// Player id.
        u: i64,
    },
    /// [`Room::abort`].
    Abort {
        /// Player id.
        u: i64,
    },
    /// [`Room::offer_draw`].
    OfferDraw {
        /// Player id.
        u: i64,
    },
    /// [`Room::accept_draw`].
    AcceptDraw {
        /// Player id.
        u: i64,
    },
    /// [`Room::decline_draw`].
    DeclineDraw {
        /// Player id.
        u: i64,
    },
    /// [`Room::chat`].
    Chat {
        /// Player id.
        u: i64,
        /// Display name.
        name: String,
        /// Message.
        text: String,
    },
    /// [`Room::tick`]: flags a player who ran out of time.
    Tick,
}

/// An action and when it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Logged {
    /// Epoch milliseconds.
    pub t: u64,
    /// What happened.
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub a: Action,
}

impl Room {
    /// Applies `action` at `now_ms`.
    pub fn apply(&mut self, action: &Action, now_ms: u64) -> Result<Vec<Event>, RoomError> {
        match action {
            Action::Move { u, n } => self.play(*u, n, now_ms),
            Action::Resign { u } => self.resign(*u, now_ms),
            Action::Abort { u } => self.abort(*u, now_ms),
            Action::OfferDraw { u } => self.offer_draw(*u),
            Action::AcceptDraw { u } => self.accept_draw(*u, now_ms),
            Action::DeclineDraw { u } => self.decline_draw(*u),
            Action::Chat { u, name, text } => self.chat(*u, name, text, now_ms),
            Action::Tick => Ok(self.tick(now_ms)),
        }
    }

    /// Replays a log made of actions that [`changed`] the room.
    pub fn replay(&mut self, log: &[Logged]) {
        for l in log {
            let _ = self.apply(&l.a, l.t);
        }
    }
}

/// Whether applying an action produced these events changed the room, so
/// the action belongs in the log. A move that only asks for a collapse
/// choice changes nothing.
pub fn changed(events: &[Event]) -> bool {
    events.iter().any(|e| !matches!(e, Event::NeedsCollapseChoice { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlayerInfo, TimeControl};

    fn room() -> Room {
        let p = |id, name: &str| PlayerInfo { user_id: id, username: name.into(), rating: None };
        Room::new("g", p(1, "a"), p(2, "b"), TimeControl { initial_ms: 60_000, increment_ms: 0 }, false, 0)
    }

    #[test]
    fn replay_rebuilds_the_same_room() {
        let mut live = room();
        let mut log = Vec::new();
        let script = [
            (1_000, Action::Move { u: 1, n: "e2e4".into() }),
            (2_000, Action::Chat { u: 2, name: "b".into(), text: "hi".into() }),
            (3_000, Action::Move { u: 2, n: "Ng8f6/h6".into() }),
            (4_000, Action::OfferDraw { u: 1 }),
            (5_000, Action::DeclineDraw { u: 2 }),
            (6_000, Action::Move { u: 1, n: "d2d4".into() }),
            (200_000, Action::Tick),
        ];
        for (t, a) in script {
            let ev = live.apply(&a, t).unwrap();
            if changed(&ev) {
                log.push(Logged { t, a });
            }
        }
        assert!(live.is_over(), "black flagged");
        let mut again = room();
        again.replay(&log);
        assert_eq!(again.snapshot(Some(1), 300_000), live.snapshot(Some(1), 300_000));
        assert_eq!(again.chat_log(), live.chat_log());
    }
}
