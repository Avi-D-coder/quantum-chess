//! Fuzzing harness: a sequence of moves, then one arbitrary token.
//!
//! The input bytes drive a game: first they pick a prefix of *valid* moves.
//! In each position every move the UI could generate (whole moves, splits,
//! stays, castling, promotions) is tried on a copy, and the bytes choose
//! among the ones the engine accepts, so the prefix is always a legal game
//! that reaches deep quantum positions. Then one final token, either raw
//! bytes or a mutated legal move, is thrown at the engine. After every step the engine must uphold these invariants:
//!
//! * nothing panics (including `collapse_options`, `targets`, `pieces`),
//! * a rejected move leaves the game exactly as it was,
//! * an accepted move adds one history entry and one display entry, flips
//!   the side to move, and keeps `state_count == states_fen().len() >= 1`,
//! * replaying the history from scratch gives the same position,
//! * every collapse option offered for a move can actually be played,
//! * every square `targets()` lists can be reached by a whole move,
//! * no square holds two certain (unsplit, alive) pieces.
//!
//! It also checks the engine against the rule sheet, using `shakmaty` (an
//! independent, well-tested chess library) on every possible board:
//!
//! * Rule 1, legality: a move legal in regular chess from at least one board
//!   must be accepted (sampled from shakmaty's legal moves).
//! * Rule 3, reachability: every resulting board is reachable by a legal
//!   regular-chess move of the moved piece from some previous board (or is
//!   that board unchanged, for the half that stays in a `X/` split).
//! * Check is check on every board; "Check?" (quantum) on some but not all.
//! * Checkmate and stalemate follow the boards: no board has a legal move.

use crate::{Game, Status};

mod oracle {
    use shakmaty::{CastlingMode, CastlingSide, Chess, Position, PositionError, fen::Fen};

    /// A regular-chess position for one possible board. Castling rights are
    /// permissive (only those the placement allows), so reachability checks
    /// never reject a castle the engine legitimately allowed.
    pub fn position(placement: &str, white: bool, castling: &str, ep: &str) -> Option<Chess> {
        let fen = format!("{placement} {} {castling} {ep} 0 1", if white { "w" } else { "b" });
        Fen::from_ascii(fen.as_bytes())
            .ok()?
            .into_position::<Chess>(CastlingMode::Standard)
            .or_else(PositionError::ignore_invalid_castling_rights)
            .or_else(PositionError::ignore_invalid_ep_square)
            .or_else(PositionError::ignore_too_much_material)
            .or_else(PositionError::ignore_impossible_check)
            .ok()
    }

    pub fn placement(p: &Chess) -> String {
        p.board().board_fen(shakmaty::Bitboard::EMPTY).to_string()
    }

    /// Positions for a board with every en passant square the side to move
    /// might have (the engine's history decides which one is real).
    pub fn variants(placement: &str, white: bool) -> Vec<Chess> {
        let rank = if white { '6' } else { '3' };
        let mut out: Vec<Chess> = position(placement, white, "KQkq", "-").into_iter().collect();
        for f in 'a'..='h' {
            out.extend(position(placement, white, "KQkq", &format!("{f}{rank}")));
        }
        out
    }

    /// The engine's notation for a regular-chess move: `Ng1f3`, `e7e8=Q`, `0-0`.
    pub fn notation(m: &shakmaty::Move) -> Option<String> {
        if let Some(side) = m.castling_side() {
            return Some(if side == CastlingSide::KingSide { "0-0" } else { "0-0-0" }.into());
        }
        let from = m.from()?;
        let letter = match m.role() {
            shakmaty::Role::Pawn => String::new(),
            r => r.upper_char().to_string(),
        };
        let promo = m.promotion().map(|r| format!("={}", r.upper_char())).unwrap_or_default();
        Some(format!("{letter}{from}{}{promo}", m.to()))
    }

    pub fn legal_moves(p: &Chess) -> Vec<shakmaty::Move> {
        p.legal_moves().into_iter().collect()
    }

