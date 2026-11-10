//! A small quantum chess AI, modelled on the classical engine loop that
//! Stockfish uses, scaled down:
//!
//! * **Search:** iterative deepening negamax with alpha-beta pruning, a
//!   transposition table keyed by [`Game::position_key`], and move ordering
//!   (transposition-table move, then captures by most valuable victim, then
//!   killer moves).
//! * **Quiescence:** at the horizon, whole-piece captures are searched until
//!   the position is quiet, so the eval isn't taken in the middle of an
//!   exchange.
//! * **Evaluation:** material plus piece-square tables. A split piece counts
//!   half on each square its halves could be on, and a half that has been
//!   captured counts nothing. That is the average over the board states,
//!   computed per piece instead of over up to 65,536 boards.
//!
//! What's different from classical chess:
//!
//! * There's no chance node. When a move forces a collapse, the side that
//!   moved chooses which minimal collapse happens (rule 2). So every
//!   collapse option becomes a move of its own, and the game stays
//!   deterministic.
//! * Moves come from the same notation the players type: whole moves, splits
//!   (`Nc3/a3`), moves of both halves (`Nc3/Nd4`), split
//!   promotions (`e8=Q/N`) and quantum castling (`0-0/`). Each candidate is
//!   played on a copy of the game, and only accepted moves are kept, so the
//!   AI can never play an illegal move.
//! * There's no opening book and no endgame tablebase.

use std::collections::{HashMap, HashSet};
use std::time::Duration;
// std::time::Instant panics on wasm32-unknown-unknown; web-time uses performance.now() there.
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use crate::board::{KING_SIDE, QUEEN_SIDE};
use crate::notation::{Hint, Intent, Second};
use crate::quantum::FastHash;
use crate::types::{Sq, file_of, rank_of};
use crate::{Color, Game, Kind, Status};

/// Score for delivering mate now. A mate found `n` plies deeper scores
/// `MATE - n`, so faster mates score higher.
pub const MATE: i32 = 100_000;
const INF: i32 = MATE + 1;
/// Futility margin: what a quiet move might gain at most, in centipawns.
const FUTILITY: i32 = 250;
/// Delta pruning margin for captures in the quiescence search.
const DELTA: i32 = 200;
/// Depth reduction of the null move search.
const NULL_R: i32 = 2;
/// Quiet moves searched before late move pruning, by depth (1 and 2).
const LMP: [usize; 3] = [usize::MAX, 10, 18];

/// Search limits and style.
#[derive(Clone, Debug)]
pub struct Config {
    /// Deepest full-width iteration, in plies.
    pub depth: i32,
    /// Stop deepening once this much time has passed (the last finished
    /// iteration's move is played).
    pub time: Option<Duration>,
    /// Consider splits and two-half moves, not only whole moves.
    /// Without them the search is much faster but plays "classical" chess
    /// on a quantum board.
    pub quantum_moves: bool,
    /// Plies of captures to follow past the horizon.
    pub quiescence: i32,
    /// Pick at random among root moves scoring within this many centipawns
    /// of the best (0 = always the best). Used for varied self-play games.
    pub margin: i32,
    /// Skip moves that can't plausibly matter: quiet moves one ply from the
    /// horizon when the position is far below alpha (futility pruning), and
    /// captures in the quiescence search that can't lift the score to alpha
    /// (delta pruning). Much faster, at a small risk of missing something.
    pub pruning: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { depth: 3, time: Some(Duration::from_secs(5)), quantum_moves: true, quiescence: 4, margin: 0, pruning: true }
    }
}

/// One playable move and the game after it.
#[derive(Clone, Debug)]
pub struct Child {
    /// Notation accepted by [`Game::play`], including any `{...}` collapse.
    pub notation: String,
    pub game: Game,
    /// Value of the most valuable enemy piece on a destination square.
    pub victim: i32,
}

/// The AI's choice.
#[derive(Clone, Debug)]
pub struct Choice {
    pub notation: String,
    /// Score in centipawns for the side to move (positive = good for it).
    pub score: i32,
    /// Depth of the last completed iteration.
    pub depth: i32,
    pub nodes: u64,
}

// ------------------------------------------------------------------ eval

fn value(k: Kind) -> i32 {
    match k {
        Kind::Pawn => 100,
        Kind::Knight => 320,
        Kind::Bishop => 330,
        Kind::Rook => 500,
        Kind::Queen => 900,
        Kind::King => 0,
    }
}

/// (file, rank) of a square.
fn coords(s: Sq) -> (i32, i32) {
    (file_of(s) as i32, rank_of(s) as i32)
}

/// One half of a piece, without the strings [`Game::pieces`] builds for
/// drawing: the search looks at these many times per second.
#[derive(Clone, Copy)]
struct Hv {
    sq: Sq,
    kind: Kind,
    alive: bool,
}

