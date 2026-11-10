//! Open seeks and friend challenges.

use std::fmt;

use glicko2::Rating;

use crate::room::PlayerInfo;
use crate::time::TimeControl;

/// Challenges expire this long after creation (24 hours).
pub const CHALLENGE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// Which color the creator of a seek or challenge wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ColorPref {
    /// Decided by a coin flip when the game starts.
    Random,
    /// The creator plays white.
    White,
    /// The creator plays black.
    Black,
}

impl ColorPref {
    /// `"random"`, `"white"` or `"black"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorPref::Random => "random",
            ColorPref::White => "white",
            ColorPref::Black => "black",
        }
    }

    /// Parses the strings produced by [`as_str`](Self::as_str).
    pub fn parse(s: &str) -> Option<ColorPref> {
        match s {
            "random" => Some(ColorPref::Random),
            "white" => Some(ColorPref::White),
            "black" => Some(ColorPref::Black),
            _ => None,
        }
    }
}

/// Everything [`Room::new`](crate::Room::new) needs.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RoomInit {
    /// Id for the new game.
    pub game_id: String,
    /// White player.
    pub white: PlayerInfo,
    /// Black player.
    pub black: PlayerInfo,
    /// Time control.
    pub tc: TimeControl,
    /// Whether the game is rated.
    pub rated: bool,
}

impl RoomInit {
    /// Builds the room, created at `now_ms`.
    pub fn into_room(self, now_ms: u64) -> crate::Room {
        crate::Room::new(self.game_id, self.white, self.black, self.tc, self.rated, now_ms)
    }
}

/// Why a lobby operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyError {
    /// No such seek or challenge (never existed, taken, cancelled or expired).
    NotFound,
    /// You cannot accept your own seek or challenge.
    OwnSeek,
    /// Only the creator can cancel.
    NotOwner,
}

impl fmt::Display for LobbyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LobbyError::NotFound => "no such seek or challenge",
            LobbyError::OwnSeek => "you cannot accept your own seek or challenge",
            LobbyError::NotOwner => "only the creator can cancel",
        })
    }
}

impl std::error::Error for LobbyError {}

/// An open invitation to play, visible in the lobby.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Seek {
    /// Seek id (chosen by the caller).
    pub id: String,
    /// The seeker's account id.
    pub user_id: i64,
    /// The seeker's name.
    pub username: String,
    /// Display rating (`None` shows as unrated/"?").
    pub rating: Option<f64>,
    /// Full rating used to seed the game when accepted.
    pub player_rating: Option<Rating>,
    /// Time control.
    pub tc: TimeControl,
    /// Rated or casual.
    pub rated: bool,
    /// Color preference.
    pub color: ColorPref,
    /// Creation time (epoch ms).
    pub created_ms: u64,
}

/// A private challenge reachable only through its token (a link).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Challenge {
    /// Secret token (chosen by the caller).
    pub token: String,
    /// The challenger.
    pub creator: PlayerInfo,
    /// Time control.
    pub tc: TimeControl,
    /// Rated or casual.
    pub rated: bool,
    /// Challenger's color preference.
    pub color: ColorPref,
    /// Creation time (epoch ms).
    pub created_ms: u64,
}

/// Open seeks and pending friend challenges.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Lobby {
    seeks: Vec<Seek>,
    challenges: Vec<Challenge>,
}

/// Decides colors: returns `(white, black)` given the creator's preference.
/// With `Random`, `coin == true` gives the creator white.
fn assign(
    creator: PlayerInfo,
    acceptor: PlayerInfo,
    pref: ColorPref,
    coin: bool,
) -> (PlayerInfo, PlayerInfo) {
    let creator_white = match pref {
        ColorPref::White => true,
        ColorPref::Black => false,
        ColorPref::Random => coin,
    };
    if creator_white { (creator, acceptor) } else { (acceptor, creator) }
}

impl Lobby {
    /// An empty lobby.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a seek. A user has at most one seek per time control: an existing
    /// one with the same time control is replaced and returned.
    pub fn add_seek(&mut self, seek: Seek) -> Option<Seek> {
        let old = self
            .seeks
            .iter()
            .position(|s| s.user_id == seek.user_id && s.tc == seek.tc)
            .map(|i| self.seeks.remove(i));
        self.seeks.push(seek);
        old
    }

