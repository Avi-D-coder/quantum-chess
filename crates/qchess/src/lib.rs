//! # qchess: a quantum chess engine
//!
//! Implements the "Quantum Chess" rule sheet: any piece can split into two
//! halves that sit in a superposition, and later moves collapse them. Moves
//! use algebraic notation with a `/` for splits; see [`notation`] for the
//! full grammar.
//!
//! ```
//! use qchess::Game;
//!
//! let mut g = Game::new();
//! for m in ["e3/4", "e5", "Nf3", "Nc6", "Bb5/c4", "Ne7/f6", "0-0/"] {
//!     g.play(m).unwrap();
//! }
//! // pawn, bishop, knight and king are each split: 2^4 possible boards
//! assert_eq!(g.state_count(), 16);
//! ```

mod board;
#[cfg(test)]
mod board_ref;
#[cfg(any(test, feature = "fuzzing"))]
pub mod fuzzing;
pub mod ai;
pub mod notation;
mod quantum;
mod render;
pub mod types;

use std::fmt;

pub use types::{Color, Kind};

use quantum::Position;
use types::{parse_sq, sq_name};

/// Error for an unreadable or illegal move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QError(pub String);

impl fmt::Display for QError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for QError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ongoing,
    Checkmate { winner: Color },
    Stalemate,
}

/// Whether the side to move is in check after a move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Not in check in any possible board state.
    None,
    /// In check in some states but not all ("Check?").
    Quantum,
    /// In check in every possible state.
    Full,
}

/// What happened when a move was played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayResult {
    /// The move as entered.
    pub notation: String,
    /// Human-readable list of collapses the move forced.
    pub collapses: Vec<String>,
    /// Check status of the opponent after the move.
    pub check: Check,
    /// The move written with numbered pieces, e.g. `P₅e3/e4` or `N₂f3`.
    pub display: String,
    /// How many equally small collapses the mover could choose between
    /// (1 when there was no choice). With no `{...}` given, the engine
    /// picked one; [`Game::collapse_options`] lists them all.
    pub options: usize,
}

/// One half of a piece, as drawn on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HalfView {
    /// Square name such as `"e4"`. For a captured half this is where it was
    /// captured.
    pub square: String,
    pub kind: Kind,
    /// `false` if this half was captured.
    pub alive: bool,
}

/// A piece for drawing: one half if unsplit, two if in superposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PieceView {
    /// Stable for the whole game.
    pub id: usize,
    pub color: Color,
    pub split: bool,
    pub halves: Vec<HalfView>,
    /// Kind the piece started as (a promoted pawn stays `Pawn`).
    pub origin: Kind,
    /// Number among same-kind pieces (pawns 1-8, rooks and knights 1-2),
    /// `None` for kings, queens and bishops.
    pub num: Option<u8>,
}

/// A quantum chess game.
#[derive(Debug, Clone)]
pub struct Game {
    pos: Position,
    /// The possible boards of `pos`, worked out on first use. Every query
    /// and every move from this position shares them.
    states: std::sync::OnceLock<Result<std::sync::Arc<quantum::States>, QError>>,
    /// [`position_key`](Self::position_key), worked out on first use.
    key: std::sync::OnceLock<u64>,
    history: Vec<String>,
    display: Vec<String>,
    status: Status,
    last_move: Vec<String>,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    /// Standard starting position.
    pub fn new() -> Self {
        Self::with_position(Position::initial())
    }

    /// Start from a classical position given as FEN.
    pub fn from_fen(fen: &str) -> Result<Self, QError> {
        Ok(Self::with_position(Position::from_fen(fen)?))
    }

    fn with_position(pos: Position) -> Self {
        let mut g = Game {
            pos,
            states: Default::default(),
            key: Default::default(),
            history: Vec::new(),
            display: Vec::new(),
            status: Status::Ongoing,
            last_move: Vec::new(),
        };
        g.status = g.compute_status();
        g
    }

    /// Play one move in quantum algebraic notation, e.g. `"e3/4"`,
    /// `"Bb5/c4"`, `"0-0/"` or `"Nxd5/ {xPd5}"`.
    pub fn play(&mut self, notation: &str) -> Result<PlayResult, QError> {
        let (report, display) = self.advance(notation, false)?;
        Ok(PlayResult {
            notation: notation.trim().to_string(),
            collapses: report.collapses,
            check: self.check(),
            display,
            options: report.options,
        })
    }

    /// [`Game::play`] for search: returns how many collapse options the move
    /// had and skips everything a search never reads (the report, the move
    /// text in [`history`](Self::history) and
    /// [`display_history`](Self::display_history), and
    /// [`last_move_squares`](Self::last_move_squares)). Only use it on games
    /// that are about to be thrown away.
    pub fn play_quiet(&mut self, notation: &str) -> Result<usize, QError> {
        self.advance(notation, true).map(|(r, _)| r.options)
    }