#[derive(Clone, Copy)]
struct Pv {
    color: Color,
    origin: Kind,
    split: bool,
    h: [Hv; 2],
}

impl Pv {
    fn halves(&self) -> &[Hv] {
        if self.split { &self.h } else { &self.h[..1] }
    }
}

/// Every piece still in play.
fn views(g: &Game) -> Views {
    let mut out = Views { items: [Pv { color: Color::White, origin: Kind::Pawn, split: false, h: [Hv { sq: 0, kind: Kind::Pawn, alive: false }; 2] }; 64], len: 0 };
    for p in g.pos.pieces.iter().filter(|p| !p.is_dead()).take(64) {
        let v = |h: &crate::quantum::Half| Hv { sq: h.sq, kind: h.kind, alive: h.alive };
        out.items[out.len] = Pv { color: p.color, origin: p.origin, split: p.is_split(), h: [v(&p.h[0]), v(&p.h[1])] };
        out.len += 1;
    }
    out
}

/// The pieces of a position on the stack (there are never more than 64).
struct Views {
    items: [Pv; 64],
    len: usize,
}

impl std::ops::Deref for Views {
    type Target = [Pv];
    fn deref(&self) -> &[Pv] {
        &self.items[..self.len]
    }
}

/// Piece-square bonus, from white's side of the board.
fn pst(k: Kind, file: i32, rank: i32, endgame: bool) -> i32 {
    // Distance from the centre: 0 on d4/e4/d5/e5, up to 3 in the corners.
    let df = if file < 4 { 3 - file } else { file - 4 };
    let dr = if rank < 4 { 3 - rank } else { rank - 4 };
    let centre = 6 - 2 * (df + dr);
    match k {
        Kind::Pawn => {
            let advance = [0, 0, 5, 10, 20, 35, 60, 0][rank as usize];
            let middle = if (2..=5).contains(&file) { rank * 2 } else { 0 };
            advance + middle
        }
        Kind::Knight => centre * 4,
        Kind::Bishop => centre * 2,
        Kind::Rook => if rank == 6 { 20 } else { 0 },
        Kind::Queen => centre,
        // In the endgame the king is a fighting piece: centralize it.
        Kind::King if endgame => centre * 8,
        // Tucked away while there is material about: prefer the back rank,
        // away from the centre files.
        Kind::King => (if rank == 0 { 10 } else { -10 * rank.min(3) }) + if (2..=5).contains(&file) { -5 } else { 5 },
    }
}

/// `value + pst` for every [endgame][kind][square] from white's side,
/// worked out once.
static WORTH: std::sync::LazyLock<[[[i32; 64]; 6]; 2]> = std::sync::LazyLock::new(|| {
    let mut t = [[[0; 64]; 6]; 2];
    for (e, table) in t.iter_mut().enumerate() {
        for k in [Kind::Pawn, Kind::Knight, Kind::Bishop, Kind::Rook, Kind::Queen, Kind::King] {
            for s in 0..64u8 {
                let (f, r) = coords(s);
                table[k as usize][s as usize] = value(k) + pst(k, f, r, e == 1);
            }
        }
    }
    t
});

/// Worth of one piece to its owner: the average over its halves.
fn piece_worth(p: &Pv, endgame: bool) -> i32 {
    let table = &WORTH[endgame as usize];
    let n = p.halves().len().max(1) as i32;
    let mut sum = 0;
    for h in p.halves().iter().filter(|h| h.alive) {
        // Black's squares mirrored onto white's side.
        let s = if p.color == Color::White { h.sq } else { h.sq ^ 56 };
        sum += table[h.kind as usize][s as usize];
    }
    sum / n
}

/// Static evaluation for the side to move, in centipawns.
pub fn evaluate(g: &Game) -> i32 {
    let me = g.to_move();
    let pieces = views(g);
    // Few pieces left (queens and rooks count as two minor pieces' worth).
    let endgame = pieces.iter().filter(|p| p.origin != Kind::King && p.origin != Kind::Pawn).map(|p| p.h[0].kind).map(value).sum::<i32>() <= 1400;
    let mut score = 0;
    for p in pieces.iter() {
        let w = piece_worth(p, endgame);
        score += if p.color == me { w } else { -w };
    }
    score + mop_up(&pieces, me, endgame) - mop_up(&pieces, me.other(), endgame)
}

