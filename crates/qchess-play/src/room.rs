//! One live game: moves, clock, draw offers and chat.

use std::fmt;

use glicko2::{Config, Outcome, Rating};
use qchess::{Check, Game, PieceView, Status};

use crate::time::{Clock, ClockView, Flag, TimeControl};
use crate::{Color, check_str, color_str};

/// Maximum number of chat lines kept per room.
pub const CHAT_CAP: usize = 200;
/// Maximum chat message length in characters (after trimming).
pub const CHAT_MAX_CHARS: usize = 140;
/// Number of plies after which a game can no longer be aborted.
pub const ABORT_PLIES: usize = 2;

/// A participant in a game.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PlayerInfo {
    /// Account id.
    pub user_id: i64,
    /// Display name.
    pub username: String,
    /// Current rating for the game's speed, if the player has one.
    pub rating: Option<Rating>,
}

/// Who won.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameResult {
    /// White won (`1-0`).
    WhiteWins,
    /// Black won (`0-1`).
    BlackWins,
    /// Drawn (`1/2-1/2`). Also used as a placeholder for aborted games.
    Draw,
}

impl GameResult {
    /// Protocol string: `"1-0"`, `"0-1"` or `"1/2-1/2"`.
    pub fn as_str(self) -> &'static str {
        match self {
            GameResult::WhiteWins => "1-0",
            GameResult::BlackWins => "0-1",
            GameResult::Draw => "1/2-1/2",
        }
    }

    /// The result in which `winner` wins.
    pub fn win_for(winner: Color) -> GameResult {
        match winner {
            Color::White => GameResult::WhiteWins,
            Color::Black => GameResult::BlackWins,
        }
    }
}

/// How a game ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// The side to move is mated in every possible state.
    Checkmate,
    /// The side to move has no legal move in any state and is not mated.
    Stalemate,
    /// A player resigned.
    Resign,
    /// A player ran out of time.
    Timeout,
    /// Both players agreed to a draw.
    DrawAgreement,
    /// The game was aborted before it really started.
    Abort,
}

impl Termination {
    /// Protocol string: `"checkmate"`, `"stalemate"`, `"resign"`,
    /// `"timeout"`, `"draw"` (agreement) or `"abort"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Termination::Checkmate => "checkmate",
            Termination::Stalemate => "stalemate",
            Termination::Resign => "resign",
            Termination::Timeout => "timeout",
            Termination::DrawAgreement => "draw",
            Termination::Abort => "abort",
        }
    }
}

/// Whether a room is still being played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomStatus {
    /// Still in progress.
    Ongoing,
    /// Finished.
    Ended {
        /// The result.
        result: GameResult,
        /// How it ended.
        termination: Termination,
    },
}

/// One played move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveRecord {
    /// 1-based ply number.
    pub ply: usize,
    /// The move as entered (including any `{...}` collapse choice).
    pub notation: String,
    /// The move with numbered pieces for display, e.g. `P₅e3/e4`.
    pub display: String,
    /// Human-readable collapses the move forced.
    pub collapses: Vec<String>,
    /// White's remaining time after the move.
    pub white_ms: u64,
    /// Black's remaining time after the move.
    pub black_ms: u64,
    /// When the move was played (epoch ms).
    pub at_ms: u64,
}

/// A chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    /// Author's account id.
    pub user_id: i64,
    /// Author's display name.
    pub username: String,
    /// Trimmed message text.
    pub text: String,
    /// When it was sent (epoch ms).
    pub ts: u64,
}

/// Something the web layer should broadcast or persist.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A move was played (broadcast the new state; persist the record).
    Moved(MoveRecord),
    /// The game ended (persist; apply [`Room::rating_updates`] if rated).
    Ended {
        /// The result.
        result: GameResult,
        /// How it ended.
        termination: Termination,
    },
    /// A chat message to broadcast.
    Chat(ChatLine),
    /// The given side offered a draw.
    DrawOffered(Color),
    /// A pending draw offer is gone (declined, or cleared by a move).
    DrawDeclined,
    /// The move needs a collapse choice. Send only to the mover; they resend
    /// `notation + " " + option`.
    NeedsCollapseChoice {
        /// The move as entered.
        notation: String,
        /// The `{...}` options to choose from.
        options: Vec<String>,
    },
}