    pub fn play(p: &Chess, m: &shakmaty::Move) -> String {
        let mut q = p.clone();
        q.play_unchecked(m);
        placement(&q)
    }

    pub fn in_check(placement: &str, white: bool) -> Option<bool> {
        position(placement, white, "-", "-").map(|p| p.is_check())
    }

    /// Whether the side to move has any legal move, if castling rights do
    /// not change the answer (we cannot see the engine's castling rights).
    pub fn has_moves(placement: &str, white: bool) -> Option<bool> {
        let a = !position(placement, white, "-", "-")?.legal_moves().is_empty();
        let b = !position(placement, white, "KQkq", "-")?.legal_moves().is_empty();
        (a == b).then_some(a)
    }
}

/// Rule 3: every board after the move comes from a previous board by a legal
/// regular-chess move from one of the squares the move left (or is that
/// previous board unchanged when one half stays).
fn check_reachable(prev: &Game, g: &Game, notation: &str) {
    if prev.state_count() > 256 || g.state_count() > 256 {
        return;
    }
    let white = prev.to_move() == crate::Color::White;
    let before: Vec<String> = prev.states_fen();
    let moved: Vec<String> = g.last_move_squares();
    // A "stay" split ends in '/', before any {collapse} or +#?! annotations.
    let stays = notation
        .split('{')
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches(['+', '#', '?', '!', ' '])
        .ends_with('/');
    let mut reachable = std::collections::HashSet::new();
    for b in &before {
        if stays {
            reachable.insert(b.clone());
        }
        for p in oracle::variants(b, white) {
            for m in oracle::legal_moves(&p) {
                if m.from().is_some_and(|f| moved.contains(&f.to_string())) {
                    reachable.insert(oracle::play(&p, &m));
                }
            }
        }
    }
    for after in g.states_fen() {
        assert!(
            reachable.contains(&after),
            "rule 3: board {after} after {notation:?} is not reachable by a legal move from any previous board\nprevious: {before:?}\nhistory: {:?}",
            g.history()
        );
    }
}

/// Check, checkmate and stalemate as the rule sheet defines them.
fn check_status(g: &Game) {
    if g.state_count() > 256 {
        return;
    }
    let white = g.to_move() == crate::Color::White;
    let boards = g.states_fen();
    let checks: Vec<bool> = boards
        .iter()
        .map(|b| {
            oracle::in_check(b, white)
                .unwrap_or_else(|| panic!("board {b} is not a valid chess position\nhistory: {:?}", g.history()))
        })
        .collect();
    let expected = if checks.iter().all(|&c| c) {
        crate::Check::Full
    } else if checks.iter().any(|&c| c) {
        crate::Check::Quantum
    } else {
        crate::Check::None
    };
    if g.status() == Status::Ongoing {
        assert_eq!(g.check(), expected, "check status\nhistory: {:?}", g.history());
    }
    let moves: Option<Vec<bool>> = boards.iter().map(|b| oracle::has_moves(b, white)).collect();
    if let Some(moves) = moves {
        let any_move = moves.iter().any(|&m| m);
        let status = g.status();
        if any_move {
            assert_eq!(status, Status::Ongoing, "game ended though a board has a legal move\nhistory: {:?}", g.history());
        } else if expected == crate::Check::Full {
            assert_eq!(status, Status::Checkmate { winner: g.to_move().other() }, "history: {:?}", g.history());
        } else {
            assert_eq!(status, Status::Stalemate, "history: {:?}", g.history());
        }
    }
}

/// Rule 1: a move legal in regular chess from some board is accepted.
/// Checks up to `n` moves picked by the fuzzer.
fn check_rule_one(g: &Game, u: &mut Bytes, n: usize) {
    if g.status() != Status::Ongoing || g.state_count() > 256 {
        return;
    }
    let white = g.to_move() == crate::Color::White;
    let boards = g.states_fen();
    for _ in 0..n {
        let b = &boards[u.pick(boards.len())];
        let Some(p) = oracle::position(b, white, "-", "-") else { continue };
        let moves = oracle::legal_moves(&p);
        if moves.is_empty() {
            continue;
        }
        let Some(mv) = oracle::notation(&moves[u.pick(moves.len())]) else { continue };
        if let Err(e) = g.clone().play(&mv) {
            assert!(
                e.0.contains("ambiguous"),
                "rule 1: {mv} is legal on board {b} but was rejected: {e}\nhistory: {:?}",
                g.history()
            );
        }
    }
}