    /// Removes a seek owned by `user_id`.
    pub fn cancel_seek(&mut self, user_id: i64, id: &str) -> Result<Seek, LobbyError> {
        let i = self.seeks.iter().position(|s| s.id == id).ok_or(LobbyError::NotFound)?;
        if self.seeks[i].user_id != user_id {
            return Err(LobbyError::NotOwner);
        }
        Ok(self.seeks.remove(i))
    }

    /// Removes all of a user's seeks (e.g. on disconnect) and returns them.
    pub fn remove_user_seeks(&mut self, user_id: i64) -> Vec<Seek> {
        let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.seeks)
            .into_iter()
            .partition(|s| s.user_id == user_id);
        self.seeks = keep;
        gone
    }

    /// Looks up a seek.
    pub fn get_seek(&self, id: &str) -> Option<&Seek> {
        self.seeks.iter().find(|s| s.id == id)
    }

    /// Accepts a seek. Both players' remaining seeks are removed. With a
    /// `Random` preference, `coin == true` gives the seeker white.
    pub fn accept_seek(
        &mut self,
        acceptor: PlayerInfo,
        id: &str,
        game_id: String,
        coin: bool,
    ) -> Result<RoomInit, LobbyError> {
        let i = self.seeks.iter().position(|s| s.id == id).ok_or(LobbyError::NotFound)?;
        if self.seeks[i].user_id == acceptor.user_id {
            return Err(LobbyError::OwnSeek);
        }
        let seek = self.seeks.remove(i);
        self.remove_user_seeks(seek.user_id);
        self.remove_user_seeks(acceptor.user_id);
        let creator = PlayerInfo { user_id: seek.user_id, username: seek.username, rating: seek.player_rating };
        let (white, black) = assign(creator, acceptor, seek.color, coin);
        Ok(RoomInit { game_id, white, black, tc: seek.tc, rated: seek.rated })
    }

    /// Open seeks, newest first (ties broken by id).
    pub fn list(&self) -> Vec<&Seek> {
        let mut v: Vec<&Seek> = self.seeks.iter().collect();
        v.sort_by(|a, b| b.created_ms.cmp(&a.created_ms).then_with(|| a.id.cmp(&b.id)));
        v
    }

    /// Creates a friend challenge reachable by `token`.
    pub fn create_challenge(
        &mut self,
        creator: PlayerInfo,
        tc: TimeControl,
        rated: bool,
        color: ColorPref,
        token: String,
        now_ms: u64,
    ) {
        self.challenges.push(Challenge { token, creator, tc, rated, color, created_ms: now_ms });
    }

    /// Looks up a challenge by token (expired ones linger until `prune`).
    pub fn get_challenge(&self, token: &str) -> Option<&Challenge> {
        self.challenges.iter().find(|c| c.token == token)
    }

    /// Accepts a challenge (single use). Expired challenges are removed and
    /// reported as `NotFound`. With `Random`, `coin == true` gives the
    /// challenger white.
    pub fn accept_challenge(
        &mut self,
        acceptor: PlayerInfo,
        token: &str,
        game_id: String,
        coin: bool,
        now_ms: u64,
    ) -> Result<RoomInit, LobbyError> {
        let i = self.challenges.iter().position(|c| c.token == token).ok_or(LobbyError::NotFound)?;
        if now_ms.saturating_sub(self.challenges[i].created_ms) >= CHALLENGE_TTL_MS {
            self.challenges.remove(i);
            return Err(LobbyError::NotFound);
        }
        if self.challenges[i].creator.user_id == acceptor.user_id {
            return Err(LobbyError::OwnSeek);
        }
        let c = self.challenges.remove(i);
        let (white, black) = assign(c.creator, acceptor, c.color, coin);
        Ok(RoomInit { game_id, white, black, tc: c.tc, rated: c.rated })
    }

    /// Withdraws a challenge created by `user_id`.
    pub fn cancel_challenge(&mut self, user_id: i64, token: &str) -> Result<Challenge, LobbyError> {
        let i = self.challenges.iter().position(|c| c.token == token).ok_or(LobbyError::NotFound)?;
        if self.challenges[i].creator.user_id != user_id {
            return Err(LobbyError::NotOwner);
        }
        Ok(self.challenges.remove(i))
    }

    /// Drops challenges older than 24 hours; returns how many were removed.
    pub fn prune(&mut self, now_ms: u64) -> usize {
        let before = self.challenges.len();
        self.challenges.retain(|c| now_ms.saturating_sub(c.created_ms) < CHALLENGE_TTL_MS);
        before - self.challenges.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: i64, name: &str) -> PlayerInfo {
        PlayerInfo { user_id: id, username: name.into(), rating: None }
    }

    fn seek(id: &str, user: i64, tc: TimeControl, color: ColorPref, at: u64) -> Seek {
        Seek {
            id: id.into(),
            user_id: user,
            username: format!("u{user}"),
            rating: Some(1500.0),
            player_rating: Some(Rating::new(1500.0, 80.0, 0.06)),
            tc,
            rated: true,
            color,
            created_ms: at,
        }
    }

    const T32: TimeControl = TimeControl::new(3, 2);
    const T50: TimeControl = TimeControl::new(5, 0);

    #[test]
    fn replace_same_tc_keeps_other_tc() {
        let mut l = Lobby::new();
        assert!(l.add_seek(seek("a", 1, T32, ColorPref::Random, 1)).is_none());
        assert!(l.add_seek(seek("b", 1, T50, ColorPref::Random, 2)).is_none());
        let old = l.add_seek(seek("c", 1, T32, ColorPref::White, 3)).unwrap();
        assert_eq!(old.id, "a");
        assert_eq!(l.list().len(), 2);
        assert!(l.get_seek("a").is_none());
        assert!(l.get_seek("c").is_some());
    }

    #[test]
    fn list_newest_first() {
        let mut l = Lobby::new();
        l.add_seek(seek("old", 1, T32, ColorPref::Random, 10));
        l.add_seek(seek("new", 2, T32, ColorPref::Random, 30));
        l.add_seek(seek("mid", 3, T32, ColorPref::Random, 20));
        let ids: Vec<_> = l.list().iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, ["new", "mid", "old"]);
    }

    #[test]
    fn cancel() {
        let mut l = Lobby::new();
        l.add_seek(seek("a", 1, T32, ColorPref::Random, 1));
        assert_eq!(l.cancel_seek(2, "a").unwrap_err(), LobbyError::NotOwner);
        assert_eq!(l.cancel_seek(1, "zzz").unwrap_err(), LobbyError::NotFound);
        assert_eq!(l.cancel_seek(1, "a").unwrap().id, "a");
        assert!(l.list().is_empty());
    }

    #[test]
    fn remove_user_seeks_only_theirs() {
        let mut l = Lobby::new();
        l.add_seek(seek("a", 1, T32, ColorPref::Random, 1));
        l.add_seek(seek("b", 1, T50, ColorPref::Random, 2));
        l.add_seek(seek("c", 2, T50, ColorPref::Random, 3));
        assert_eq!(l.remove_user_seeks(1).len(), 2);
        assert_eq!(l.list().len(), 1);
    }

    #[test]
    fn accept_rejects_own_and_missing() {
        let mut l = Lobby::new();
        l.add_seek(seek("a", 1, T32, ColorPref::Random, 1));
        assert_eq!(l.accept_seek(p(1, "u1"), "a", "g".into(), true).unwrap_err(), LobbyError::OwnSeek);
        assert_eq!(l.accept_seek(p(2, "u2"), "x", "g".into(), true).unwrap_err(), LobbyError::NotFound);
        assert!(l.get_seek("a").is_some());
    }

    #[test]
    fn accept_color_preferences() {
        for (pref, coin, seeker_white) in [
            (ColorPref::White, false, true),
            (ColorPref::White, true, true),
            (ColorPref::Black, true, false),
            (ColorPref::Black, false, false),
            (ColorPref::Random, true, true),
            (ColorPref::Random, false, false),
        ] {
            let mut l = Lobby::new();
            l.add_seek(seek("a", 1, T32, pref, 1));
            let init = l.accept_seek(p(2, "u2"), "a", "gid".into(), coin).unwrap();
            assert_eq!(init.game_id, "gid");
            assert_eq!(init.tc, T32);
            assert!(init.rated);
            let (seeker, other) = if seeker_white { (&init.white, &init.black) } else { (&init.black, &init.white) };
            assert_eq!(seeker.user_id, 1);
            assert_eq!(other.user_id, 2);
            assert_eq!(seeker.rating.unwrap().deviation, 80.0);
            assert!(l.list().is_empty(), "seek is consumed");
        }
    }

    #[test]
    fn accept_clears_both_users_seeks() {
        let mut l = Lobby::new();
        l.add_seek(seek("a", 1, T32, ColorPref::Random, 1));
        l.add_seek(seek("b", 1, T50, ColorPref::Random, 2));
        l.add_seek(seek("c", 2, T50, ColorPref::Random, 3));
        l.add_seek(seek("d", 3, T50, ColorPref::Random, 4));
        l.accept_seek(p(2, "u2"), "a", "g".into(), false).unwrap();
        let ids: Vec<_> = l.list().iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, ["d"]);
    }

    #[test]
    fn challenge_single_use() {
        let mut l = Lobby::new();
        l.create_challenge(p(1, "a"), T32, false, ColorPref::Black, "tok".into(), 1000);
        assert_eq!(l.get_challenge("tok").unwrap().creator.user_id, 1);
        assert_eq!(
            l.accept_challenge(p(1, "a"), "tok", "g".into(), true, 2000).unwrap_err(),
            LobbyError::OwnSeek
        );
        let init = l.accept_challenge(p(2, "b"), "tok", "g1".into(), true, 2000).unwrap();
        assert_eq!(init.black.user_id, 1);
        assert_eq!(init.white.user_id, 2);
        assert!(!init.rated);
        assert_eq!(
            l.accept_challenge(p(3, "c"), "tok", "g2".into(), true, 2000).unwrap_err(),
            LobbyError::NotFound
        );
        assert!(l.get_challenge("tok").is_none());
    }

    #[test]
    fn challenge_random_and_cancel() {
        let mut l = Lobby::new();
        l.create_challenge(p(1, "a"), T32, true, ColorPref::Random, "t1".into(), 0);
        l.create_challenge(p(1, "a"), T32, true, ColorPref::Random, "t2".into(), 0);
        assert_eq!(l.cancel_challenge(2, "t1").unwrap_err(), LobbyError::NotOwner);
        l.cancel_challenge(1, "t1").unwrap();
        assert_eq!(l.cancel_challenge(1, "t1").unwrap_err(), LobbyError::NotFound);
        let init = l.accept_challenge(p(2, "b"), "t2", "g".into(), false, 5).unwrap();
        assert_eq!(init.white.user_id, 2);
    }

    #[test]
    fn challenge_expiry() {
        let mut l = Lobby::new();
        l.create_challenge(p(1, "a"), T32, true, ColorPref::Random, "old".into(), 0);
        l.create_challenge(p(1, "a"), T32, true, ColorPref::Random, "new".into(), CHALLENGE_TTL_MS);
        assert_eq!(l.prune(CHALLENGE_TTL_MS - 1), 0);
        assert_eq!(l.prune(CHALLENGE_TTL_MS), 1);
        assert!(l.get_challenge("old").is_none());
        assert!(l.get_challenge("new").is_some());
        // accept also refuses expired ones even if not yet pruned
        assert_eq!(
            l.accept_challenge(p(2, "b"), "new", "g".into(), true, 2 * CHALLENGE_TTL_MS).unwrap_err(),
            LobbyError::NotFound
        );
        assert!(l.get_challenge("new").is_none());
    }

    #[test]
    fn room_init_builds_room() {
        let mut l = Lobby::new();
        l.add_seek(seek("a", 1, T32, ColorPref::White, 1));
        let room = l.accept_seek(p(2, "u2"), "a", "gid".into(), false).unwrap().into_room(77);
        assert_eq!(room.id(), "gid");
        assert_eq!(room.white().user_id, 1);
        assert_eq!(room.created_ms(), 77);
    }

    #[test]
    fn color_pref_strings() {
        for c in [ColorPref::Random, ColorPref::White, ColorPref::Black] {
            assert_eq!(ColorPref::parse(c.as_str()), Some(c));
        }
        assert_eq!(ColorPref::parse("x"), None);
    }
}