/// Why a room operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomError {
    /// The game has already ended.
    GameOver,
    /// The user is not one of the two players.
    NotAPlayer,
    /// It is the opponent's turn.
    NotYourTurn,
    /// The engine rejected the move.
    IllegalMove(String),
    /// Too many plies have been played to abort.
    TooLateToAbort,
    /// There is no draw offer from the opponent to answer.
    NoDrawOffer,
    /// A draw offer is already pending.
    DrawAlreadyOffered,
    /// The chat message is empty after trimming.
    ChatEmpty,
    /// The chat message exceeds [`CHAT_MAX_CHARS`].
    ChatTooLong,
}

impl fmt::Display for RoomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoomError::GameOver => f.write_str("the game is over"),
            RoomError::NotAPlayer => f.write_str("you are not a player in this game"),
            RoomError::NotYourTurn => f.write_str("it is not your turn"),
            RoomError::IllegalMove(m) => write!(f, "illegal move: {m}"),
            RoomError::TooLateToAbort => f.write_str("the game can no longer be aborted"),
            RoomError::NoDrawOffer => f.write_str("there is no draw offer to answer"),
            RoomError::DrawAlreadyOffered => f.write_str("a draw offer is already pending"),
            RoomError::ChatEmpty => f.write_str("message is empty"),
            RoomError::ChatTooLong => write!(f, "message is longer than {CHAT_MAX_CHARS} characters"),
        }
    }
}

impl std::error::Error for RoomError {}

/// A live game.
#[derive(Debug, Clone)]
pub struct Room {
    id: String,
    white: PlayerInfo,
    black: PlayerInfo,
    tc: TimeControl,
    rated: bool,
    game: Game,
    clock: Clock,
    moves: Vec<MoveRecord>,
    draw_offer: Option<Color>,
    chat: Vec<ChatLine>,
    status: RoomStatus,
    created_ms: u64,
}

/// A player as shown to clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerView {
    /// Display name.
    pub username: String,
    /// Rounded rating (1500 for a player without one).
    pub rating: i32,
    /// Whether the rating is provisional.
    pub provisional: bool,
}

/// `status` of the protocol state message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusView {
    /// `{"kind":"ongoing"}`.
    Ongoing,
    /// `{"kind":"ended", ...}` with protocol strings.
    Ended {
        /// `"1-0"`, `"0-1"` or `"1/2-1/2"`.
        result: &'static str,
        /// Termination name, see [`Termination::as_str`].
        termination: &'static str,
    },
}

impl StatusView {
    /// `"ongoing"` or `"ended"`.
    pub fn kind(&self) -> &'static str {
        match self {
            StatusView::Ongoing => "ongoing",
            StatusView::Ended { .. } => "ended",
        }
    }
}

/// Plain-data mirror of the protocol `state` message.
#[derive(Debug, Clone, PartialEq)]
pub struct StateView {
    /// Game id.
    pub id: String,
    /// The viewer's color, or `None` for a spectator.
    pub you: Option<Color>,
    /// White player.
    pub white: PlayerView,
    /// Black player.
    pub black: PlayerView,
    /// Clock numbers.
    pub clock: ClockView,
    /// Side to move.
    pub to_move: Color,
    /// Ongoing or ended.
    pub status: StatusView,
    /// Check state of the side to move (`"none"`, `"quantum"`, `"full"` via
    /// [`check_str`]).
    pub check: Check,
    /// Number of possible classical boards.
    pub state_count: usize,
    /// All pieces in play.
    pub pieces: Vec<PieceView>,
    /// From-square to destinations for the side to move; empty once ended.
    pub targets: Vec<(String, Vec<String>)>,
    /// Squares to highlight.
    pub last_move: Vec<String>,
    /// All moves so far.
    pub moves: Vec<MoveRecord>,
    /// Pending draw offer.
    pub draw_offer: Option<Color>,
    /// Whether the game is rated.
    pub rated: bool,
}

impl StateView {
    /// Protocol string for [`you`](Self::you).
    pub fn you_str(&self) -> Option<&'static str> {
        self.you.map(color_str)
    }

    /// Protocol string for [`check`](Self::check).
    pub fn check_str(&self) -> &'static str {
        check_str(self.check)
    }
}

