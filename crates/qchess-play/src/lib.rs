//! # qchess-play
//!
//! Framework-independent game-server logic for a lichess-style quantum chess
//! site. Everything here is pure and deterministic: time is always passed in
//! as `now_ms` (epoch milliseconds) and randomness (ids, tokens, coin flips)
//! is supplied by the caller. A web layer is expected to wrap these types in
//! locks, timers and websockets.
//!
//! * [`TimeControl`] / [`Clock`] — clocks (see [`time`]).
//! * [`Room`] — one live game, with chat, draw offers and a clock (see [`room`]).
//! * [`Lobby`] — open seeks and friend challenges (see [`lobby`]).
#![deny(missing_docs)]

pub mod lobby;
pub mod room;
pub mod time;

pub use lobby::{Challenge, ColorPref, Lobby, LobbyError, RoomInit, Seek};
pub use qchess::Color;
pub use room::{
    ChatLine, Event, GameResult, MoveRecord, PlayerInfo, PlayerView, Room, RoomError, RoomStatus,
    StateView, StatusView, Termination,
};
pub use time::{Clock, ClockView, Flag, PRESETS, TimeControl};

/// Protocol string for a color: `"white"` or `"black"`.
pub fn color_str(c: Color) -> &'static str {
    c.name()
}

/// Protocol string for a check state: `"none"`, `"quantum"` or `"full"`.
pub fn check_str(c: qchess::Check) -> &'static str {
    match c {
        qchess::Check::None => "none",
        qchess::Check::Quantum => "quantum",
        qchess::Check::Full => "full",
    }
}