    /// Every way to play a move for search: one game per minimal collapse
    /// the mover could choose (with the collapse that picks it), or a single
    /// one when none or only one is needed. The games carry no move text.
    pub(crate) fn expand_quiet(&self, intent: &notation::Intent) -> Result<Vec<(Option<quantum::Cube>, Game)>, QError> {
        if self.status != Status::Ongoing {
            return Err(QError("the game is over".into()));
        }
        let st = self.st()?;
        Ok(self
            .pos
            .play_each(&st, intent)?
            .into_iter()
            .map(|(cube, pos, states)| {
                let mut g = Game {
                    pos,
                    states: Default::default(),
                    key: Default::default(),
                    history: Vec::new(),
                    display: Vec::new(),
                    status: Status::Ongoing,
                    last_move: Vec::new(),
                };
                if let Some(s) = states {
                    let _ = g.states.set(Ok(std::sync::Arc::new(s)));
                }
                g.status = g.compute_status();
                (cube, g)
            })
            .collect())
    }

    /// The same position with the other side to move and no en passant: the
    /// "null move" of search, never a legal move in the game.
    pub(crate) fn null_move(&self) -> Game {
        let mut pos = self.pos.clone();
        pos.turn = pos.turn.other();
        pos.ep = None;
        let mut g = Game { pos, states: Default::default(), key: Default::default(), history: Vec::new(), display: Vec::new(), status: Status::Ongoing, last_move: Vec::new() };
        if let Ok(st) = self.st() {
            let list = st
                .list
                .iter()
                .map(|(m, b)| {
                    let mut b = b.clone();
                    b.turn = b.turn.other();
                    b.ep = None;
                    (*m, b)
                })
                .collect();
            let _ = g.states.set(Ok(std::sync::Arc::new(quantum::States { splits: st.splits.clone(), list, invalid: st.invalid.clone() })));
        }
        g.status = g.compute_status();
        g
    }

    /// Notation for a move [`expand_quiet`](Self::expand_quiet) produced,
    /// with its `{...}` when it picks a collapse.
    pub(crate) fn notation_of(&self, intent: &notation::Intent, cube: Option<(u32, u32)>) -> String {
        let mv = notation::format(intent);
        match cube.and_then(|c| self.pos.describe_cube(&*self.st().ok()?, intent, c).ok()) {
            Some(spec) => format!("{mv} {spec}"),
            None => mv,
        }
    }

    fn advance(&mut self, notation: &str, quiet: bool) -> Result<(quantum::MoveReport, String), QError> {
        if self.status != Status::Ongoing {
            return Err(QError("the game is over".into()));
        }
        let parsed = notation::parse(notation)?;
        let st = self.st()?;
        let (next, mut report) = self.pos.play(&st, &parsed.intent, parsed.collapse.as_deref(), quiet)?;
        let display = if quiet { String::new() } else { display_move(&parsed.intent, &self.pos.pieces[report.piece]) };
        self.pos = next;
        self.states = Default::default();
        self.key = Default::default();
        if let Some(s) = report.states.take() {
            let _ = self.states.set(Ok(std::sync::Arc::new(s)));
        }
        if !quiet {
            self.last_move = report.moved.iter().map(|&s| sq_name(s)).collect();
            self.history.push(notation.trim().to_string());
            self.display.push(display.clone());
        }
        self.status = self.compute_status();
        Ok((report, display))
    }

    /// The start position and the position after each move, so
    /// `result.len() == moves.len() + 1`. Fails on the first bad move,
    /// naming its number.
    pub fn replay(moves: &[impl AsRef<str>]) -> Result<Vec<Game>, QError> {
        let mut g = Game::new();
        let mut out = vec![g.clone()];
        for (i, m) in moves.iter().enumerate() {
            let m = m.as_ref();
            g.play(m)
                .map_err(|e| QError(format!("move {} ({m}): {e}", i + 1)))?;
            out.push(g.clone());
        }
        Ok(out)
    }

    /// The game after playing all of `moves` from the start.
    pub fn from_history(moves: &[impl AsRef<str>]) -> Result<Game, QError> {
        let mut g = Game::new();
        for (i, m) in moves.iter().enumerate() {
            let m = m.as_ref();
            g.play(m)
                .map_err(|e| QError(format!("move {} ({m}): {e}", i + 1)))?;
        }
        Ok(g)
    }