/// Where a king's halves are: the average (file, rank) in 1/2 squares, so
/// a split king counts as halfway between its two squares.
fn king_at(pieces: &[Pv], color: Color) -> Option<(i32, i32)> {
    let p = pieces.iter().find(|p| p.color == color && p.halves().iter().any(|h| h.kind == Kind::King))?;
    let hs: Vec<_> = p.halves().iter().filter(|h| h.alive).collect();
    if hs.is_empty() {
        return None;
    }
    let (f, r) = hs.iter().fold((0, 0), |(f, r), h| {
        let (hf, hr) = coords(h.sq);
        (f + hf * 2, r + hr * 2)
    });
    Some((f / hs.len() as i32, r / hs.len() as i32))
}

/// Endgame knowledge, as in Stockfish's special cases for a lone king:
/// when `color` has pieces and the other side has only a king, reward
/// driving that king to the edge (to a corner of the bishop's colour when
/// the pieces are a bishop and a knight) and bringing our king close.
/// Without it a shallow search can't find its way to mate.
fn mop_up(pieces: &[Pv], color: Color, endgame: bool) -> i32 {
    let others = |c: Color| pieces.iter().filter(move |p| p.color == c && p.origin != Kind::King);
    if others(color.other()).next().is_some() {
        return 0;
    }
    let (Some(mine), Some(theirs)) = (king_at(pieces, color), king_at(pieces, color.other())) else { return 0 };
    let material: i32 = others(color).map(|p| piece_worth(p, endgame)).sum();
    if material < 500 {
        return 0;
    }
    // Everything here is in half squares.
    let centre_dist = |(f, r): (i32, i32)| {
        let df = (2 * f - 7).abs();
        let dr = (2 * r - 7).abs();
        (df + dr) / 2
    };
    // Corner to aim for: the bishop's colour in K+B+N, else any corner.
    let bishops: Vec<_> = others(color)
        .filter(|p| p.halves().iter().any(|h| h.kind == Kind::Bishop && h.alive))
        .collect();
    let knights = others(color).filter(|p| p.halves().iter().any(|h| h.kind == Kind::Knight)).count();
    let mut edge = centre_dist((theirs.0 / 2, theirs.1 / 2)) * 12;
    if let (1, 1) = (bishops.len(), knights)
        && others(color).count() == 2
    {
        let (bf, br) = coords(bishops[0].halves().iter().find(|h| h.alive).unwrap().sq);
        let light = (bf + br) % 2 == 1;
        let corners = [(0, 0), (7, 7), (0, 7), (7, 0)].into_iter().filter(|(f, r)| ((f + r) % 2 == 1) == light);
        let d = corners
            .map(|(cf, cr)| ((theirs.0 - cf * 2).abs() + (theirs.1 - cr * 2).abs()) / 2)
            .min()
            .unwrap_or(0);
        // Steep: this has to outweigh centralization of our own pieces.
        edge = (14 - d) * 200;
    }
    let close = (14 - ((mine.0 - theirs.0).abs() + (mine.1 - theirs.1).abs()) / 2) * 10;
    edge + close
}

// ------------------------------------------------------------- moves

fn last_rank(s: Sq) -> bool {
    rank_of(s) == 0 || rank_of(s) == 7
}

/// Order squares the way their names sort ("a1" < "a2" < "b1"), which is the
/// order the candidate list has always been generated in.
fn by_name(s: Sq) -> (u8, u8) {
    (file_of(s), rank_of(s))
}

/// A move as the search keeps it: what to play, and which collapse when the
/// move leaves the mover a choice. Turned into notation only for the move
/// that is finally played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mv {
    intent: Intent,
    cube: Option<(u32, u32)>,
}

/// Splits, two-half moves and quantum castling.
fn is_quantum(i: &Intent) -> bool {
    matches!(i, Intent::Castle { split: true, .. } | Intent::Move { second: Some(_), .. })
}

/// A piece of kind `kind` on `from` going to `to`, named by its square.
fn from_to(kind: Kind, from: Sq, to: Sq, promo: Option<Kind>, second: Option<Second>) -> Intent {
    Intent::Move { kind, hint: Hint { file: Some(file_of(from)), rank: Some(rank_of(from)) }, to, promo, second }
}

fn to_sq(to: Sq, promo: Option<Kind>) -> Option<Second> {
    Some(Second::To { hint: Hint::default(), to, promo })
}