/// Reads choices from the fuzzer's bytes; returns zeros once exhausted.
pub struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bytes<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Bytes { data, pos: 0 }
    }
    pub fn byte(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }
    /// A choice in `0..n` (`n > 0`).
    pub fn pick(&mut self, n: usize) -> usize {
        let v = (self.byte() as usize) << 8 | self.byte() as usize;
        v % n
    }
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos.min(self.data.len())..]
    }
    pub fn done(&self) -> bool {
        self.pos >= self.data.len()
    }
}

/// Everything observable about a game, for "nothing changed" checks.
#[derive(Debug, PartialEq)]
struct Snapshot {
    history: Vec<String>,
    display: Vec<String>,
    states: Vec<String>,
    pieces: String,
    to_move: crate::Color,
    status: Status,
}

fn snapshot(g: &Game) -> Snapshot {
    let mut states = g.states_fen();
    states.sort();
    Snapshot {
        history: g.history().to_vec(),
        display: g.display_history().to_vec(),
        states,
        pieces: format!("{:?}", g.pieces()),
        to_move: g.to_move(),
        status: g.status(),
    }
}

const PROMOS: [&str; 4] = ["Q", "R", "B", "N"];

fn letter(kind: crate::Kind) -> &'static str {
    match kind {
        crate::Kind::Pawn => "",
        crate::Kind::Knight => "N",
        crate::Kind::Bishop => "B",
        crate::Kind::Rook => "R",
        crate::Kind::Queen => "Q",
        crate::Kind::King => "K",
    }
}

/// A king moving two files is castling.
fn castle_for(kind: crate::Kind, from: &str, to: &str) -> Option<&'static str> {
    let df = to.as_bytes()[0] as i32 - from.as_bytes()[0] as i32;
    (kind == crate::Kind::King && df.abs() == 2).then_some(if df > 0 { "0-0" } else { "0-0-0" })
}

fn last_rank(sq: &str) -> bool {
    sq.ends_with('8') || sq.ends_with('1')
}

/// Whole moves "K e1 g1"-style from every target: (notation, must_be_legal).
fn whole_moves(g: &Game) -> Vec<(String, bool)> {
    let me = g.to_move();
    let mut out = Vec::new();
    for p in g.pieces().into_iter().filter(|p| p.color == me) {
        for h in p.halves.iter().filter(|h| h.alive) {
            for t in g.targets(&h.square) {
                if let Some(castle) = castle_for(h.kind, &h.square, &t) {
                    // Castling is written 0-0 / 0-0-0, as the board UI sends it.
                    out.push((castle.to_string(), true));
                    continue;
                }
                let promo = if h.kind == crate::Kind::Pawn && last_rank(&t) { "=Q" } else { "" };
                out.push((format!("{}{}{}{promo}", letter(h.kind), h.square, t), true));
            }
        }
    }
    out
}

/// Every move the browser UI could generate here that the engine accepts.
/// Candidates are tried on a copy; whole moves from `targets()` must succeed.
pub fn legal_moves(g: &Game, u: &mut Bytes) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (mv, must) in candidates(g, u) {
        if !seen.insert(mv.clone()) {
            continue;
        }
        match g.clone().play(&mv) {
            Ok(_) => out.push(mv),
            Err(e) => assert!(
                !must || e.0.contains("ambiguous"),
                "targets() offered {mv:?} but it was rejected: {e}\nhistory: {:?}",
                g.history()
            ),
        }
    }
    out
}