impl Room {
    /// Creates a fresh game. The clock is idle until both sides have moved.
    pub fn new(
        id: impl Into<String>,
        white: PlayerInfo,
        black: PlayerInfo,
        tc: TimeControl,
        rated: bool,
        now_ms: u64,
    ) -> Room {
        Room {
            id: id.into(),
            white,
            black,
            tc,
            rated,
            game: Game::new(),
            clock: Clock::new(tc),
            moves: Vec::new(),
            draw_offer: None,
            chat: Vec::new(),
            status: RoomStatus::Ongoing,
            created_ms: now_ms,
        }
    }

    /// Game id.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// The white player.
    pub fn white(&self) -> &PlayerInfo {
        &self.white
    }
    /// The black player.
    pub fn black(&self) -> &PlayerInfo {
        &self.black
    }
    /// The player of the given color.
    pub fn player(&self, c: Color) -> &PlayerInfo {
        match c {
            Color::White => &self.white,
            Color::Black => &self.black,
        }
    }
    /// The time control.
    pub fn time_control(&self) -> TimeControl {
        self.tc
    }
    /// Whether the game is rated.
    pub fn rated(&self) -> bool {
        self.rated
    }
    /// The engine game.
    pub fn game(&self) -> &Game {
        &self.game
    }
    /// The clock.
    pub fn clock(&self) -> &Clock {
        &self.clock
    }
    /// All moves played.
    pub fn moves(&self) -> &[MoveRecord] {
        &self.moves
    }
    /// The chat log (at most [`CHAT_CAP`] lines, oldest first).
    pub fn chat_log(&self) -> &[ChatLine] {
        &self.chat
    }
    /// The pending draw offer, if any.
    pub fn draw_offer(&self) -> Option<Color> {
        self.draw_offer
    }
    /// Current status.
    pub fn status(&self) -> RoomStatus {
        self.status
    }
    /// Whether the game has ended.
    pub fn is_over(&self) -> bool {
        self.status != RoomStatus::Ongoing
    }
    /// Creation time (epoch ms).
    pub fn created_ms(&self) -> u64 {
        self.created_ms
    }
    /// Number of plies played.
    pub fn plies(&self) -> usize {
        self.moves.len()
    }
    /// The color `user_id` plays, if they are a player.
    pub fn color_of(&self, user_id: i64) -> Option<Color> {
        if user_id == self.white.user_id {
            Some(Color::White)
        } else if user_id == self.black.user_id {
            Some(Color::Black)
        } else {
            None
        }
    }

    fn player_color(&self, user_id: i64) -> Result<Color, RoomError> {
        self.color_of(user_id).ok_or(RoomError::NotAPlayer)
    }

    fn ensure_ongoing(&self) -> Result<(), RoomError> {
        if self.is_over() { Err(RoomError::GameOver) } else { Ok(()) }
    }

    fn end(&mut self, result: GameResult, termination: Termination, now_ms: u64) -> Event {
        self.clock.stop(now_ms);
        self.draw_offer = None;
        self.status = RoomStatus::Ended { result, termination };
        Event::Ended { result, termination }
    }

    /// Ends the game by timeout if the running side is out of time. Lichess
    /// rule: if the opponent of the flagged side has only a king, it is a
    /// draw.
    fn end_if_flagged(&mut self, now_ms: u64) -> Option<Event> {
        let loser = self.clock.flagged(now_ms)?;
        Some(self.timeout(loser, now_ms))
    }

    fn timeout(&mut self, loser: Color, now_ms: u64) -> Event {
        let result = if self.has_only_king(loser.other()) {
            GameResult::Draw
        } else {
            GameResult::win_for(loser.other())
        };
        self.end(result, Termination::Timeout, now_ms)
    }

    /// True if `side` has no alive half other than kings (in any state).
    fn has_only_king(&self, side: Color) -> bool {
        self.game
            .pieces()
            .iter()
            .filter(|p| p.color == side)
            .flat_map(|p| p.halves.iter())
            .all(|h| !h.alive || h.kind == qchess::Kind::King)
    }