/// Moves to try from `g`. Not all of them are legal.
fn candidates(g: &Game, quantum: bool, captures_only: bool) -> Vec<Intent> {
    let me = g.to_move();
    let pieces = views(g);
    let mut enemy = [false; 64];
    let mut enemy_bb = 0u64;
    for p in pieces.iter().filter(|p| p.color != me) {
        for h in p.halves().iter().filter(|h| h.alive) {
            enemy[h.sq as usize] = true;
            enemy_bb |= 1 << h.sq;
        }
    }
    // Captures (and promotions, which land on the first or last rank) only
    // need those squares tested.
    let only = if captures_only { enemy_bb | 0xff00_0000_0000_00ff } else { u64::MAX };
    let targets = |s: Sq| {
        let mut v = g.targets_sq(s, only);
        v.sort_unstable_by_key(|&t| by_name(t));
        v
    };
    let mut out = Vec::with_capacity(64);
    for p in pieces.iter().filter(|p| p.color == me) {
        let mut alive = [p.h[0]; 2];
        let mut n = 0;
        for h in p.halves().iter().filter(|h| h.alive) {
            alive[n] = *h;
            n += 1;
        }
        let alive = &alive[..n];
        for h in alive {
            let ts = targets(h.sq);
            let pawn = h.kind == Kind::Pawn;
            for &t in &ts {
                if captures_only && !enemy[t as usize] && !(pawn && last_rank(t)) {
                    continue;
                }
                let df = coords(t).0 - coords(h.sq).0;
                if h.kind == Kind::King && df.abs() == 2 {
                    let side = if df > 0 { KING_SIDE } else { QUEEN_SIDE };
                    out.push(Intent::Castle { side, split: false });
                    if quantum {
                        out.push(Intent::Castle { side, split: true });
                    }
                    continue;
                }
                let promotes = pawn && last_rank(t);
                if promotes {
                    for pr in [Kind::Queen, Kind::Knight, Kind::Rook, Kind::Bishop] {
                        out.push(from_to(h.kind, h.sq, t, Some(pr), None));
                    }
                    if quantum && !captures_only {
                        out.push(from_to(h.kind, h.sq, t, Some(Kind::Queen), to_sq(t, Some(Kind::Knight))));
                    }
                } else {
                    out.push(from_to(h.kind, h.sq, t, None, None));
                }
                if !quantum || captures_only {
                    continue;
                }
                let pr = promotes.then_some(Kind::Queen);
                for &t2 in ts.iter().filter(|&&t2| by_name(t2) > by_name(t)) {
                    let pr2 = (pawn && last_rank(t2)).then_some(Kind::Queen);
                    out.push(from_to(h.kind, h.sq, t, pr, to_sq(t2, pr2)));
                }
            }
        }
        if quantum && !captures_only && let [a, b] = *alive {
            let other = Hint { file: Some(file_of(b.sq)), rank: Some(rank_of(b.sq)) };
            for ta in targets(a.sq) {
                for tb in targets(b.sq) {
                    out.push(from_to(a.kind, a.sq, ta, None, Some(Second::To { hint: other, to: tb, promo: None })));
                }
            }
        }
    }
    out
}

/// Every legal move from `g` with the game after it. A move that forces a
/// collapse the mover chooses appears once per option. Moves that lead to
/// the same position are listed once.
pub fn children(g: &Game, quantum: bool) -> Vec<Child> {
    all_moves(g, quantum, false)
        .map(|n| Child { notation: g.notation_of(&n.mv.intent, n.mv.cube), game: n.game, victim: n.victim })
        .collect()
}

/// Value of the most valuable enemy half on each square.
fn victims(g: &Game) -> [i32; 64] {
    let me = g.to_move();
    let mut out = [0; 64];
    for p in views(g).iter().filter(|p| p.color != me) {
        for h in p.halves().iter().filter(|h| h.alive) {
            let v = &mut out[h.sq as usize];
            *v = (*v).max(value(h.kind).max(1));
        }
    }
    out
}

/// The most valuable enemy half on a destination square of `mv`.
fn victim_of(victims: &[i32; 64], mv: &Intent) -> i32 {
    match *mv {
        Intent::Castle { .. } => 0,
        Intent::Move { to, second, .. } => {
            let second = match second {
                Some(Second::To { to, .. }) => victims[to as usize],
                _ => 0,
            };
            victims[to as usize].max(second)
        }
    }
}

/// A move the search reached, with the game after it.
struct Node {
    mv: Mv,
    game: Game,
    victim: i32,
}

/// Plays candidate moves one at a time, best-looking first, so a search
/// that cuts off after the first few never pays for the rest. This is the
/// staged move generation of classical engines: playing a quantum move is
/// the expensive part (every board has to be checked), so it is done only
/// when the search actually reaches the move. The move the transposition
/// table remembers is tried before the other candidates are even listed: it
/// would sort first anyway, and when it causes a cutoff, listing the rest
/// (every target of every piece on every board) is saved.
struct Picker<'a, R: Fn(&Intent, i32) -> i64> {
    g: &'a Game,
    quantum: bool,
    captures_only: bool,
    rank: R,
    first: Option<Intent>,
    /// The first move once tried, left out of the list.
    skip: Option<Intent>,
    /// Listed when first needed.
    cands: Option<Vec<(Intent, i32)>>,
    next: usize,
    /// Collapse options of the last candidate, still to hand out.
    pending: Vec<Node>,
    seen: HashSet<u64, FastHash>,
    /// Leave out moves capturing less than this (promotions are kept), and
    /// note that some were left out. For futility and delta pruning.
    min_victim: Option<i32>,
    pruned: bool,
}