    /// Every piece still in play (fully captured pieces are skipped), with
    /// one half if unsplit and two if split.
    pub fn pieces(&self) -> Vec<PieceView> {
        let view = |h: &quantum::Half| HalfView {
            square: sq_name(h.sq),
            kind: h.kind,
            alive: h.alive,
        };
        self.pos
            .pieces
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_dead())
            .map(|(id, p)| {
                let split = p.is_split();
                PieceView {
                    id,
                    color: p.color,
                    split,
                    halves: if split { p.h.iter().map(view).collect() } else { vec![view(&p.h[0])] },
                    origin: p.origin,
                    num: (p.num > 0).then_some(p.num),
                }
            })
            .collect()
    }

    /// Squares the side to move's piece half on `from` can legally reach in
    /// at least one possible state: sorted, no duplicates. Includes castling
    /// destinations. Empty if `from` is not a square or holds no such half.
    pub fn targets(&self, from: &str) -> Vec<String> {
        if self.status != Status::Ongoing {
            return Vec::new();
        }
        match parse_sq(from) {
            Some(s) => {
                let mut v = match self.st() {
                    Ok(st) => self.pos.targets(&st, s, u64::MAX),
                    Err(_) => Vec::new(),
                };
                v.sort_by_key(|&t| sq_name(t));
                v.into_iter().map(sq_name).collect()
            }
            None => Vec::new(),
        }
    }

    /// [`targets`](Self::targets) for the search: squares as numbers, no names.
    /// Only squares in the bitboard `only` are considered.
    pub(crate) fn targets_sq(&self, from: types::Sq, only: u64) -> Vec<types::Sq> {
        if self.status != Status::Ongoing {
            return Vec::new();
        }
        self.st().map(|st| self.pos.targets(&st, from, only)).unwrap_or_default()
    }

    /// Squares involved in the last move (those left and those reached,
    /// including the rook when castling); empty before the first move.
    pub fn last_move_squares(&self) -> Vec<String> {
        self.last_move.clone()
    }

    /// The minimal collapse choices for `notation` as `{...}` specs, without
    /// changing the game. Empty if the move needs no collapse. Any `{...}`
    /// already in `notation` is ignored. Each option appended to the move
    /// (`"<move> <option>"`) is accepted by [`Game::play`].
    pub fn collapse_options(&self, notation: &str) -> Result<Vec<String>, QError> {
        if self.status != Status::Ongoing {
            return Err(QError("the game is over".into()));
        }
        let parsed = notation::parse(notation)?;
        self.pos.collapse_options(&*self.st()?, &parsed.intent)
    }

    /// Hash of the quantum position (pieces, halves, side to move, en
    /// passant). Equal positions reached by different move orders hash alike.
    pub fn position_key(&self) -> u64 {
        *self.key.get_or_init(|| {
            use std::hash::{Hash, Hasher};
            let mut h = quantum::Fx::default();
            self.pos.hash(&mut h);
            h.finish()
        })
    }

    fn st(&self) -> Result<std::sync::Arc<quantum::States>, QError> {
        self.states.get_or_init(|| self.pos.states().map(std::sync::Arc::new)).clone()
    }

    pub fn to_move(&self) -> Color {
        self.pos.turn
    }

    pub fn status(&self) -> Status {
        self.status.clone()
    }

    /// Moves played so far, as entered.
    /// Every move so far written with numbered pieces (see
    /// [`PlayResult::display`]), parallel to [`history`](Self::history).
    pub fn display_history(&self) -> &[String] {
        &self.display
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Number of possible classical board states.
    pub fn state_count(&self) -> usize {
        self.st().map(|s| s.list.len()).unwrap_or(0)
    }

    /// Check status of the side to move.
    pub fn check(&self) -> Check {
        let Ok(st) = self.st() else {
            return Check::None;
        };
        let n = st.list.iter().filter(|(_, b)| b.in_check(self.pos.turn)).count();
        match n {
            0 => Check::None,
            n if n == st.list.len() => Check::Full,
            _ => Check::Quantum,
        }
    }

    /// ASCII board. Quantum halves are marked with `?`, followed by a list
    /// of every split piece.
    pub fn board_text(&self) -> String {
        render::board_text(&self.pos)
    }

    /// FEN piece placement of each possible board state (at most 256).
    pub fn states_fen(&self) -> Vec<String> {
        self.st()
            .map(|s| s.list.iter().take(256).map(|(_, b)| b.placement_fen()).collect())
            .unwrap_or_default()
    }

    fn compute_status(&self) -> Status {
        let Ok(st) = self.st() else {
            return Status::Ongoing;
        };
        if st.list.iter().any(|(_, b)| b.has_legal_move()) {
            return Status::Ongoing;
        }
        if st.list.iter().all(|(_, b)| b.in_check(self.pos.turn)) {
            Status::Checkmate {
                winner: self.pos.turn.other(),
            }
        } else {
            Status::Stalemate
        }
    }
}

#[cfg(test)]
mod tests;

/// A move written with the moving piece's label and its destinations.
fn display_move(intent: &notation::Intent, piece: &quantum::Piece) -> String {
    use notation::{Intent, Second};
    let promo = |p: Option<Kind>| p.map(|k| format!("={}", k.letter().to_ascii_uppercase())).unwrap_or_default();
    match *intent {
        Intent::Castle { side, split } => {
            let base = if side == crate::board::KING_SIDE { "0-0" } else { "0-0-0" };
            if split { format!("{base}/") } else { base.to_string() }
        }
        Intent::Move { kind, to, promo: p, second, .. } => {
            let mut s = format!("{}{}{}", piece.label(kind), sq_name(to), promo(p));
            match second {
                None => {}
                Some(Second::Stay) => s.push('/'),
                Some(Second::To { to, promo: p2, .. }) => s.push_str(&format!("/{}{}", sq_name(to), promo(p2))),
            }
            s
        }
    }
}