    /// Plays `notation` for `user_id`.
    ///
    /// Returns `Moved` (plus `Ended` if it mates or stalemates, plus
    /// `DrawDeclined` first if it cleared a pending draw offer). If the
    /// mover's clock had already run out the move is not played and the game
    /// ends by timeout instead. If the move needs a collapse choice and the
    /// notation has no `{...}`, only `NeedsCollapseChoice` is returned and
    /// nothing changes.
    pub fn play(&mut self, user_id: i64, notation: &str, now_ms: u64) -> Result<Vec<Event>, RoomError> {
        self.ensure_ongoing()?;
        let color = self.player_color(user_id)?;
        if color != self.game.to_move() {
            return Err(RoomError::NotYourTurn);
        }
        if let Some(ev) = self.end_if_flagged(now_ms) {
            return Ok(vec![ev]);
        }
        let notation = notation.trim();
        if !notation.contains('{') {
            let options = self
                .game
                .collapse_options(notation)
                .map_err(|e| RoomError::IllegalMove(e.0))?;
            if options.len() > 1 {
                return Ok(vec![Event::NeedsCollapseChoice { notation: notation.to_string(), options }]);
            }
        }
        let res = self.game.play(notation).map_err(|e| RoomError::IllegalMove(e.0))?;

        let mut events = Vec::new();
        let plies_before = self.moves.len();
        let flagged = if plies_before >= 2 { self.clock.press(now_ms).err() } else { None };
        if plies_before == 1 {
            self.clock.start(Color::White, now_ms);
        }
        let record = MoveRecord {
            ply: plies_before + 1,
            notation: res.notation,
            display: res.display,
            collapses: res.collapses,
            white_ms: self.clock.remaining(Color::White, now_ms),
            black_ms: self.clock.remaining(Color::Black, now_ms),
            at_ms: now_ms,
        };
        self.moves.push(record.clone());
        if self.draw_offer.take().is_some() {
            events.push(Event::DrawDeclined);
        }
        events.push(Event::Moved(record));
        if let Some(Flag(loser)) = flagged {
            // Unreachable after the check above, but keep the clock honest.
            events.push(self.timeout(loser, now_ms));
            return Ok(events);
        }
        match self.game.status() {
            Status::Ongoing => {}
            Status::Checkmate { winner } => {
                events.push(self.end(GameResult::win_for(winner), Termination::Checkmate, now_ms));
            }
            Status::Stalemate => events.push(self.end(GameResult::Draw, Termination::Stalemate, now_ms)),
        }
        Ok(events)
    }

    /// `user_id` resigns; the opponent wins.
    pub fn resign(&mut self, user_id: i64, now_ms: u64) -> Result<Vec<Event>, RoomError> {
        self.ensure_ongoing()?;
        let color = self.player_color(user_id)?;
        Ok(vec![self.end(GameResult::win_for(color.other()), Termination::Resign, now_ms)])
    }

    /// Either player aborts the game; only allowed before 2 plies are played.
    pub fn abort(&mut self, user_id: i64, now_ms: u64) -> Result<Vec<Event>, RoomError> {
        self.ensure_ongoing()?;
        self.player_color(user_id)?;
        if self.moves.len() >= ABORT_PLIES {
            return Err(RoomError::TooLateToAbort);
        }
        Ok(vec![self.end(GameResult::Draw, Termination::Abort, now_ms)])
    }

    /// `user_id` offers a draw.
    pub fn offer_draw(&mut self, user_id: i64) -> Result<Vec<Event>, RoomError> {
        self.ensure_ongoing()?;
        let color = self.player_color(user_id)?;
        if self.draw_offer.is_some() {
            return Err(RoomError::DrawAlreadyOffered);
        }
        self.draw_offer = Some(color);
        Ok(vec![Event::DrawOffered(color)])
    }

    /// `user_id` accepts the opponent's pending offer; the game is drawn.
    /// If a clock has run out the game ends by timeout instead.
    pub fn accept_draw(&mut self, user_id: i64, now_ms: u64) -> Result<Vec<Event>, RoomError> {
        self.ensure_ongoing()?;
        let color = self.player_color(user_id)?;
        if self.draw_offer != Some(color.other()) {
            return Err(RoomError::NoDrawOffer);
        }
        if let Some(ev) = self.end_if_flagged(now_ms) {
            return Ok(vec![ev]);
        }
        Ok(vec![self.end(GameResult::Draw, Termination::DrawAgreement, now_ms)])
    }