impl<'a, R: Fn(&Intent, i32) -> i64> Picker<'a, R> {
    /// `rank(move, victim)` orders candidates (lower first); ties keep whole
    /// moves ahead of quantum ones, so a split that collapses to the same
    /// position as a plain move is listed as the plain move. `first` must be
    /// the move `rank` puts first, if it is a candidate at all.
    fn new(g: &'a Game, quantum: bool, captures_only: bool, first: Option<Intent>, rank: R) -> Self {
        let done = g.status() != Status::Ongoing;
        Picker {
            g,
            quantum,
            captures_only,
            rank,
            first: if done { None } else { first },
            skip: None,
            cands: if done { Some(Vec::new()) } else { None },
            next: 0,
            pending: Vec::new(),
            seen: HashSet::default(),
            min_victim: None,
            pruned: false,
        }
    }

    fn expand(&mut self, intent: Intent, victim: i32) {
        if let Some(min) = self.min_victim
            && victim < min
            && !matches!(intent, Intent::Move { promo: Some(_), .. })
        {
            self.pruned = true;
            return;
        }
        let Ok(options) = self.g.expand_quiet(&intent) else { return };
        // `pending` is popped from the back: keep the options in order.
        for (cube, game) in options.into_iter().rev() {
            if self.seen.insert(game.position_key()) {
                self.pending.push(Node { mv: Mv { intent, cube }, game, victim });
            }
        }
    }
}

/// [`Picker`] with every candidate in the usual order.
fn all_moves(g: &Game, quantum: bool, captures_only: bool) -> Picker<'_, impl Fn(&Intent, i32) -> i64> {
    Picker::new(g, quantum, captures_only, None, |_: &Intent, _| 0)
}

impl<R: Fn(&Intent, i32) -> i64> Iterator for Picker<'_, R> {
    type Item = Node;

    fn next(&mut self) -> Option<Node> {
        loop {
            if let Some(c) = self.pending.pop() {
                return Some(c);
            }
            if let Some(f) = self.first.take() {
                self.skip = Some(f);
                let victim = victim_of(&victims(self.g), &f);
                self.expand(f, victim);
                continue;
            }
            if self.cands.is_none() {
                let v = victims(self.g);
                let mut tried: HashSet<Intent, FastHash> = HashSet::default();
                if let Some(f) = self.skip {
                    tried.insert(f);
                }
                let mut cands: Vec<(Intent, i32)> = candidates(self.g, self.quantum, self.captures_only)
                    .into_iter()
                    .filter(|m| tried.insert(*m))
                    .map(|m| (m, victim_of(&v, &m)))
                    .collect();
                let rank = &self.rank;
                cands.sort_by_cached_key(|(m, victim)| (rank(m, *victim) - *victim as i64 * 100, is_quantum(m)));
                self.cands = Some(cands);
                self.next = 0;
            }
            let (intent, victim) = *self.cands.as_ref().unwrap().get(self.next)?;
            self.next += 1;
            self.expand(intent, victim);
        }
    }
}

// ------------------------------------------------------------- search

#[derive(Clone, Copy, PartialEq)]
enum Bound {
    Exact,
    Lower,
    Upper,
}

struct Entry {
    depth: i32,
    score: i32,
    bound: Bound,
    best: Option<Mv>,
}

/// A search with its transposition table. Reuse one across moves of a game
/// to keep what it learned.
pub struct Searcher {
    pub cfg: Config,
    tt: HashMap<u64, Entry>,
    killers: Vec<[Option<Mv>; 2]>,
    nodes: u64,
    deadline: Option<Instant>,
    stopped: bool,
    rng: u64,
    /// Positions on the current line: the game so far plus the moves being
    /// searched. Reaching one again scores as a draw.
    path: Vec<u64>,
    /// The move into this node was a null move (two in a row prove nothing).
    null_last: bool,
}

impl Searcher {
    pub fn new(cfg: Config) -> Self {
        Searcher { cfg, tt: HashMap::new(), killers: Vec::new(), nodes: 0, deadline: None, stopped: false, rng: 0x2545_F491_4F6C_DD1D, path: Vec::new(), null_last: false }
    }

