#![allow(clippy::needless_range_loop)]
//! Quantum positions: pieces made of two halves, the set of possible
//! classical board states, and the move/collapse rules from the sheet.
//!
//! Model
//! -----
//! Every piece has two halves. An ordinary piece has both halves on the same
//! square. A split piece has halves on different squares (or one captured,
//! or two promotion kinds). The possible board states are every way of picking
//! one half per split piece (2^n of them).
//!
//! Playing a move:
//! 1. Work out which halves move where (from the notation) and the classical
//!    moves ("branches") this stands for in each state.
//! 2. Move the halves. Enemy halves on a destination square are captured.
//! 3. `reachable` = every board you get by playing a branch legally (regular
//!    chess rules, own king safe) from a previous state.
//! 4. A resulting state that isn't reachable is impossible, so pieces must
//!    collapse until none is left. That is rule 3 on the sheet. Rule 2 says use
//!    as few collapses as possible, and the mover chooses between equally small
//!    options.
//!
//! The sheet's consequences then fall out automatically: moving through a
//! quantum piece collapses it away, pawn captures take both halves, a
//! half-capture forces a collapse, and quantum check must be resolved.



use crate::QError;
use crate::board::*;
use crate::notation::{CollapseSpec, Hint, Intent, Second};
use crate::types::*;

/// Upper limit on simultaneously split pieces (2^16 = 65,536 board states).
pub const MAX_SPLIT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Half {
    pub sq: Sq,
    pub kind: Kind,
    /// `false` once captured; `sq` then remembers where.
    pub alive: bool,
    pub moved: bool,
}

impl Half {
    fn same(&self, o: &Half) -> bool {
        // Two captured halves are the same outcome: the piece is gone.
        (!self.alive && !o.alive) || (self.sq == o.sq && self.kind == o.kind && self.alive == o.alive)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub color: Color,
    pub h: [Half; 2],
    /// Kind the piece started the game as (a promoted pawn stays `Pawn`).
    pub origin: Kind,
    /// Number telling same-kind pieces apart (pawns 1-8 by file, rooks and
    /// knights 1-2 from the a-file side); 0 for unnumbered kinds.
    pub num: u8,
}

/// A collapse choice: bit `k` of the mask fixes split piece `k` to the half
/// in bit `k` of the value.
pub type Cube = (u32, u32);

/// One way a move can be played: the collapse picked (if the mover had a
/// choice), the position, and its boards when they are already known.
pub type Outcome = (Option<Cube>, Position, Option<States>);

/// At most two distinct halves, without allocating.
#[derive(Clone, Copy)]
struct Locs {
    items: [(usize, Sq, Kind); 2],
    len: usize,
}

impl std::ops::Deref for Locs {
    type Target = [(usize, Sq, Kind)];
    fn deref(&self) -> &Self::Target {
        &self.items[..self.len]
    }
}

impl IntoIterator for Locs {
    type Item = (usize, Sq, Kind);
    type IntoIter = std::iter::Take<std::array::IntoIter<(usize, Sq, Kind), 2>>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(self.len)
    }
}

impl std::hash::Hash for Piece {
    /// Everything about the piece packed into one word: positions are hashed
    /// for every move the AI looks at, and field-by-field hashing was a tenth
    /// of its time.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let half = |h: &Half| h.sq as u64 | (h.kind as u64) << 6 | (h.alive as u64) << 9 | (h.moved as u64) << 10;
        state.write_u64(
            half(&self.h[0]) | half(&self.h[1]) << 11 | (self.color as u64) << 22 | (self.origin as u64) << 23 | (self.num as u64) << 26,
        );
    }
}

fn subscript(n: u8) -> String {
    n.to_string().chars().map(|c| char::from_u32(0x2080 + c.to_digit(10).unwrap_or(0)).unwrap_or(c)).collect()
}

impl Piece {
    /// Short name for notation when its half of kind `kind` moves, e.g.
    /// `P₅`, `R₁`, `B`, or `Qₚ₅` for a queen promoted from pawn 5.
    pub fn label(&self, kind: Kind) -> String {
        let letter = kind.letter().to_ascii_uppercase();
        match (self.num, kind == self.origin) {
            (0, _) => letter.to_string(),
            (n, true) => format!("{letter}{}", subscript(n)),
            (n, false) => format!("{letter}\u{209A}{}", subscript(n)),
        }
    }

    /// Spoken name, e.g. `white rook 2` or `white queen (pawn 5)`.
    pub fn name(&self, kind: Kind) -> String {
        match (self.num, kind == self.origin) {
            (0, _) => format!("{} {}", self.color.name(), kind.name()),
            (n, true) => format!("{} {} {n}", self.color.name(), kind.name()),
            (n, false) => format!("{} {} ({} {n})", self.color.name(), kind.name(), self.origin.name()),
        }
    }