/// Moves the browser UI can generate, including splits and castling.
/// Only plain whole moves are guaranteed legal.
pub fn candidates(g: &Game, u: &mut Bytes) -> Vec<(String, bool)> {
    let me = g.to_move();
    let mut out = whole_moves(g);
    for p in g.pieces().into_iter().filter(|p| p.color == me) {
        let alive: Vec<_> = p.halves.iter().filter(|h| h.alive).collect();
        for h in &alive {
            let ts = g.targets(&h.square);
            let l = letter(h.kind);
            for t in &ts {
                let pr = if h.kind == crate::Kind::Pawn && last_rank(t) {
                    format!("={}", PROMOS[u.pick(4)])
                } else {
                    String::new()
                };
                out.push((format!("{l}{}{t}{pr}/", h.square), false));
                if let Some(base) = castle_for(h.kind, &h.square, t) {
                    out.push((format!("{base}/"), false));
                }
                for t2 in &ts {
                    if t2 != t {
                        let pr2 = if h.kind == crate::Kind::Pawn && last_rank(t2) {
                            format!("={}", PROMOS[u.pick(4)])
                        } else {
                            String::new()
                        };
                        out.push((format!("{l}{}{t}{pr}/{t2}{pr2}", h.square), false));
                    }
                }
            }
        }
        if let [a, b] = alive[..] {
            for ta in g.targets(&a.square) {
                for tb in g.targets(&b.square) {
                    out.push((format!("{}{}{ta}/{}{tb}", letter(a.kind), a.square, b.square), false));
                }
            }
        }
    }
    out
}

const ALPHABET: &[u8] = b"KQRBNPkqrbnpabcdefgh123456780xX/=+?#!{} -Oo\t";

fn mutate(s: &str, u: &mut Bytes) -> String {
    let mut v: Vec<u8> = s.bytes().collect();
    for _ in 0..1 + u.byte() % 4 {
        let at = if v.is_empty() { 0 } else { u.pick(v.len() + 1) };
        let c = ALPHABET[u.pick(ALPHABET.len())];
        match u.byte() % 3 {
            0 => v.insert(at, c),
            1 if at < v.len() => {
                v.remove(at);
            }
            _ if at < v.len() => v[at] = c,
            _ => v.push(c),
        }
    }
    String::from_utf8_lossy(&v).into_owned()
}

/// Plays `notation` and checks every invariant. Returns whether it was played.
pub fn step(g: &mut Game, notation: &str, must_be_legal: bool) -> bool {
    let before = snapshot(g);
    let options = g.collapse_options(notation);
    let _ = g.targets(notation.get(..2).unwrap_or(""));
    let mut trial = g.clone();
    match trial.play(notation) {
        Err(e) => {
            assert_eq!(snapshot(&trial), before, "rejected move {notation:?} changed the game");
            if must_be_legal {
                assert!(
                    e.0.contains("ambiguous"),
                    "targets() offered {notation:?} but it was rejected: {e}\n{}\nhistory: {:?}",
                    g.board_text(),
                    g.history()
                );
            }
            false
        }
        Ok(r) => {
            check_after(g, &trial, &before);
            check_reachable(g, &trial, notation);
            check_status(&trial);
            assert_eq!(r.display, *trial.display_history().last().unwrap());
            if let Ok(opts) = options
                && opts.len() > 1
            {
                for o in &opts {
                    let mut t = g.clone();
                    let with = format!("{} {o}", notation.trim());
                    if let Err(e) = t.play(&with) {
                        panic!("offered collapse {o} for {notation:?} is not playable: {e}\nhistory: {:?}", g.history());
                    }
                    check_after(g, &t, &before);
                }
            }
            *g = trial;
            true
        }
    }
}

