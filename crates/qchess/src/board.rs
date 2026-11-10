//! A single classical board state and ordinary chess move rules.
//!
//! Quantum positions are sets of these; every legality question in the
//! quantum rules ("would this be legal in regular chess from that state?")
//! is answered here, often thousands of times per move the AI considers, so
//! the board keeps bitboards (one bit per square) next to its square array:
//! attack tests are a handful of mask operations, and legality is tested
//! without copying the board.

use crate::types::*;

/// What occupies a square in one classical state. `id` names the piece
/// (which pair of quantum halves it came from).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Occ {
    pub id: u8,
    pub color: Color,
    pub kind: Kind,
}

pub const KING_SIDE: usize = 0;
pub const QUEEN_SIDE: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveKind {
    Normal,
    DoublePush,
    EnPassant,
    Castle(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CMove {
    pub from: Sq,
    pub to: Sq,
    pub promo: Option<Kind>,
    pub kind: MoveKind,
}

/// One bit per square, bit `s` for square `s`.
pub type Bb = u64;

const fn bit(s: Sq) -> Bb {
    1 << s
}

#[derive(Clone, Debug)]
pub struct Board {
    /// The square array. Read it freely; change it only through
    /// [`place`](Board::place) and [`apply`](Board::apply), which keep the
    /// fields below in step.
    pub sq: [Option<Occ>; 64],
    pub turn: Color,
    pub ep: Option<Sq>,
    /// `castle[color][side]`
    pub castle: [[bool; 2]; 2],
    /// Where each king stands (indexed like `castle`); 64 if it is off the
    /// board.
    pub kings: [Sq; 2],
    /// Squares held by each color.
    pub by_color: [Bb; 2],
    /// Squares held by each kind of piece (both colors), by `Kind as usize`.
    pub by_kind: [Bb; 6],
    /// Identity of the placement: a hash of (square, piece id, kind) over
    /// every occupied square, kept up to date move by move. Two boards with
    /// the same pieces on the same squares have the same key.
    pub key: u128,
}

const KNIGHT: [(i8, i8); 8] = [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)];
const KING: [(i8, i8); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

const fn step_table(steps: &[(i8, i8)]) -> [Bb; 64] {
    let mut t = [0; 64];
    let mut s = 0;
    while s < 64 {
        let (f, r) = ((s % 8) as i8, (s / 8) as i8);
        let mut i = 0;
        while i < steps.len() {
            let (nf, nr) = (f + steps[i].0, r + steps[i].1);
            if nf >= 0 && nf < 8 && nr >= 0 && nr < 8 {
                t[s] |= 1 << (nr * 8 + nf);
            }
            i += 1;
        }
        s += 1;
    }
    t
}

const KNIGHT_ATT: [Bb; 64] = step_table(&KNIGHT);
const KING_ATT: [Bb; 64] = step_table(&KING);
/// Squares a pawn of each color on a square attacks (white first).
const PAWN_ATT: [[Bb; 64]; 2] = [step_table(&[(-1, 1), (1, 1)]), step_table(&[(-1, -1), (1, -1)])];

/// Ray directions. The first four run towards higher square numbers, so the
/// nearest blocker on them is the lowest set bit; the last four the reverse.
const DIRS: [(i8, i8); 8] = [(0, 1), (1, 0), (1, 1), (-1, 1), (0, -1), (-1, 0), (1, -1), (-1, -1)];
const N: usize = 0;
const E: usize = 1;
const NE: usize = 2;
const NW: usize = 3;
const S: usize = 4;
const W: usize = 5;
const SE: usize = 6;
const SW: usize = 7;

/// `RAYS[d][s]`: every square from `s` (exclusive) to the edge in direction `d`.
const RAYS: [[Bb; 64]; 8] = {
    let mut t = [[0; 64]; 8];
    let mut d = 0;
    while d < 8 {
        let mut s = 0;
        while s < 64 {
            let (mut f, mut r) = ((s % 8) as i8 + DIRS[d].0, (s / 8) as i8 + DIRS[d].1);
            while f >= 0 && f < 8 && r >= 0 && r < 8 {
                t[d][s] |= 1 << (r * 8 + f);
                f += DIRS[d].0;
                r += DIRS[d].1;
            }
            s += 1;
        }
        d += 1;
    }
    t
};

/// Squares a slider on `s` attacks along direction `d`, up to and including
/// the first occupied square.
#[inline]
fn ray(d: usize, s: Sq, occ: Bb) -> Bb {
    let r = RAYS[d][s as usize];
    let blockers = r & occ;
    if blockers == 0 {
        return r;
    }
    let first = if d < 4 { blockers.trailing_zeros() } else { 63 - blockers.leading_zeros() };
    r ^ RAYS[d][first as usize]
}

#[inline]
fn bishop_att(s: Sq, occ: Bb) -> Bb {
    ray(NE, s, occ) | ray(NW, s, occ) | ray(SE, s, occ) | ray(SW, s, occ)
}

#[inline]
fn rook_att(s: Sq, occ: Bb) -> Bb {
    ray(N, s, occ) | ray(E, s, occ) | ray(S, s, occ) | ray(W, s, occ)
}

/// Random odd multipliers per square, two independent sets (one per half
/// of the key).
const ZOB: [[u64; 64]; 2] = {
    let mut t = [[0; 64]; 2];
    let mut z: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut i = 0;
    while i < 128 {
        // splitmix64
        z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut x = z;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^= x >> 31;
        t[i / 64][i % 64] = x | 1;
        i += 1;
    }
    t
};

/// Key contribution of piece `o` standing on `s`: a random per-square
/// multiplier times the piece's (id, kind), in two independent 64-bit
/// halves, so an accidental clash among the at most 65,536 boards of a
/// position is astronomically unlikely.
#[inline]
fn zob(s: Sq, o: Occ) -> u128 {
    let v = ((o.id as u64) << 3 | o.kind as u64) + 1;
    let v = v.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (v << 40);
    (ZOB[0][s as usize].wrapping_mul(v) as u128) << 64 | ZOB[1][s as usize].wrapping_mul(v ^ 0xa076_1d64_78bd_642f) as u128
}

fn pawn_dir(c: Color) -> i8 {
    match c {
        Color::White => 1,
        Color::Black => -1,
    }
}

/// Iterate over the set squares of a bitboard.
fn squares(mut b: Bb) -> impl Iterator<Item = Sq> {
    std::iter::from_fn(move || {
        if b == 0 {
            return None;
        }
        let s = b.trailing_zeros() as Sq;
        b &= b - 1;
        Some(s)
    })
}

impl Board {
    pub fn empty(turn: Color) -> Self {
        Board {
            sq: [None; 64],
            turn,
            ep: None,
            castle: [[false; 2]; 2],
            kings: [64; 2],
            by_color: [0; 2],
            by_kind: [0; 6],
            key: 0,
        }
    }

    /// Put a piece on an empty square.
    pub fn place(&mut self, s: Sq, o: Occ) {
        debug_assert!(self.sq[s as usize].is_none());
        if o.kind == Kind::King {
            self.kings[o.color.idx()] = s;
        }
        self.by_color[o.color.idx()] |= bit(s);
        self.by_kind[o.kind as usize] |= bit(s);
        self.key = self.key.wrapping_add(zob(s, o));
        self.sq[s as usize] = Some(o);
    }

    fn remove(&mut self, s: Sq) -> Option<Occ> {
        let o = self.sq[s as usize].take()?;
        if o.kind == Kind::King {
            self.kings[o.color.idx()] = 64;
        }
        self.by_color[o.color.idx()] &= !bit(s);
        self.by_kind[o.kind as usize] &= !bit(s);
        self.key = self.key.wrapping_sub(zob(s, o));
        Some(o)
    }

    fn occupied(&self) -> Bb {
        self.by_color[0] | self.by_color[1]
    }

    fn is(&self, s: Sq, c: Color, k: Kind) -> bool {
        matches!(self.sq[s as usize], Some(o) if o.color == c && o.kind == k)
    }

    pub fn king_sq(&self, c: Color) -> Option<Sq> {
        let s = self.kings[c.idx()];
        (s < 64).then_some(s)
    }

    /// Is square `s` attacked by any piece of color `by`?
    pub fn attacked(&self, s: Sq, by: Color) -> bool {
        self.attacked_with(s, by, self.occupied(), self.by_color[by.idx()])
    }

    /// [`attacked`](Self::attacked) on a board whose occupied squares are
    /// `occ` and whose attackers are the pieces of `by` on `them` (the rest of
    /// the board as it stands), for testing a move without making it.
    #[inline]
    fn attacked_with(&self, s: Sq, by: Color, occ: Bb, them: Bb) -> bool {
        let k = &self.by_kind;
        let s = s as usize;
        PAWN_ATT[by.other().idx()][s] & k[Kind::Pawn as usize] & them != 0
            || KNIGHT_ATT[s] & k[Kind::Knight as usize] & them != 0
            || KING_ATT[s] & k[Kind::King as usize] & them != 0
            || bishop_att(s as Sq, occ) & (k[Kind::Bishop as usize] | k[Kind::Queen as usize]) & them != 0
            || rook_att(s as Sq, occ) & (k[Kind::Rook as usize] | k[Kind::Queen as usize]) & them != 0
    }

    pub fn in_check(&self, c: Color) -> bool {
        self.king_sq(c).is_some_and(|k| self.attacked(k, c.other()))
    }

    /// Squares a non-pawn piece of kind `k` on `from` attacks.
    fn piece_att(&self, k: Kind, from: Sq) -> Bb {
        match k {
            Kind::Knight => KNIGHT_ATT[from as usize],
            Kind::King => KING_ATT[from as usize],
            Kind::Bishop => bishop_att(from, self.occupied()),
            Kind::Rook => rook_att(from, self.occupied()),
            Kind::Queen => bishop_att(from, self.occupied()) | rook_att(from, self.occupied()),
            Kind::Pawn => 0,
        }
    }

    /// Pseudo-legal moves (own king safety not checked, except castling
    /// rules about check) for the piece of the side to move on `from`, handed
    /// one by one to `out` so callers needn't allocate.
    pub fn each_pseudo(&self, from: Sq, out: &mut impl FnMut(CMove)) {
        let Some(o) = self.sq[from as usize] else {
            return;
        };
        if o.color != self.turn {
            return;
        }
        let c = o.color;
        let mut push = |to: Sq, kind: MoveKind, promo: Option<Kind>| out(CMove { from, to, promo, kind });
        if o.kind == Kind::Pawn {
            let dir = pawn_dir(c);
            let start = if c == Color::White { 1 } else { 6 };
            let last = if c == Color::White { 7 } else { 0 };
            let mut pawn_to = |to: Sq, kind: MoveKind| {
                if rank_of(to) == last {
                    for k in [Kind::Queen, Kind::Rook, Kind::Bishop, Kind::Knight] {
                        push(to, kind, Some(k));
                    }
                } else {
                    push(to, kind, None);
                }
            };
            if let Some(t) = offset(from, 0, dir)
                && self.sq[t as usize].is_none()
            {
                pawn_to(t, MoveKind::Normal);
                if rank_of(from) == start
                    && let Some(t2) = offset(from, 0, 2 * dir)
                    && self.sq[t2 as usize].is_none()
                {
                    pawn_to(t2, MoveKind::DoublePush);
                }
            }
            for t in squares(PAWN_ATT[c.idx()][from as usize]) {
                match self.sq[t as usize] {
                    Some(x) if x.color != c => pawn_to(t, MoveKind::Normal),
                    None if self.ep == Some(t) => pawn_to(t, MoveKind::EnPassant),
                    _ => {}
                }
            }
            return;
        }
        for t in squares(self.piece_att(o.kind, from) & !self.by_color[c.idx()]) {
            push(t, MoveKind::Normal, None);
        }
        if o.kind == Kind::King {
            for side in [KING_SIDE, QUEEN_SIDE] {
                if let Some(m) = self.castle_move(from, side) {
                    out(m);
                }
            }
        }
    }

    /// Castling move for the king on `from`, if castling is allowed in this
    /// state (rights, empty squares, not castling out of/through check).
    pub fn castle_move(&self, from: Sq, side: usize) -> Option<CMove> {
        let c = self.turn;
        let home = c.home_rank();
        if from != sq(4, home) || !self.is(from, c, Kind::King) || !self.castle[c.idx()][side] {
            return None;
        }
        let (rook_file, empty, safe, to): (u8, &[u8], &[u8], u8) =
            if side == KING_SIDE { (7, &[5, 6], &[5, 6], 6) } else { (0, &[1, 2, 3], &[3, 2], 2) };
        if !self.is(sq(rook_file, home), c, Kind::Rook) {
            return None;
        }
        if empty.iter().any(|&f| self.sq[sq(f, home) as usize].is_some()) {
            return None;
        }
        if self.attacked(from, c.other()) || safe.iter().any(|&f| self.attacked(sq(f, home), c.other())) {
            return None;
        }
        Some(CMove { from, to: sq(to, home), promo: None, kind: MoveKind::Castle(side) })
    }

    pub fn apply(&self, m: CMove) -> Board {
        let mut b = self.clone();
        let o = b.remove(m.from).expect("apply: empty from-square");
        match m.kind {
            MoveKind::EnPassant => {
                b.remove(sq(file_of(m.to), rank_of(m.from)));
            }
            MoveKind::Castle(side) => {
                let home = rank_of(m.from);
                let (rf, rt) = if side == KING_SIDE { (7, 5) } else { (0, 3) };
                if let Some(r) = b.remove(sq(rf, home)) {
                    b.place(sq(rt, home), r);
                }
            }
            _ => {}
        }
        b.remove(m.to);
        b.place(m.to, Occ { kind: m.promo.unwrap_or(o.kind), ..o });
        b.ep = (m.kind == MoveKind::DoublePush).then(|| (m.from + m.to) / 2);
        b.turn = self.turn.other();
        b
    }

    /// The [`key`](Board::key) of `self.apply(m)`, without making the move.
    pub fn key_after(&self, m: CMove) -> u128 {
        let o = self.sq[m.from as usize].expect("key_after: empty from-square");
        let mut k = self.key.wrapping_sub(zob(m.from, o));
        if let Some(c) = self.sq[m.to as usize] {
            k = k.wrapping_sub(zob(m.to, c));
        }
        match m.kind {
            MoveKind::EnPassant => {
                let cap = sq(file_of(m.to), rank_of(m.from));
                if let Some(c) = self.sq[cap as usize] {
                    k = k.wrapping_sub(zob(cap, c));
                }
            }
            MoveKind::Castle(side) => {
                let home = rank_of(m.from);
                let (rf, rt) = if side == KING_SIDE { (7, 5) } else { (0, 3) };
                if let Some(r) = self.sq[sq(rf, home) as usize] {
                    k = k.wrapping_sub(zob(sq(rf, home), r)).wrapping_add(zob(sq(rt, home), r));
                }
            }
            _ => {}
        }
        k.wrapping_add(zob(m.to, Occ { kind: m.promo.unwrap_or(o.kind), ..o }))
    }

    /// Fully legal: the mover's king is not left attacked. Tested on masks,
    /// without making the move.
    pub fn is_legal(&self, m: CMove) -> bool {
        if let MoveKind::Castle(_) = m.kind {
            return !self.apply(m).in_check(self.turn);
        }
        let us = self.turn;
        let (from, to) = (bit(m.from), bit(m.to));
        let mut occ = (self.occupied() & !from) | to;
        let mut them = self.by_color[us.other().idx()] & !to;
        if m.kind == MoveKind::EnPassant {
            let cap = bit(sq(file_of(m.to), rank_of(m.from)));
            occ &= !cap;
            them &= !cap;
        }
        let king = if self.kings[us.idx()] == m.from { m.to } else { self.kings[us.idx()] };
        king >= 64 || !self.attacked_with(king, us.other(), occ, them)
    }

    pub fn has_legal_move(&self) -> bool {
        for s in squares(self.by_color[self.turn.idx()]) {
            let mut found = false;
            self.each_pseudo(s, &mut |m| found = found || self.is_legal(m));
            if found {
                return true;
            }
        }
        false
    }

    /// Find the legal non-castling move `from -> to`. A missing promotion
    /// piece defaults to a queen.
    pub fn find_move(&self, from: Sq, to: Sq, promo: Option<Kind>) -> Option<CMove> {
        let o = self.sq[from as usize]?;
        if o.color != self.turn {
            return None;
        }
        if o.kind != Kind::Pawn {
            if promo.is_some() || self.piece_att(o.kind, from) & !self.by_color[o.color.idx()] & bit(to) == 0 {
                return None;
            }
            let m = CMove { from, to, promo: None, kind: MoveKind::Normal };
            return self.is_legal(m).then_some(m);
        }
        let mut found = None;
        self.each_pseudo(from, &mut |m| {
            if found.is_none()
                && m.to == to
                && (m.promo == promo || (promo.is_none() && m.promo == Some(Kind::Queen)))
                && self.is_legal(m)
            {
                found = Some(m);
            }
        });
        found
    }

    pub fn find_castle(&self, from: Sq, side: usize) -> Option<CMove> {
        self.castle_move(from, side).filter(|&m| self.is_legal(m))
    }

    pub fn placement_fen(&self) -> String {
        let mut out = String::new();
        for r in (0..8).rev() {
            let mut empty = 0;
            for f in 0..8 {
                match self.sq[sq(f, r) as usize] {
                    None => empty += 1,
                    Some(o) => {
                        if empty > 0 {
                            out.push_str(&empty.to_string());
                            empty = 0;
                        }
                        out.push(o.kind.fen_char(o.color));
                    }
                }
            }
            if empty > 0 {
                out.push_str(&empty.to_string());
            }
            if r > 0 {
                out.push('/');
            }
        }
        out
    }
}

/// The bitboard board must answer every question exactly as the plain
/// square-scanning board it replaced (kept as `board_ref`) does.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::board_ref;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn fresh_key(b: &Board) -> u128 {
        (0..64u8).filter_map(|s| b.sq[s as usize].map(|o| zob(s, o))).fold(0u128, |a, z| a.wrapping_add(z))
    }

    fn random_pair(r: &mut Rng) -> (Board, board_ref::Board) {
        let turn = if r.below(2) == 0 { Color::White } else { Color::Black };
        let mut b = Board::empty(turn);
        let mut kinds = vec![(Color::White, Kind::King), (Color::Black, Kind::King)];
        if r.below(10) == 0 {
            kinds.pop(); // a board without a black king still answers questions
        }
        for c in [Color::White, Color::Black] {
            kinds.extend([(c, Kind::Rook), (c, Kind::Rook)]);
        }
        for _ in 0..r.below(14) {
            let c = if r.below(2) == 0 { Color::White } else { Color::Black };
            let k = [Kind::Pawn, Kind::Pawn, Kind::Knight, Kind::Bishop, Kind::Rook, Kind::Queen][r.below(6) as usize];
            kinds.push((c, k));
        }
        // Castling setups and en passant chances come up often enough when
        // kings, rooks and pawns favour their usual squares.
        for (id, (c, k)) in kinds.into_iter().enumerate() {
            for _ in 0..20 {
                let s = match (k, r.below(3)) {
                    (Kind::King, 0 | 1) => sq(4, c.home_rank()),
                    (Kind::Rook, 0 | 1) => sq(if r.below(2) == 0 { 0 } else { 7 }, c.home_rank()),
                    (Kind::Pawn, 0) => sq(r.below(8) as u8, if c == Color::White { 4 } else { 3 }),
                    _ => r.below(64) as Sq,
                };
                if k == Kind::Pawn && (rank_of(s) == 0 || rank_of(s) == 7) {
                    continue;
                }
                if b.sq[s as usize].is_none() {
                    b.place(s, Occ { id: id as u8, color: c, kind: k });
                    break;
                }
            }
        }
        for c in [Color::White, Color::Black] {
            for side in [KING_SIDE, QUEEN_SIDE] {
                b.castle[c.idx()][side] = r.below(2) == 0;
            }
        }
        if r.below(2) == 0 {
            // A pawn that just pushed two squares, with a pawn of the side to
            // move beside it, and the en passant target behind it.
            let them = turn.other();
            let rank = if them == Color::White { 3 } else { 4 };
            let behind = if them == Color::White { 2 } else { 5 };
            let f = r.below(8) as u8;
            let side = if f == 0 || (f < 7 && r.below(2) == 0) { f + 1 } else { f - 1 };
            let id = 40;
            if b.sq[sq(f, rank) as usize].is_none() && b.sq[sq(f, behind) as usize].is_none() {
                b.place(sq(f, rank), Occ { id, color: them, kind: Kind::Pawn });
                b.ep = Some(sq(f, behind));
                if b.sq[sq(side, rank) as usize].is_none() {
                    b.place(sq(side, rank), Occ { id: id + 1, color: turn, kind: Kind::Pawn });
                }
            }
        }
        let mut old = board_ref::Board::empty(turn);
        old.sq = b.sq;
        old.ep = b.ep;
        old.castle = b.castle;
        old.kings = b.kings;
        (b, old)
    }

    fn same(new: &Board, old: &board_ref::Board) {
        assert_eq!(new.sq, old.sq);
        assert_eq!((new.turn, new.ep, new.castle), (old.turn, old.ep, old.castle));
        for c in [Color::White, Color::Black] {
            assert_eq!(new.king_sq(c), (0..64u8).find(|&s| old.sq[s as usize].is_some_and(|o| o.color == c && o.kind == Kind::King)));
        }
        assert_eq!(new.key, fresh_key(new));
        let occ: Bb = (0..64u8).filter(|&s| new.sq[s as usize].is_some()).fold(0, |a, s| a | bit(s));
        assert_eq!(new.occupied(), occ);
    }

    fn conv(m: CMove) -> board_ref::CMove {
        board_ref::CMove {
            from: m.from,
            to: m.to,
            promo: m.promo,
            kind: match m.kind {
                MoveKind::Normal => board_ref::MoveKind::Normal,
                MoveKind::DoublePush => board_ref::MoveKind::DoublePush,
                MoveKind::EnPassant => board_ref::MoveKind::EnPassant,
                MoveKind::Castle(s) => board_ref::MoveKind::Castle(s),
            },
        }
    }

    #[test]
    fn bitboards_match_the_reference_board() {
        let mut r = Rng(0x1234_5678_9abc_def1);
        let (mut castles, mut passants, mut promos, mut illegal) = (0, 0, 0, 0);
        for _ in 0..6000 {
            let (b, old) = random_pair(&mut r);
            same(&b, &old);
            for s in 0..64u8 {
                for c in [Color::White, Color::Black] {
                    assert_eq!(b.attacked(s, c), old.attacked(s, c), "attacked {s} {c:?}\n{}", b.placement_fen());
                }
            }
            for c in [Color::White, Color::Black] {
                assert_eq!(b.in_check(c), old.in_check(c));
            }
            assert_eq!(b.has_legal_move(), old.has_legal_move(), "{}", b.placement_fen());
            for from in 0..64u8 {
                let mut mine = Vec::new();
                b.each_pseudo(from, &mut |m| mine.push(m));
                let mut theirs = Vec::new();
                old.each_pseudo(from, &mut |m| theirs.push(m));
                let mut a: Vec<_> = mine.iter().map(|&m| conv(m)).map(|m| (m.from, m.to, m.promo, format!("{:?}", m.kind))).collect();
                let mut z: Vec<_> = theirs.iter().map(|m| (m.from, m.to, m.promo, format!("{:?}", m.kind))).collect();
                a.sort();
                z.sort();
                assert_eq!(a, z, "moves from {from}\n{}", b.placement_fen());
                for &m in &mine {
                    castles += matches!(m.kind, MoveKind::Castle(_)) as usize;
                    passants += (m.kind == MoveKind::EnPassant) as usize;
                    promos += m.promo.is_some() as usize;
                    illegal += !b.is_legal(m) as usize;
                    assert_eq!(b.is_legal(m), old.is_legal(conv(m)), "{m:?}\n{}", b.placement_fen());
                    let after = b.apply(m);
                    same(&after, &old.apply(conv(m)));
                    assert_eq!(b.key_after(m), after.key);
                }
                for to in 0..64u8 {
                    for promo in [None, Some(Kind::Queen), Some(Kind::Knight)] {
                        let x = b.find_move(from, to, promo).map(conv);
                        assert_eq!(x, old.find_move(from, to, promo), "find {from} {to} {promo:?}\n{}", b.placement_fen());
                    }
                }
                for side in [KING_SIDE, QUEEN_SIDE] {
                    assert_eq!(b.find_castle(from, side).map(conv), old.find_castle(from, side));
                }
            }
        }
        // The random boards must reach the special cases.
        assert!(castles > 100 && passants > 100 && promos > 100 && illegal > 1000, "{castles} {passants} {promos} {illegal}");
    }
}