    pub fn is_split(&self) -> bool {
        !self.h[0].same(&self.h[1])
    }
    pub fn is_dead(&self) -> bool {
        !self.h[0].alive && !self.h[1].alive
    }
    /// Distinct living (square, kind) halves with a representative index.
    fn locs(&self) -> Locs {
        let mut v = Locs { items: [(0, 0, Kind::Pawn); 2], len: 0 };
        for (i, h) in self.h.iter().enumerate() {
            if h.alive && !v.iter().any(|&(_, s, k)| s == h.sq && k == h.kind) {
                v.items[v.len] = (i, h.sq, h.kind);
                v.len += 1;
            }
        }
        v
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct EpInfo {
    pub id: usize,
    pub landed: Sq,
    pub target: Sq,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Position {
    pub pieces: Vec<Piece>,
    pub turn: Color,
    pub ep: Option<EpInfo>,
}

/// Board identity used to compare states: piece id + kind on every square.
type Key = u128;

/// A fast non-cryptographic hasher (FxHash) for board keys and positions.
/// The default SipHash was a fifth of the AI's run time.
#[derive(Default, Clone, Copy)]
pub struct Fx(u64);

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.write_u64(u64::from_le_bytes(*c));
        }
        for &b in rest {
            self.write_u64(b as u64);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u8(&mut self, n: u8) {
        self.write_u64(n as u64);
    }
    fn write_u16(&mut self, n: u16) {
        self.write_u64(n as u64);
    }
    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }
    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}

pub type FastHash = std::hash::BuildHasherDefault<Fx>;

fn key(b: &Board) -> Key {
    b.key
}

#[derive(Debug)]
pub struct States {
    /// Ids of split pieces; bit `i` of a mask picks the half of `splits[i]`.
    pub splits: Vec<usize>,
    /// Valid states.
    pub list: Vec<(u32, Board)>,
    /// Masks whose half choices put two pieces on one square.
    pub invalid: Vec<u32>,
}

impl States {
    /// The boards left after collapse `(mask, val)` (bit `k` of `mask` fixes
    /// `splits[k]` to the half in bit `k` of `val`), indexed for the position
    /// after the collapse, whose split pieces are the others.
    fn restrict(&self, (mask, val): (u32, u32)) -> States {
        let keep: Vec<usize> = (0..self.splits.len()).filter(|&k| mask >> k & 1 == 0).collect();
        let squeeze = |m: u32| keep.iter().enumerate().fold(0u32, |acc, (i, &k)| acc | (m >> k & 1) << i);
        States {
            splits: keep.iter().map(|&k| self.splits[k]).collect(),
            list: self.list.iter().filter(|(m, _)| m & mask == val).map(|(m, b)| (squeeze(*m), b.clone())).collect(),
            invalid: self.invalid.iter().filter(|&&m| m & mask == val).map(|&m| squeeze(m)).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Action {
    To { to: Sq, promo: Option<Kind> },
    Castle(usize),
    RookTo { from: Sq, to: Sq },
}

/// A classical move that applies in the states where the moving piece's
/// half on `from` (of kind `kind`) is the real one.
#[derive(Clone, Copy, Debug)]
struct Branch {
    from: Sq,
    kind: Kind,
    action: Action,
}

struct Plan {
    piece: usize,
    /// New (square, kind) for each half, `None` = stays.
    dest: [Option<(Sq, Kind)>; 2],
    /// (rook piece, half, destination) for castling.
    rook: Option<(usize, usize, Sq)>,
    branches: Vec<Branch>,
    ep: Option<EpInfo>,
}

/// Result of a successful move.
pub struct MoveReport {
    pub collapses: Vec<String>,
    /// The piece that moved (the king when castling).
    pub piece: usize,
    /// Number of different minimal collapse choices that were available.
    pub options: usize,
    /// Squares the moving halves left, then the squares they arrived on
    /// (including the rook's squares when castling). No duplicates.
    pub moved: Vec<Sq>,
    /// The new position's boards, when they were already worked out
    /// (nothing collapsed), so the caller needn't list them again.
    pub states: Option<States>,
}

/// Everything about a move that is known before a collapse is chosen.
/// Shared by [`Position::play`] and [`Position::collapse_options`].
struct Analysis {
    plan: Plan,
    /// The position after the move, before any collapse is applied.
    next: Position,
    /// The boards of `next`.
    states: States,
    /// Split pieces of `next`; bit `k` of a cube refers to `splits[k]`.
    splits: Vec<usize>,
    /// Minimal collapse choices as (mask, value); empty if none is needed.
    cubes: Vec<(u32, u32)>,
    /// For each branch of the plan: legal in at least one previous state.
    used: Vec<bool>,
}

impl Position {
    pub fn initial() -> Self {
        Self::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1").expect("valid start")
    }

    /// Classical position from FEN (placement, side, castling, en passant).
    pub fn from_fen(fen: &str) -> Result<Self, QError> {
        let bad = |w: &str| QError(format!("bad FEN: {w}"));
        let mut parts = fen.split_whitespace();
        let placement = parts.next().ok_or_else(|| bad("empty"))?;
        let turn = match parts.next().unwrap_or("w") {
            "w" => Color::White,
            "b" => Color::Black,
            _ => return Err(bad("side to move")),
        };
        let castling = parts.next().unwrap_or("-");
        let ep = parts.next().unwrap_or("-");

        let mut pieces = Vec::new();
        let rows: Vec<&str> = placement.split('/').collect();
        if rows.len() != 8 {
            return Err(bad("need 8 ranks"));
        }
        for (i, row) in rows.iter().enumerate() {
            let rank = 7 - i as u8;
            let mut file = 0u8;
            for c in row.chars() {
                if let Some(d) = c.to_digit(10) {
                    file += d as u8;
                    continue;
                }
                let kind = Kind::from_letter(c.to_ascii_uppercase()).ok_or_else(|| bad("piece letter"))?;
                let color = if c.is_ascii_uppercase() { Color::White } else { Color::Black };
                if file > 7 {
                    return Err(bad("rank too long"));
                }
                let s = sq(file, rank);
                let unmoved = match kind {
                    Kind::King => {
                        let (k, q) = rights_letters(color);
                        s == sq(4, color.home_rank()) && (castling.contains(k) || castling.contains(q))
                    }
                    Kind::Rook => {
                        let (k, q) = rights_letters(color);
                        (s == sq(7, color.home_rank()) && castling.contains(k))
                            || (s == sq(0, color.home_rank()) && castling.contains(q))
                    }
                    _ => false,
                };
                let h = Half {
                    sq: s,
                    kind,
                    alive: true,
                    moved: !unmoved,
                };
                pieces.push(Piece { color, h: [h, h], origin: kind, num: 0 });
                file += 1;
            }
            if file != 8 {
                return Err(bad("rank length"));
            }
        }
        if pieces.len() > 64 {
            return Err(bad("too many pieces"));
        }
        // Number pawns, rooks and knights per color from the a-file side.
        for color in [Color::White, Color::Black] {
            for kind in [Kind::Pawn, Kind::Rook, Kind::Knight] {
                let mut ids: Vec<usize> =
                    (0..pieces.len()).filter(|&i| pieces[i].color == color && pieces[i].origin == kind).collect();
                ids.sort_by_key(|&i| (file_of(pieces[i].h[0].sq), rank_of(pieces[i].h[0].sq)));
                for (n, i) in ids.into_iter().enumerate() {
                    pieces[i].num = (n + 1).min(255) as u8;
                }
            }
        }
        let mut pos = Position {
            pieces,
            turn,
            ep: None,
        };
        if ep != "-" {
            let target = parse_sq(ep).ok_or_else(|| bad("en passant square"))?;
            let landed = if turn == Color::White { target - 8 } else { target + 8 };
            if let Some(id) = pos.pieces.iter().position(|p| p.h[0].alive && p.h[0].sq == landed && p.h[0].kind == Kind::Pawn) {
                pos.ep = Some(EpInfo { id, landed, target });
            }
        }
        Ok(pos)
    }

    pub fn splits(&self) -> Vec<usize> {
        (0..self.pieces.len()).filter(|&i| self.pieces[i].is_split()).collect()
    }

    /// Castling rights and the en passant target of a board whose pieces
    /// are placed; `moved[s]` says whether the half on `s` has moved.
    fn finish_board(&self, b: &mut Board, moved: &[bool; 64]) {
        for c in [Color::White, Color::Black] {
            let home = c.home_rank();
            let at = |f: u8, k: Kind| {
                let s = sq(f, home);
                !moved[s as usize] && matches!(b.sq[s as usize], Some(o) if o.color == c && o.kind == k)
            };
            if at(4, Kind::King) {
                b.castle[c.idx()] = [at(7, Kind::Rook), at(0, Kind::Rook)];
            }
        }
        if let Some(e) = self.ep
            && matches!(b.sq[e.landed as usize], Some(o) if o.id as usize == e.id && o.kind == Kind::Pawn)
        {
            b.ep = Some(e.target);
        }
    }

    /// Every way of picking one half per split piece: the boards where no
    /// two pieces share a square, and the masks where they do. The pieces
    /// that aren't split are placed once, on a base board every state
    /// starts from.
    pub fn states(&self) -> Result<States, QError> {
        let splits = self.splits();
        if splits.len() > MAX_SPLIT {
            return Err(QError(format!(
                "too many pieces in superposition ({}; the limit is {MAX_SPLIT})",
                splits.len()
            )));
        }
        let place = |b: &mut Board, moved: &mut [bool; 64], id: usize, h: Half| -> bool {
            if !h.alive {
                return true;
            }
            if b.sq[h.sq as usize].is_some() {
                return false;
            }
            b.place(h.sq, Occ { id: id as u8, color: self.pieces[id].color, kind: h.kind });
            moved[h.sq as usize] = h.moved;
            true
        };
        let mut base = Board::empty(self.turn);
        let mut base_moved = [true; 64];
        let mut ok = true;
        for (id, p) in self.pieces.iter().enumerate() {
            if !p.is_split() {
                ok &= place(&mut base, &mut base_moved, id, p.h[0]);
            }
        }
        let mut list = Vec::with_capacity(1 << splits.len());
        let mut invalid = Vec::new();
        for mask in 0..(1u32 << splits.len()) {
            let mut b = base.clone();
            let mut moved = base_moved;
            let fits = ok
                && splits
                    .iter()
                    .enumerate()
                    .all(|(k, &id)| place(&mut b, &mut moved, id, self.pieces[id].h[(mask >> k & 1) as usize]));
            if fits {
                self.finish_board(&mut b, &moved);
                list.push((mask, b));
            } else {
                invalid.push(mask);
            }
        }
        Ok(States { splits, list, invalid })
    }

    // ------------------------------------------------------------------
    // Playing moves
    // ------------------------------------------------------------------

    /// Work out a move without collapsing anything: the resulting position
    /// and every minimal collapse the mover could choose between.
    fn analyze(&self, prev: &States, intent: &Intent) -> Result<Analysis, QError> {
        let plan = self.plan(prev, intent)?;
        // Every board reachable by a legal classical branch from a previous state.
        let mut reach: Vec<Key> = Vec::with_capacity(prev.list.len() * plan.branches.len());
        let mut real_move = false;
        let mut used = vec![false; plan.branches.len()];
        for (_, b) in &prev.list {
            for (bi, br) in plan.branches.iter().enumerate() {
                let here = matches!(b.sq[br.from as usize],
                    Some(o) if o.id as usize == plan.piece && o.kind == br.kind);
                if !here {
                    continue;
                }
                let m = match br.action {
                    Action::To { to, promo } => b.find_move(br.from, to, promo),
                    Action::Castle(side) => b.find_castle(br.from, side),
                    Action::RookTo { from, to } => b.find_move(from, to, None),
                };
                if let Some(m) = m {
                    reach.push(b.key_after(m));
                    real_move = true;
                    used[bi] = true;
                }
            }
        }
        if !real_move {
            return Err(QError("illegal move: not legal in any possible board state".into()));
        }

        reach.sort_unstable();
        let next = self.apply_plan(&plan);
        let ns = next.states()?;
        let n = ns.splits.len();
        let mut bad = vec![false; 1usize << n];
        for &m in &ns.invalid {
            bad[m as usize] = true;
        }
        for (m, b) in &ns.list {
            if reach.binary_search(&key(b)).is_err() {
                bad[*m as usize] = true;
            }
        }
        if bad.iter().all(|&x| x) {
            return Err(QError("illegal move: no consistent outcome".into()));
        }
        let cubes = if bad.iter().any(|&x| x) {
            minimal_collapses(n, &bad)
        } else {
            Vec::new()
        };
        Ok(Analysis {
            plan,
            next,
            splits: ns.splits.clone(),
            states: ns,
            cubes,
            used,
        })
    }

    /// Destination squares among `only` (a bitboard) that the side to
    /// move's piece on `from` can legally reach in at least one possible
    /// state (castling and promotion included). Sorted and deduplicated. A
    /// square already found legal on one board isn't tested on the others.
    pub fn targets(&self, st: &States, from: Sq, only: u64) -> Vec<Sq> {
        let mut found = 0u64;
        for (_, b) in &st.list {
            b.each_pseudo(from, &mut |m| {
                let bit = 1u64 << m.to;
                if only & bit != 0 && found & bit == 0 && b.is_legal(m) {
                    found |= bit;
                }
            });
        }
        let mut out = Vec::with_capacity(found.count_ones() as usize);
        while found != 0 {
            out.push(found.trailing_zeros() as Sq);
            found &= found - 1;
        }
        out
    }

    /// The minimal collapse choices for a move, as `{...}` specs that
    /// [`Position::play`] accepts. Empty when the move needs no collapse.
    pub fn collapse_options(&self, prev: &States, intent: &Intent) -> Result<Vec<String>, QError> {
        let a = self.analyze(prev, intent)?;
        Ok(a.cubes.iter().map(|c| a.next.describe_spec(&a.splits, *c)).collect())
    }

    /// Play a move. Returns the new position and what collapsed.
    pub fn play(
        &self,
        prev: &States,
        intent: &Intent,
        choice: Option<&[CollapseSpec]>,
        quiet: bool,
    ) -> Result<(Position, MoveReport), QError> {
        let mut a = self.analyze(prev, intent)?;
        let cube = if a.cubes.is_empty() {
            if choice.is_some() {
                return Err(QError("no collapse is needed for this move".into()));
            }
            None
        } else {
            Some(match choice {
                Some(spec) => {
                    let cube = a.next.cube_from_spec(&a.splits, spec)?;
                    if !a.cubes.contains(&cube) {
                        let opts: Vec<String> = a.cubes.iter().map(|c| a.next.describe_spec(&a.splits, *c)).collect();
                        return Err(QError(format!(
                            "that collapse isn't a valid minimal choice here; options: {}",
                            opts.join(" ")
                        )));
                    }
                    cube
                }
                None => a.next.rank_collapses(&a.splits, &a.cubes, a.plan.piece),
            })
        };
        let (next, mut report, changed) = self.finish(&a, intent, cube, quiet);
        report.states = Self::child_states(&mut a, cube, changed, &next);
        Ok((next, report))
    }

    /// The boards of the position a move produced, taken from its analysis
    /// instead of listed again: all of them when nothing collapsed, the ones
    /// the collapse kept otherwise. `None` when tidying the position changed
    /// it (rare), so they must be listed afresh.
    fn child_states(a: &mut Analysis, cube: Option<(u32, u32)>, changed: bool, next: &Position) -> Option<States> {
        if changed {
            return None;
        }
        let st = match cube {
            None => std::mem::replace(&mut a.states, States { splits: Vec::new(), list: Vec::new(), invalid: Vec::new() }),
            Some(c) => a.states.restrict(c),
        };
        #[cfg(debug_assertions)]
        {
            let fresh = next.states().expect("states");
            assert_eq!(fresh.splits, st.splits);
            assert_eq!(fresh.invalid, st.invalid);
            assert_eq!(fresh.list.len(), st.list.len());
            for ((m1, b1), (m2, b2)) in fresh.list.iter().zip(&st.list) {
                assert_eq!((m1, b1.key, b1.castle, b1.ep, b1.turn), (m2, b2.key, b2.castle, b2.ep, b2.turn));
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = next;
        Some(st)
    }

    /// Every way a move can be played: one entry when it needs no collapse
    /// or only one, otherwise one per minimal collapse the mover could
    /// choose, with the collapse ("cube") that picks it. The move is worked
    /// out once, for search; [`Position::describe_cube`] names a cube.
    pub fn play_each(&self, prev: &States, intent: &Intent) -> Result<Vec<Outcome>, QError> {
        let mut a = self.analyze(prev, intent)?;
        if a.cubes.len() <= 1 {
            let cube = a.cubes.first().copied();
            let (next, _, changed) = self.finish(&a, intent, cube, true);
            let states = Self::child_states(&mut a, cube, changed, &next);
            return Ok(vec![(None, next, states)]);
        }
        let cubes = a.cubes.clone();
        Ok(cubes
            .into_iter()
            .map(|c| {
                let (next, _, changed) = self.finish(&a, intent, Some(c), true);
                let states = Self::child_states(&mut a, Some(c), changed, &next);
                (Some(c), next, states)
            })
            .collect())
    }

    /// The `{...}` that picks collapse `cube` of a move, as
    /// [`Position::play_each`] returned it.
    pub fn describe_cube(&self, prev: &States, intent: &Intent, cube: (u32, u32)) -> Result<String, QError> {
        let a = self.analyze(prev, intent)?;
        Ok(a.next.describe_spec(&a.splits, cube))
    }

    /// Apply the chosen collapse (if any) to an analysed move. The flag says
    /// whether tidying the result changed it (then the analysis's boards
    /// don't describe it).
    fn finish(&self, a: &Analysis, intent: &Intent, cube: Option<(u32, u32)>, quiet: bool) -> (Position, MoveReport, bool) {
        let plan = &a.plan;
        let mut next = a.next.clone();
        let mut report = MoveReport {
            collapses: Vec::new(),
            options: a.cubes.len().max(1),
            piece: plan.piece,
            moved: if quiet { Vec::new() } else { self.moved_squares(plan) },
            states: None,
        };
        let mut mover_collapsed = false;
        if let Some(chosen) = cube {
            for (k, &id) in a.splits.iter().enumerate() {
                if chosen.0 >> k & 1 == 1 {
                    let hi = (chosen.1 >> k & 1) as usize;
                    mover_collapsed |= id == plan.piece;
                    if !quiet {
                        report.collapses.push(next.describe_collapse(id, hi));
                    }
                    next.pieces[id].h[1 - hi] = next.pieces[id].h[hi];
                }
            }
        }
        // Name the implicit collapse of a whole-piece move, unless the
        // mover's collapse is already listed above.
        if !quiet && !mover_collapsed && let Some(c) = self.whole_move_collapse(intent, plan, &a.used) {
            report.collapses.insert(0, c);
        }
        let changed = next.normalize();
        (next, report, changed)
    }

    /// Squares left and squares reached by the halves a plan moves.
    fn moved_squares(&self, plan: &Plan) -> Vec<Sq> {
        let mut from = Vec::new();
        let mut to = Vec::new();
        for hi in 0..2 {
            if let Some((t, _)) = plan.dest[hi] {
                from.push(self.pieces[plan.piece].h[hi].sq);
                to.push(t);
            }
        }
        if let Some((r, rh, t)) = plan.rook {
            from.push(self.pieces[r].h[rh].sq);
            to.push(t);
        }
        let mut out: Vec<Sq> = Vec::new();
        for s in from.into_iter().chain(to) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    fn plan(&self, prev: &States, intent: &Intent) -> Result<Plan, QError> {
        let me = self.turn;
        match *intent {
            Intent::Castle { side, split } => {
                let home = me.home_rank();
                let e = sq(4, home);
                let king = self
                    .pieces
                    .iter()
                    .position(|p| p.color == me && p.h.iter().any(|h| h.alive && h.kind == Kind::King))
                    .ok_or_else(|| QError("no king".into()))?;
                let p = &self.pieces[king];
                let hc = (0..2)
                    .find(|&i| p.h[i].alive && p.h[i].sq == e && p.h[i].kind == Kind::King && !p.h[i].moved)
                    .ok_or_else(|| QError("illegal move: the king has no half that can still castle".into()))?;
                let (rf, rt, kt) = if side == KING_SIDE { (7, 5, 6) } else { (0, 3, 2) };
                let corner = sq(rf, home);
                let (rook, rh) = self
                    .pieces
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.color == me)
                    .find_map(|(id, p)| {
                        (0..2)
                            .find(|&i| p.h[i].alive && p.h[i].sq == corner && p.h[i].kind == Kind::Rook && !p.h[i].moved)
                            .map(|i| (id, i))
                    })
                    .ok_or_else(|| QError("illegal move: no unmoved rook to castle with".into()))?;
                let ho = 1 - hc;
                let mut dest = [None, None];
                dest[hc] = Some((sq(kt, home), Kind::King));
                let mut branches = vec![Branch {
                    from: e,
                    kind: Kind::King,
                    action: Action::Castle(side),
                }];
                if split {
                    let other = p.h[ho];
                    if !other.alive {
                        return Err(QError("illegal move: can't quantum castle with a half-captured king".into()));
                    }
                    branches.push(Branch {
                        from: other.sq,
                        kind: Kind::King,
                        action: Action::RookTo {
                            from: corner,
                            to: sq(rt, home),
                        },
                    });
                } else if p.h[ho].alive {
                    dest[ho] = Some((sq(kt, home), Kind::King));
                }
                Ok(Plan {
                    piece: king,
                    dest,
                    rook: Some((rook, rh, sq(rt, home))),
                    branches,
                    ep: None,
                })
            }
            Intent::Move {
                kind,
                hint,
                to,
                promo,
                second,
            } => self.plan_move(prev, kind, hint, to, promo, second),
        }
    }

    fn reach(prev: &States, id: usize, from: Sq, kind: Kind, to: Sq, promo: Option<Kind>) -> bool {
        prev.list.iter().any(|(_, b)| {
            matches!(b.sq[from as usize], Some(o) if o.id as usize == id && o.kind == kind)
                && b.find_move(from, to, promo).is_some()
        })
    }

    fn plan_move(
        &self,
        prev: &States,
        kind: Kind,
        hint: Hint,
        to: Sq,
        promo: Option<Kind>,
        second: Option<Second>,
    ) -> Result<Plan, QError> {
        let me = self.turn;
        // Halves of the right kind that can reach `to` in some state.
        let mut cands: Vec<(usize, usize, Sq, Kind)> = Vec::new();
        for (id, p) in self.pieces.iter().enumerate() {
            if p.color != me {
                continue;
            }
            for (hi, s, k) in p.locs() {
                if k == kind && hint.matches(s) && Self::reach(prev, id, s, k, to, promo) {
                    cands.push((id, hi, s, k));
                }
            }
        }
        // A split like `Ne7/f6` names the piece that can reach both squares.
        if let Some(Second::To {
            hint: hint2,
            to: to2,
            promo: promo2,
        }) = second
        {
            cands.retain(|&(id, _, _, _)| {
                self.pieces[id]
                    .locs()
                    .iter()
                    .any(|&(_, s, k)| k == kind && hint2.matches(s) && Self::reach(prev, id, s, k, to2, promo2))
            });
        }
        let mut ids: Vec<usize> = cands.iter().map(|c| c.0).collect();
        ids.dedup();
        let piece = match ids.as_slice() {
            [] => {
                return Err(QError(format!(
                    "illegal move: no {} can move to {}",
                    kind.name(),
                    sq_name(to)
                )));
            }
            [one] => *one,
            _ => {
                let from: Vec<String> = cands.iter().map(|c| sq_name(c.2)).collect();
                return Err(QError(format!(
                    "ambiguous: {}s on {} can all reach {}",
                    kind.name(),
                    from.join(", "),
                    sq_name(to)
                )));
            }
        };
        let p = &self.pieces[piece];
        let locs = p.locs();
        let new_kind = |k: Kind, dst: Sq, pr: Option<Kind>| {
            let last = if me == Color::White { 7 } else { 0 };
            if k == Kind::Pawn && rank_of(dst) == last {
                pr.unwrap_or(Kind::Queen)
            } else {
                k
            }
        };
        let to_branch = |(_, s, k): (usize, Sq, Kind), dst: Sq, pr: Option<Kind>| Branch {
            from: s,
            kind: k,
            action: Action::To { to: dst, promo: pr },
        };
        let mut dest = [None, None];
        let mut branches = Vec::new();

        match second {
            None => {
                for hi in 0..2 {
                    let h = p.h[hi];
                    if h.alive {
                        dest[hi] = Some((to, new_kind(h.kind, to, promo)));
                    }
                }
                for l in locs.iter() {
                    branches.push(to_branch(*l, to, promo));
                }
            }
            Some(Second::Stay) => {
                // On the boards where the staying half was the real one,
                // nothing would have moved: not a legal chess move. Only
                // quantum castling leaves half the king home, because the
                // rook moves on those boards.
                return Err(QError(
                    "illegal move: a piece can't leave half of itself behind (only quantum castling, 0-0/ or 0-0-0/, keeps half the king home)".into(),
                ));
            }
            Some(Second::To {
                hint: hint2,
                to: to2,
                promo: promo2,
            }) => {
                if to2 == to && promo2 == promo {
                    return Err(QError("both halves can't make the same move; drop the '/'".into()));
                }
                if !p.is_split() {
                    let l = locs[0];
                    if !hint2.matches(l.1) || !Self::reach(prev, piece, l.1, l.2, to2, promo2) {
                        return Err(QError(format!("illegal move: the {} can't also reach {}", kind.name(), sq_name(to2))));
                    }
                    dest[0] = Some((to, new_kind(l.2, to, promo)));
                    dest[1] = Some((to2, new_kind(l.2, to2, promo2)));
                    branches.push(to_branch(l, to, promo));
                    branches.push(to_branch(l, to2, promo2));
                } else {
                    if locs.len() < 2 {
                        return Err(QError("illegal move: can't split a piece that is partly captured".into()));
                    }
                    let (a, b) = (locs[0], locs[1]);
                    let ok = |x: (usize, Sq, Kind), y: (usize, Sq, Kind)| {
                        x.2 == kind
                            && y.2 == kind
                            && hint.matches(x.1)
                            && hint2.matches(y.1)
                            && Self::reach(prev, piece, x.1, x.2, to, promo)
                            && Self::reach(prev, piece, y.1, y.2, to2, promo2)
                    };
                    let (x, y) = match (ok(a, b), ok(b, a)) {
                        (true, false) => (a, b),
                        (false, true) => (b, a),
                        (true, true) => {
                            return Err(QError(format!(
                                "ambiguous: either half could go to {}; disambiguate like {}{}/{}{}",
                                sq_name(to),
                                sq_name(a.1),
                                sq_name(to),
                                sq_name(b.1),
                                sq_name(to2)
                            )));
                        }
                        (false, false) => {
                            return Err(QError(format!(
                                "illegal move: the halves can't reach {} and {}",
                                sq_name(to),
                                sq_name(to2)
                            )));
                        }
                    };
                    dest[x.0] = Some((to, new_kind(x.2, to, promo)));
                    dest[y.0] = Some((to2, new_kind(y.2, to2, promo2)));
                    branches.push(to_branch(x, to, promo));
                    branches.push(to_branch(y, to2, promo2));
                }
            }
        }

        // En passant becomes available after a two-square pawn push.
        let mut ep = None;
        for hi in 0..2 {
            if let Some((t, _)) = dest[hi] {
                let f = p.h[hi].sq;
                // Only a real double push: from the pawn's own second rank,
                // two squares forward (not, say, an impossible branch of a
                // whole-piece move that happens to span two ranks).
                let (start, fwd) = if p.color == Color::White { (1, 3) } else { (6, 4) };
                if p.h[hi].kind == Kind::Pawn && file_of(f) == file_of(t) && rank_of(f) == start && rank_of(t) == fwd {
                    ep = Some(EpInfo {
                        id: piece,
                        landed: t,
                        target: (f + t) / 2,
                    });
                }
            }
        }
        Ok(Plan {
            piece,
            dest,
            rook: None,
            branches,
            ep,
        })
    }

    fn apply_plan(&self, plan: &Plan) -> Position {
        let me = self.turn;
        let mut n = self.clone();
        n.turn = me.other();
        n.ep = plan.ep;
        let kill_at = |n: &mut Position, s: Sq| {
            for q in n.pieces.iter_mut().filter(|q| q.color != me) {
                for h in q.h.iter_mut() {
                    if h.alive && h.sq == s {
                        h.alive = false;
                    }
                }
            }
        };
        for hi in 0..2 {
            if let Some((to, kind)) = plan.dest[hi] {
                let old = self.pieces[plan.piece].h[hi];
                n.pieces[plan.piece].h[hi] = Half {
                    sq: to,
                    kind,
                    alive: true,
                    moved: true,
                };
                kill_at(&mut n, to);
                if let Some(e) = self.ep
                    && old.kind == Kind::Pawn && file_of(old.sq) != file_of(to) && e.target == to {
                        for h in n.pieces[e.id].h.iter_mut() {
                            if h.alive && h.sq == e.landed {
                                h.alive = false;
                            }
                        }
                    }
            }
        }
        if let Some((r, rh, to)) = plan.rook {
            let corner = self.pieces[r].h[rh].sq;
            for h in n.pieces[r].h.iter_mut().filter(|h| h.alive && h.sq == corner) {
                h.sq = to;
                h.moved = true;
            }
        }
        n
    }

    /// Merge halves that ended up identical. Says whether anything changed.
    fn normalize(&mut self) -> bool {
        let mut changed = false;
        for p in &mut self.pieces {
            if !p.is_split() {
                let moved = p.h[0].moved || p.h[1].moved;
                let merged = Half { moved, ..p.h[0] };
                changed |= p.h[0] != merged || p.h[1] != merged;
                p.h = [merged, merged];
            }
        }
        if let Some(e) = self.ep {
            let p = &self.pieces[e.id];
            if !p.h.iter().any(|h| h.alive && h.sq == e.landed) {
                self.ep = None;
                changed = true;
            }
        }
        changed
    }

    fn cube_from_spec(&self, splits: &[usize], spec: &[CollapseSpec]) -> Result<(u32, u32), QError> {
        let (mut mask, mut val) = (0u32, 0u32);
        for c in spec {
            let mut hits = Vec::new();
            for (k, &id) in splits.iter().enumerate() {
                for hi in 0..2 {
                    let h = self.pieces[id].h[hi];
                    if h.sq == c.sq
                        && h.alive != c.dead
                        && c.kind.is_none_or(|kd| kd == h.kind)
                        && c.num.is_none_or(|n| n == self.pieces[id].num)
                    {
                        hits.push((k, hi));
                    }
                }
            }
            hits.dedup_by_key(|x| x.0);
            let &[(k, hi)] = hits.as_slice() else {
                return Err(QError(format!(
                    "collapse choice {}{}{}{} doesn't name exactly one quantum piece",
                    if c.dead { "x" } else { "" },
                    c.kind.map(|k| k.letter().to_string()).unwrap_or_default(),
                    c.num.map(|n| n.to_string()).unwrap_or_default(),
                    sq_name(c.sq)
                )));
            };
            mask |= 1 << k;
            val |= (hi as u32) << k;
        }
        Ok((mask, val))
    }

    /// `{...}` token keeping half `hi` of piece `id`. The piece number is
    /// added only when another quantum piece has a matching half there.
    fn spec_token(&self, splits: &[usize], id: usize, hi: usize) -> String {
        let h = self.pieces[id].h[hi];
        let clash = splits.iter().any(|&o| {
            o != id && self.pieces[o].h.iter().any(|x| x.sq == h.sq && x.kind == h.kind && x.alive == h.alive)
        });
        format!(
            "{}{}{}{}",
            if h.alive { "" } else { "x" },
            h.kind.letter(),
            if clash { self.pieces[id].num.to_string() } else { String::new() },
            sq_name(h.sq)
        )
    }

    fn describe_spec(&self, splits: &[usize], cube: (u32, u32)) -> String {
        let toks: Vec<String> = splits
            .iter()
            .enumerate()
            .filter(|(k, _)| cube.0 >> k & 1 == 1)
            .map(|(k, &id)| self.spec_token(splits, id, (cube.1 >> k & 1) as usize))
            .collect();
        format!("{{{}}}", toks.join(","))
    }

    /// Moving a split piece as a whole (`Nd5`, no `/`) when only one of its
    /// halves can make the move collapses it onto that half first. The piece
    /// ends up unsplit, so this does not show up among the cubes; name it.
    fn whole_move_collapse(&self, intent: &Intent, plan: &Plan, used: &[bool]) -> Option<String> {
        if !matches!(intent, Intent::Move { second: None, .. }) {
            return None;
        }
        let p = &self.pieces[plan.piece];
        if p.locs().len() != 2 {
            return None;
        }
        let live: Vec<usize> = (0..plan.branches.len()).filter(|&i| used[i]).collect();
        let [only] = live[..] else { return None };
        let from = plan.branches[only].from;
        let hi = (0..2).find(|&h| p.h[h].alive && p.h[h].sq == from)?;
        Some(self.describe_collapse(plan.piece, hi))
    }

    fn describe_collapse(&self, id: usize, hi: usize) -> String {
        let p = &self.pieces[id];
        let h = p.h[hi];
        if h.alive {
            format!("{} collapsed to {}", p.name(h.kind), sq_name(h.sq))
        } else {
            format!("{} collapsed to captured (on {})", p.name(h.kind), sq_name(h.sq))
        }
    }

    /// Default choice among minimal collapses: capture as much as possible
    /// for the mover, then avoid collapsing the piece that just moved.
    fn rank_collapses(&self, splits: &[usize], cubes: &[(u32, u32)], mover: usize) -> (u32, u32) {
        let me = self.turn.other(); // `self` is the post-move position
        let score = |c: &(u32, u32)| {
            let mut enemy_dead = 0i32;
            let mut touches_mover = 0i32;
            for (id, p) in self.pieces.iter().enumerate() {
                let mut h = p.h;
                if let Some(k) = splits.iter().position(|&s| s == id)
                    && c.0 >> k & 1 == 1 {
                        let hi = (c.1 >> k & 1) as usize;
                        h = [p.h[hi], p.h[hi]];
                        if id == mover {
                            touches_mover = 1;
                        }
                    }
                if p.color != me {
                    enemy_dead += h.iter().filter(|x| !x.alive).count() as i32;
                }
            }
            (enemy_dead, -touches_mover)
        };
        let mut best = cubes[0];
        for c in &cubes[1..] {
            if score(c) > score(&best) {
                best = *c;
            }
        }
        best
    }
}

fn rights_letters(c: Color) -> (char, char) {
    match c {
        Color::White => ('K', 'Q'),
        Color::Black => ('k', 'q'),
    }
}

/// All smallest "cubes" (fixed bits `mask` set to `val`) that exclude every
/// bad state while keeping at least one good state.
fn minimal_collapses(n: usize, bad: &[bool]) -> Vec<(u32, u32)> {
    let total = 1usize << n;
    // Only bits that influence badness can matter.
    let relevant: Vec<usize> = (0..n)
        .filter(|&j| (0..total).any(|m| bad[m] != bad[m ^ (1 << j)]))
        .collect();
    let bad_list: Vec<u32> = (0..total as u32).filter(|&m| bad[m as usize]).collect();
    let good_list: Vec<u32> = (0..total as u32).filter(|&m| !bad[m as usize]).collect();
    let r = relevant.len();
    for k in 0..=r {
        let mut found = Vec::new();
        for sub in 0u32..(1 << r) {
            if sub.count_ones() as usize != k {
                continue;
            }
            let mut mask = 0u32;
            for (i, &bit) in relevant.iter().enumerate() {
                if sub >> i & 1 == 1 {
                    mask |= 1 << bit;
                }
            }
            for v in 0u32..(1 << k) {
                // Spread v over the bits of mask.
                let mut val = 0u32;
                let mut vi = 0;
                for bit in 0..n {
                    if mask >> bit & 1 == 1 {
                        val |= (v >> vi & 1) << bit;
                        vi += 1;
                    }
                }
                if bad_list.iter().all(|&b| b & mask != val) && good_list.iter().any(|&g| g & mask == val) {
                    found.push((mask, val));
                }
            }
        }
        if !found.is_empty() {
            return found;
        }
    }
    // Unreachable when at least one good state exists (fixing every relevant
    // bit to a good state's values always works), but stay safe.
    let g = good_list[0];
    let mask = relevant.iter().fold(0u32, |m, &b| m | 1 << b);
    vec![(mask, g & mask)]
}