fn check_after(prev: &Game, g: &Game, before: &Snapshot) {
    assert_eq!(g.history().len(), before.history.len() + 1);
    assert_eq!(g.display_history().len(), g.history().len());
    assert_ne!(g.to_move(), prev.to_move(), "side to move did not change");
    let states = g.states_fen();
    assert!(!states.is_empty(), "no possible states");
    // states_fen() lists at most 256 boards.
    assert_eq!(g.state_count().min(256), states.len(), "history: {:?}", g.history());
    let replayed = Game::from_history(g.history())
        .unwrap_or_else(|e| panic!("history does not replay: {e}\nhistory: {:?}", g.history()));
    let after = snapshot(g);
    assert_eq!(snapshot(&replayed), after, "replay differs\nhistory: {:?}", g.history());
    let mut certain = std::collections::HashSet::new();
    for p in g.pieces() {
        assert!((1..=2).contains(&p.halves.len()));
        assert_eq!(p.split, p.halves.len() == 2);
        assert!(p.halves.iter().any(|h| h.alive), "fully captured piece listed");
        if !p.split {
            assert!(certain.insert(p.halves[0].square.clone()), "two certain pieces on {}", p.halves[0].square);
        }
    }
    let _ = (g.check(), g.board_text(), g.last_move_squares());
}

/// One fuzz case: up to `max_prefix` moves, then one final token.
pub fn run(data: &[u8], max_prefix: usize) {
    let mut u = Bytes::new(data);
    let mut g = Game::new();
    let n = u.byte() as usize % (max_prefix + 1);
    for _ in 0..n {
        if g.status() != Status::Ongoing || u.done() {
            break;
        }
        check_rule_one(&g, &mut u, 3);
        let moves = legal_moves(&g, &mut u);
        if moves.is_empty() {
            break;
        }
        let mv = moves[u.pick(moves.len())].clone();
        assert!(step(&mut g, &mv, true), "a move accepted on a copy was rejected: {mv}");
    }
    // The final token: raw bytes, or a mutated move from this position.
    let token = if u.byte().is_multiple_of(2) {
        String::from_utf8_lossy(u.rest()).into_owned()
    } else {
        let moves = legal_moves(&g, &mut u);
        let base = if moves.is_empty() { String::new() } else { moves[u.pick(moves.len())].clone() };
        mutate(&base, &mut u)
    };
    step(&mut g, &token, false);
}

/// One AI-vs-AI game from `seed`, checking every move like [`step`].
/// The AI picks at random among moves within `margin` centipawns of its
/// best, so seeds give different, but still purposeful, games: real
/// exchanges, promotions, castling and mates, which random play rarely
/// reaches. Returns the final game.
pub fn self_play(seed: u64, max_plies: usize, depth: i32, margin: i32) -> Game {
    use crate::ai::{Config, Searcher};
    let mut g = Game::new();
    let mut ai = Searcher::new(Config { depth, time: None, margin, ..Config::default() }).seed(seed);
    let mut x = seed | 1;
    for _ in 0..max_plies {
        if g.status() != Status::Ongoing {
            break;
        }
        // Rule 1 check on a few classical moves from some board.
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let bytes = x.to_le_bytes();
        let mut b = Bytes::new(&bytes);
        check_rule_one(&g, &mut b, 2);
        let Some(c) = ai.choose(&g) else {
            panic!("ongoing game but the AI found no move\n{}\nhistory: {:?}", g.board_text(), g.history());
        };
        assert!(step(&mut g, &c.notation, true), "AI move {} was rejected\nhistory: {:?}", c.notation, g.history());
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic fuzz run (xorshift input) so `cargo test`
    /// exercises the harness. The real fuzzer lives in `fuzz/`.
    #[test]
    fn smoke_fuzz() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let cases: usize = std::env::var("QCHESS_FUZZ_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(if cfg!(debug_assertions) { 12 } else { 150 });
        for _ in 0..cases {
            let data: Vec<u8> = (0..400)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    (x >> 24) as u8
                })
                .collect();
            run(&data, if cfg!(debug_assertions) { 12 } else { 30 });
        }
    }

    /// AI-vs-AI games with every move checked. `QCHESS_SELFPLAY_GAMES`
    /// sets how many (each with its own seed).
    #[test]
    fn smoke_self_play() {
        let games: u64 = std::env::var("QCHESS_SELFPLAY_GAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let plies = if cfg!(debug_assertions) { 16 } else { 120 };
        for seed in 1..=games {
            let g = self_play(seed, plies, if cfg!(debug_assertions) { 1 } else { 2 }, 40);
            assert!(!g.history().is_empty());
        }
    }
}