    /// Seed the random choice used when `margin > 0`.
    pub fn seed(mut self, seed: u64) -> Self {
        self.rng = seed | 1;
        self
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn out_of_time(&mut self) -> bool {
        if !self.stopped && self.nodes.is_multiple_of(64) && self.deadline.is_some_and(|d| Instant::now() >= d) {
            self.stopped = true;
        }
        self.stopped
    }

    /// Choose a move for the side to move. `None` if the game is over.
    pub fn choose(&mut self, g: &Game) -> Option<Choice> {
        let mut root: Vec<Node> = all_moves(g, self.cfg.quantum_moves, false).collect();
        if root.is_empty() {
            return None;
        }
        // Positions already seen in this game. Repeating one scores as a
        // draw, so a side that is winning looks for progress instead of
        // shuffling, and one that is losing can use repetition to draw.
        self.path = Game::replay(g.history())
            .map(|v| v.iter().map(Game::position_key).collect())
            .unwrap_or_else(|_| vec![g.position_key()]);
        self.nodes = 0;
        self.stopped = false;
        self.deadline = self.cfg.time.map(|t| Instant::now() + t);
        let mut scores: Vec<i32> = vec![0; root.len()];
        let mut done_depth = 0;
        for depth in 1..=self.cfg.depth.max(1) {
            self.order(g, &mut root, 0);
            let mut alpha = -INF;
            let mut best_i = 0;
            let mut these = Vec::with_capacity(root.len());
            for (i, c) in root.iter().enumerate() {
                // When choosing at random among near-best moves, widen the
                // window by the margin: every move within the margin of the
                // best then gets an exact score, and the rest fail low.
                let lo = if self.cfg.margin > 0 && alpha > -INF { alpha - self.cfg.margin } else { alpha };
                let s = -self.negamax(&c.game, depth - 1, 1, -INF, -lo);
                if self.stopped {
                    break;
                }
                // Later moves searched with a narrow window only return a
                // bound, so a tie with the best is not a real tie.
                if s > alpha {
                    alpha = s;
                    best_i = i;
                }
                these.push(s);
            }
            if self.stopped && depth > 1 {
                break;
            }
            // Keep root moves sorted best first for the next iteration.
            let mut idx: Vec<usize> = (0..these.len()).collect();
            if self.cfg.margin > 0 {
                // Scores within the margin are exact; the rest are below it.
                idx.sort_by_key(|&i| -these[i]);
            } else {
                idx.retain(|&i| i != best_i);
                idx.insert(0, best_i);
            }
            let mut slots: Vec<Option<Node>> = root.drain(..).map(Some).collect();
            let mut sorted: Vec<Node> = idx.iter().map(|&i| slots[i].take().unwrap()).collect();
            sorted.extend(slots.into_iter().skip(these.len()).flatten());
            scores = idx.iter().map(|&i| these[i]).collect();
            root = sorted;
            done_depth = depth;
            let best = &root[0];
            self.tt.insert(g.position_key(), Entry { depth, score: scores[0], bound: Bound::Exact, best: Some(best.mv) });
            if scores[0].abs() >= MATE - 100 || self.stopped {
                break;
            }
        }
        let best = scores[0];
        let near = if self.cfg.margin > 0 {
            scores.iter().take_while(|&&s| s >= best - self.cfg.margin).count().max(1)
        } else {
            1
        };
        let pick = if near > 1 { (self.rand() % near as u64) as usize } else { 0 };
        Some(Choice { notation: g.notation_of(&root[pick].mv.intent, root[pick].mv.cube), score: scores[pick], depth: done_depth, nodes: self.nodes })
    }

    /// Order moves best first: transposition-table move, captures by
    /// victim value, killers, then the rest (whole moves before splits).
    fn order(&self, g: &Game, moves: &mut [Node], ply: usize) {
        let tt_best = self.tt.get(&g.position_key()).and_then(|e| e.best);
        let killers = self.killers.get(ply).copied().unwrap_or_default();
        moves.sort_by_cached_key(|c| {
            let mut k = 0i64;
            if tt_best == Some(c.mv) {
                k -= 1_000_000;
            }
            k -= c.victim as i64 * 100;
            if killers.contains(&Some(c.mv)) {
                k -= 5_000;
            }
            if is_quantum(&c.mv.intent) {
                k += 10;
            }
            k
        });
    }

    fn negamax(&mut self, g: &Game, depth: i32, ply: usize, mut alpha: i32, beta: i32) -> i32 {
        self.nodes += 1;
        match g.status() {
            Status::Checkmate { .. } => return -(MATE - ply as i32),
            Status::Stalemate => return 0,
            _ => {}
        }
        if self.out_of_time() {
            return 0;
        }
        if depth <= 0 {
            return self.quiesce(g, ply, alpha, beta, self.cfg.quiescence);
        }
        let key = g.position_key();
        if self.path.contains(&key) {
            return 0;
        }
        if let Some(e) = self.tt.get(&key)
            && e.depth >= depth
        {
            let s = from_tt(e.score, ply);
            match e.bound {
                Bound::Exact => return s,
                Bound::Lower if s >= beta => return s,
                Bound::Upper if s <= alpha => return s,
                _ => {}
            }
        }
        // Null move pruning: if passing still leaves us at or above beta, a
        // real move will too (barring zugzwang), so cut without searching.
        let in_check = g.check() != crate::Check::None;
        if self.cfg.pruning && depth >= 3 && !in_check && beta.abs() < MATE - 1000 && !self.null_last && evaluate(g) >= beta {
            let null = g.null_move();
            if null.status() == Status::Ongoing {
                self.null_last = true;
                self.path.push(key);
                let s = -self.negamax(&null, depth - 1 - NULL_R, ply + 1, -beta, -beta + 1);
                self.path.pop();
                self.null_last = false;
                if self.stopped {
                    return 0;
                }
                if s >= beta {
                    return beta;
                }
            }
        }
        let null_last = std::mem::replace(&mut self.null_last, false);
        let tt_best = self.tt.get(&key).and_then(|e| e.best);
        let killers = self.killers.get(ply).copied().unwrap_or_default();
        let rank = |m: &Intent, _victim: i32| -> i64 {
            let mut k = 0;
            if tt_best.is_some_and(|b| b.intent == *m) {
                k -= 1_000_000;
            }
            if killers.iter().any(|x| x.is_some_and(|x| x.intent == *m)) {
                k -= 5_000;
            }
            k
        };
        let mut moves = Picker::new(g, self.cfg.quantum_moves, false, tt_best.map(|b| b.intent), rank);
        // Futility pruning: one ply from the horizon and well below alpha,
        // a quiet move won't bring the score back, so don't even play it.
        let mut futility = -INF;
        if self.cfg.pruning && depth == 1 && alpha.abs() < MATE - 1000 && beta.abs() < MATE - 1000 && !in_check {
            let margin = evaluate(g) + FUTILITY;
            if margin <= alpha {
                moves.min_victim = Some(1);
                futility = margin;
            }
        }
        let alpha0 = alpha;
        let mut best = -INF;
        let mut best_move = None;
        self.path.push(key);
        let mut i = 0;
        let mut quiet = 0;
        // Late move pruning: near the horizon, after this many quiet moves
        // the rest (mostly splits, sorted last) are left unplayed.
        let lmp = if self.cfg.pruning && depth <= 2 && !in_check && alpha.abs() < MATE - 1000 { LMP[depth as usize] } else { usize::MAX };
        while let Some(c) = moves.next() {
            let i = {
                i += 1;
                i - 1
            };
            if c.victim == 0 {
                quiet += 1;
                if quiet >= lmp {
                    moves.min_victim = Some(1);
                }
            }
            // Late move reduction: quiet moves late in the order are rarely
            // best, so look at them one ply shallower with a null window,
            // and search them fully only if they beat alpha.
            let s = if i >= 4 && depth >= 2 && c.victim == 0 {
                let r = -self.negamax(&c.game, depth - 2, ply + 1, -alpha - 1, -alpha);
                if r > alpha { -self.negamax(&c.game, depth - 1, ply + 1, -beta, -alpha) } else { r }
            } else {
                -self.negamax(&c.game, depth - 1, ply + 1, -beta, -alpha)
            };
            if self.stopped {
                self.path.pop();
                self.null_last = null_last;
                return 0;
            }
            if s > best {
                best = s;
                best_move = Some(c.mv);
            }
            alpha = alpha.max(s);
            if alpha >= beta {
                if c.victim == 0 {
                    if self.killers.len() <= ply {
                        self.killers.resize(ply + 1, [None, None]);
                    }
                    let k = &mut self.killers[ply];
                    if k[0] != Some(c.mv) {
                        k[1] = k[0].take();
                        k[0] = Some(c.mv);
                    }
                }
                break;
            }
        }
        self.path.pop();
        self.null_last = null_last;
        if moves.pruned {
            best = best.max(futility);
        }
        if best_move.is_none() {
            // Status says ongoing but nothing playable: treat as a draw;
            // everything pruned: the position is below alpha.
            return if moves.pruned { best } else { 0 };
        }
        let bound = if best <= alpha0 {
            Bound::Upper
        } else if best >= beta {
            Bound::Lower
        } else {
            Bound::Exact
        };
        self.tt.insert(key, Entry { depth, score: to_tt(best, ply), bound, best: best_move });
        best
    }

    /// Search captures (and promotions) only, until the position is quiet.
    fn quiesce(&mut self, g: &Game, ply: usize, mut alpha: i32, beta: i32, left: i32) -> i32 {
        self.nodes += 1;
        match g.status() {
            Status::Checkmate { .. } => return -(MATE - ply as i32),
            Status::Stalemate => return 0,
            _ => {}
        }
        let stand = evaluate(g);
        if stand >= beta || left <= 0 {
            return stand;
        }
        alpha = alpha.max(stand);
        let mut moves = all_moves(g, false, true);
        if self.cfg.pruning {
            // Delta pruning: a capture that can't lift the score near alpha
            // even if it wins its victim outright isn't worth playing.
            moves.min_victim = Some(alpha - stand - DELTA);
        }
        for c in moves {
            if self.out_of_time() {
                break;
            }
            let s = -self.quiesce(&c.game, ply + 1, -beta, -alpha, left - 1);
            if s >= beta {
                return s;
            }
            alpha = alpha.max(s);
        }
        alpha
    }
}

/// Mate scores are stored relative to the node, so they stay right when
/// the same position is reached at another ply.
fn to_tt(s: i32, ply: usize) -> i32 {
    if s >= MATE - 1000 {
        s + ply as i32
    } else if s <= -(MATE - 1000) {
        s - ply as i32
    } else {
        s
    }
}

fn from_tt(s: i32, ply: usize) -> i32 {
    if s >= MATE - 1000 {
        s - ply as i32
    } else if s <= -(MATE - 1000) {
        s + ply as i32
    } else {
        s
    }
}

/// Choose a move with a fresh searcher.
pub fn best_move(g: &Game, cfg: Config) -> Option<Choice> {
    Searcher::new(cfg).choose(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fen(f: &str) -> Game {
        Game::from_fen(f).unwrap()
    }

    fn quick(depth: i32) -> Config {
        Config { depth, time: None, ..Config::default() }
    }

    #[test]
    fn start_position_has_whole_and_quantum_moves() {
        let g = Game::new();
        let classical = children(&g, false);
        assert_eq!(classical.len(), 20, "{:?}", classical.iter().map(|c| &c.notation).collect::<Vec<_>>());
        let all = children(&g, true);
        assert!(all.iter().any(|c| c.notation == "Nb1a3/c3"));
        assert!(all.iter().any(|c| c.notation == "e2e3/e4"));
        // Never a stay move: it would leave boards where nothing moved.
        assert!(all.iter().all(|c| !c.notation.trim_end().ends_with('/')), "stay move generated");
        assert!(all.len() > 25, "{}", all.len());
    }

    #[test]
    fn every_collapse_choice_is_its_own_move() {
        // White knight split c3/a3, black pawn split d5/d6: Nc3d5/a3b5 forces a
        // collapse with two options.
        let mut g = Game::new();
        for m in ["Nc3/a3", "d5/6"] {
            g.play(m).unwrap();
        }
        let opts = g.collapse_options("Nc3d5/a3b5").unwrap();
        assert_eq!(opts.len(), 2);
        let kids: HashSet<u64> = children(&g, true).iter().map(|c| c.game.position_key()).collect();
        for o in opts {
            let mut c = g.clone();
            c.play(&format!("Nc3d5/a3b5 {o}")).unwrap();
            assert!(kids.contains(&c.position_key()), "{o}");
        }
    }

    #[test]
    fn finds_mate_in_one() {
        let g = fen("6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1");
        let c = best_move(&g, quick(2)).unwrap();
        assert_eq!(c.notation, "Ra1a8");
        assert!(c.score >= MATE - 10);
    }

    #[test]
    fn takes_a_hanging_queen() {
        let g = fen("4k3/8/8/3q4/8/8/8/3RK3 w - - 0 1");
        assert_eq!(best_move(&g, quick(2)).unwrap().notation, "Rd1d5");
    }

    #[test]
    fn does_not_hang_its_queen() {
        // Qxd7 (defended by the king) would lose the queen for a pawn.
        let g = fen("4k3/3p4/8/8/8/8/8/3QK3 w - - 0 1");
        let c = best_move(&g, Config { depth: 2, time: None, quantum_moves: false, ..Config::default() }).unwrap();
        assert_ne!(c.notation, "Qd1d7");
    }

    #[test]
    fn a_split_piece_counts_half_on_each_square() {
        let mut g = Game::new();
        let before = evaluate(&g);
        g.play("Nc3/a3").unwrap();
        // Black to move now; from white's side the knight is still worth
        // about a knight.
        let after = -evaluate(&g);
        assert!((after - before).abs() < 60, "{before} -> {after}");
    }

    #[test]
    fn plays_a_short_game_to_the_end_or_the_move_cap() {
        let mut g = Game::new();
        let mut s = Searcher::new(Config { depth: 1, time: None, margin: 30, ..Config::default() }).seed(7);
        for _ in 0..20 {
            let Some(c) = s.choose(&g) else { break };
            g.play(&c.notation).unwrap();
        }
        assert!(!g.history().is_empty());
    }
}