    /// `user_id` declines the opponent's pending offer.
    pub fn decline_draw(&mut self, user_id: i64) -> Result<Vec<Event>, RoomError> {
        self.ensure_ongoing()?;
        let color = self.player_color(user_id)?;
        if self.draw_offer != Some(color.other()) {
            return Err(RoomError::NoDrawOffer);
        }
        self.draw_offer = None;
        Ok(vec![Event::DrawDeclined])
    }

    /// Adds a chat line from any user (players and spectators alike; also
    /// after the game). The text is trimmed; empty or over
    /// [`CHAT_MAX_CHARS`] characters is rejected. Players are shown under
    /// their room username; others under `username`.
    pub fn chat(&mut self, user_id: i64, username: &str, text: &str, now_ms: u64) -> Result<Vec<Event>, RoomError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RoomError::ChatEmpty);
        }
        if text.chars().count() > CHAT_MAX_CHARS {
            return Err(RoomError::ChatTooLong);
        }
        let username = match self.color_of(user_id) {
            Some(c) => self.player(c).username.clone(),
            None => username.to_string(),
        };
        let line = ChatLine { user_id, username, text: text.to_string(), ts: now_ms };
        self.chat.push(line.clone());
        if self.chat.len() > CHAT_CAP {
            self.chat.remove(0);
        }
        Ok(vec![Event::Chat(line)])
    }

    /// Called by the web layer's timer: ends the game if a clock has run out.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Event> {
        if self.is_over() {
            return Vec::new();
        }
        self.end_if_flagged(now_ms).into_iter().collect()
    }

    /// When the web layer should next call [`tick`](Self::tick): the moment
    /// the running clock flags (never earlier than `now_ms`), or `None` if
    /// the game is over or the clock is not running.
    pub fn next_deadline_ms(&self, now_ms: u64) -> Option<u64> {
        if self.is_over() {
            return None;
        }
        self.clock.flag_at_ms().map(|t| t.max(now_ms))
    }

    /// New ratings `(white, black)` for an ended, rated, non-aborted game,
    /// from the players' current ratings (default when unrated).
    pub fn rating_updates(&self) -> Option<(Rating, Rating)> {
        let RoomStatus::Ended { result, termination } = self.status else { return None };
        if !self.rated || termination == Termination::Abort {
            return None;
        }
        let outcome = match result {
            GameResult::WhiteWins => Outcome::Win,
            GameResult::BlackWins => Outcome::Loss,
            GameResult::Draw => Outcome::Draw,
        };
        Some(glicko2::update_game(
            self.white.rating.unwrap_or_default(),
            self.black.rating.unwrap_or_default(),
            outcome,
            &Config::default(),
        ))
    }

    fn player_view(p: &PlayerInfo) -> PlayerView {
        let r = p.rating.unwrap_or_default();
        PlayerView {
            username: p.username.clone(),
            rating: r.rating.round() as i32,
            provisional: glicko2::provisional(&r),
        }
    }

    /// The protocol `state` message as plain data, for `viewer` (`None` for
    /// an anonymous spectator).
    pub fn snapshot(&self, viewer: Option<i64>, now_ms: u64) -> StateView {
        let to_move = self.game.to_move();
        let ongoing = !self.is_over();
        let mut targets = Vec::new();
        if ongoing {
            let mut froms: Vec<String> = self
                .game
                .pieces()
                .iter()
                .filter(|p| p.color == to_move)
                .flat_map(|p| p.halves.iter().filter(|h| h.alive).map(|h| h.square.clone()))
                .collect();
            froms.sort();
            for from in froms {
                let t = self.game.targets(&from);
                if !t.is_empty() {
                    targets.push((from, t));
                }
            }
        }
        StateView {
            id: self.id.clone(),
            you: viewer.and_then(|u| self.color_of(u)),
            white: Self::player_view(&self.white),
            black: Self::player_view(&self.black),
            clock: self.clock.view(self.tc, now_ms),
            to_move,
            status: match self.status {
                RoomStatus::Ongoing => StatusView::Ongoing,
                RoomStatus::Ended { result, termination } => {
                    StatusView::Ended { result: result.as_str(), termination: termination.as_str() }
                }
            },
            check: self.game.check(),
            state_count: self.game.state_count(),
            pieces: self.game.pieces(),
            targets,
            last_move: self.game.last_move_squares(),
            moves: self.moves.clone(),
            draw_offer: self.draw_offer,
            rated: self.rated,
        }
    }
}

#[cfg(test)]
mod tests;
