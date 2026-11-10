//! Solves king, bishop and knight against a lone king that may split.
//!
//! `cargo run --release -p qchess --example qkbn -- <mode>`:
//! * `check [walks]`: compares this file's move rules with the real engine
//!   on random games (every legal move, every resulting set of boards).
//! * `solve`: builds the table for both defenders (classical, and one that
//!   may split its king) and prints how they compare.
//! * `play [FEN]`: plays the fastest win against the longest defence, every
//!   move through the engine, and checks the engine agrees it is mate.
//! * `nopass`: the splitting defender again, but without the move that
//!   swaps the two halves (which changes nothing, so it passes the turn).
//! * `verify [games]`: the same from random won positions, black moving at
//!   random; every game must end in mate within the promised moves.
//!
//! White plays ordinary (unsplit) moves. Black's king is on one square or
//! split across two (a piece has two halves, so never more). A win found
//! here is a real forced win against every black move the engine allows;
//! with white splits allowed it could only be faster.
//!
//! Rules, as the engine applies them (and `check` confirms):
//! * Black, king on `a`: move to `x`; split to `x`/`y`; or move one half
//!   to `x` and leave the other on `a` (only if `a` is not in check).
//! * Black, halves on `a` and `b` (boards A and B): move from `a` (the
//!   king collapses to `a`), or split from `a`; move the `a` half and keep
//!   the `b` half (only if B is not in check); move both halves at once;
//!   and the same with `a` and `b` swapped. Each move must be legal in the
//!   board it is played in. A capture always collapses the king first, so
//!   splits never capture.
//! * White: a move is played in every board where it is legal; boards
//!   where it isn't drop out. So walking the king next to one half
//!   collapses black's king onto the other.
//! * Mate: every board is checkmate.
use qchess::{Game, Status, ai};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering::Relaxed};

// ---------- geometry ----------

const fn steps(d: &[(i32, i32)]) -> [u64; 64] {
    let mut t = [0u64; 64];
    let mut s = 0;
    while s < 64 {
        let (f, r) = ((s % 8) as i32, (s / 8) as i32);
        let mut i = 0;
        while i < d.len() {
            let (nf, nr) = (f + d[i].0, r + d[i].1);
            if nf >= 0 && nf < 8 && nr >= 0 && nr < 8 {
                t[s] |= 1 << (nr * 8 + nf);
            }
            i += 1;
        }
        s += 1;
    }
    t
}
const KING: [u64; 64] = steps(&[(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)]);
const KNIGHT: [u64; 64] = steps(&[(1, 2), (2, 1), (-1, 2), (-2, 1), (1, -2), (2, -1), (-1, -2), (-2, -1)]);

fn bishop(s: u8, occ: u64) -> u64 {
    let (f, r) = ((s % 8) as i32, (s / 8) as i32);
    let mut a = 0;
    for (df, dr) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
        let (mut nf, mut nr) = (f + df, r + dr);
        while (0..8).contains(&nf) && (0..8).contains(&nr) {
            let b = 1u64 << (nr * 8 + nf);
            a |= b;
            if occ & b != 0 {
                break;
            }
            nf += df;
            nr += dr;
        }
    }
    a
}

fn bits(mut b: u64) -> impl Iterator<Item = u8> {
    std::iter::from_fn(move || {
        (b != 0).then(|| {
            let s = b.trailing_zeros() as u8;
            b &= b - 1;
            s
        })
    })
}
const fn bit(s: u8) -> u64 {
    1 << s
}

// ---------- positions ----------

/// White king, bishop, knight; black king halves (`k[0] == k[1]` when it
/// is on one square, otherwise `k[0] < k[1]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct St {
    wk: u8,
    wb: u8,
    wn: u8,
    k: [u8; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Out {
    /// Black took the bishop or knight: no longer a win.
    Capture,
    To(St),
}

impl St {
    fn with_k(self, a: u8, b: u8) -> St {
        St { k: [a.min(b), a.max(b)], ..self }
    }
    fn split(&self) -> bool {
        self.k[0] != self.k[1]
    }
    fn halves(&self) -> &[u8] {
        if self.split() { &self.k } else { &self.k[..1] }
    }
    fn white(&self) -> u64 {
        bit(self.wk) | bit(self.wb) | bit(self.wn)
    }
    /// Is `x` attacked by white when black's king stands there (or moves
    /// there, capturing whatever is on `x`)?
    fn attacked(&self, x: u8) -> bool {
        KING[self.wk as usize] & bit(x) != 0
            || (x != self.wn && KNIGHT[self.wn as usize] & bit(x) != 0)
            || (x != self.wb && bishop(self.wb, self.white()) & bit(x) != 0)
    }
    /// Legal king moves in the board where the king is on `a`.
    fn dests(&self, a: u8) -> u64 {
        bits(KING[a as usize] & !bit(self.wk)).filter(|&x| !self.attacked(x)).fold(0, |m, x| m | bit(x))
    }
    fn valid(&self, white_to_move: bool) -> bool {
        let w = [self.wk, self.wb, self.wn];
        if w[0] == w[1] || w[0] == w[2] || w[1] == w[2] {
            return false;
        }
        self.halves().iter().all(|&h| {
            !w.contains(&h) && KING[self.wk as usize] & bit(h) == 0 && !(white_to_move && self.attacked(h))
        })
    }
    /// 1 = checkmate, 2 = stalemate, 0 = neither.
    fn terminal(&self) -> u8 {
        if self.halves().iter().any(|&h| self.dests(h) != 0) {
            0
        } else if self.halves().iter().all(|&h| self.attacked(h)) {
            1
        } else {
            2
        }
    }

    /// Every distinct result of a black move.
    fn black_moves(&self, quantum: bool, f: &mut impl FnMut(Out) -> bool) {
        let pieces = bit(self.wb) | bit(self.wn);
        let hs = self.halves();
        let mut q = [0u64; 2];
        for (i, &h) in hs.iter().enumerate() {
            let d = self.dests(h);
            if d & pieces != 0 && !f(Out::Capture) {
                return;
            }
            q[i] = d & !pieces;
        }
        // Small sets of results, deduplicated in a 2080-bit set.
        let mut seen = [0u64; 33];
        let mut emit = |a: u8, b: u8| {
            let i = kpair(a.min(b), a.max(b));
            let (w, m) = (i / 64, 1u64 << (i % 64));
            if seen[w] & m != 0 {
                return true;
            }
            seen[w] |= m;
            f(Out::To(self.with_k(a, b)))
        };
        for &d in &q[..hs.len()] {
            for x in bits(d) {
                if !emit(x, x) {
                    return;
                }
            }
        }
        if !quantum {
            return;
        }
        for i in 0..hs.len() {
            // Split (only a king on one square: a piece has two halves).
            if hs.len() == 1 {
                for x in bits(q[0]) {
                    for y in bits(q[0] & !(bit(x + 1) - 1)) {
                        if !emit(x, y) {
                            return;
                        }
                    }
                }
            }
            // Move this half, keep the other (on its own square when the
            // king isn't split).
            let other = hs[hs.len() - 1 - i];
            if !self.attacked(other) {
                for x in bits(q[i]) {
                    if !emit(x, other) {
                        return;
                    }
                }
            }
        }
        if hs.len() == 2 {
            for x in bits(q[0]) {
                for y in bits(q[1]) {
                    if !emit(x, y) {
                        return;
                    }
                }
            }
        }
    }

    /// Every result of a white (unsplit) move.
    fn white_moves(&self, f: &mut impl FnMut(St) -> bool) {
        let own = self.white();
        let hs = self.halves();
        let mut play = |to: St, legal: &dyn Fn(u8) -> bool| {
            let ok: Vec<u8> = hs.iter().copied().filter(|&h| legal(h)).collect();
            match ok[..] {
                [] => true,
                [a] => f(to.with_k(a, a)),
                [a, b] => f(to.with_k(a, b)),
                _ => unreachable!(),
            }
        };
        for t in bits(KING[self.wk as usize] & !own) {
            if !play(St { wk: t, ..*self }, &|h| KING[h as usize] & bit(t) == 0 && h != t) {
                return;
            }
        }
        for t in bits(KNIGHT[self.wn as usize] & !own) {
            if !play(St { wn: t, ..*self }, &|h| h != t) {
                return;
            }
        }
        for t in bits(bishop(self.wb, own) & !own) {
            if !play(St { wb: t, ..*self }, &|h| h != t && bishop(self.wb, own | bit(h)) & bit(t) != 0) {
                return;
            }
        }
    }
}

// ---------- indexing (8-fold symmetry on the white king) ----------

fn kpair(a: u8, b: u8) -> usize {
    let (a, b) = (a as usize, b as usize);
    if a == b { a } else { 64 + b * (b - 1) / 2 + a }
}
fn unpair(i: usize) -> (u8, u8) {
    if i < 64 {
        return (i as u8, i as u8);
    }
    let i = i - 64;
    let mut b = 1;
    while (b + 1) * b / 2 <= i {
        b += 1;
    }
    ((i - b * (b - 1) / 2) as u8, b as u8)
}
const PAIRS: usize = 64 + 64 * 63 / 2;
/// a1-d1-d4 triangle.
const TRI: [u8; 10] = [0, 1, 2, 3, 9, 10, 11, 18, 19, 27];
const N: usize = 10 * 64 * 64 * PAIRS;

fn xf(t: u8, s: u8) -> u8 {
    let (mut f, mut r) = (s % 8, s / 8);
    if t & 1 != 0 {
        f = 7 - f;
    }
    if t & 2 != 0 {
        r = 7 - r;
    }
    if t & 4 != 0 {
        std::mem::swap(&mut f, &mut r);
    }
    r * 8 + f
}

struct Ix {
    /// For each square, the transforms taking it into the triangle.
    to_tri: [Vec<u8>; 64],
    tri_of: [u8; 64],
    xf: [[u8; 64]; 8],
}

impl Ix {
    fn new() -> Ix {
        let mut tri_of = [255u8; 64];
        for (i, &s) in TRI.iter().enumerate() {
            tri_of[s as usize] = i as u8;
        }
        let xft = std::array::from_fn(|t| std::array::from_fn(|s| xf(t as u8, s as u8)));
        let to_tri = std::array::from_fn(|s| (0..8).filter(|&t| tri_of[xf(t, s as u8) as usize] != 255).collect());
        Ix { to_tri, tri_of, xf: xft }
    }
    fn index(&self, s: &St) -> usize {
        let mut best = usize::MAX;
        for &t in &self.to_tri[s.wk as usize] {
            let m = &self.xf[t as usize];
            let (a, b) = (m[s.k[0] as usize], m[s.k[1] as usize]);
            let i = ((self.tri_of[m[s.wk as usize] as usize] as usize * 64 + m[s.wb as usize] as usize) * 64
                + m[s.wn as usize] as usize)
                * PAIRS
                + kpair(a.min(b), a.max(b));
            best = best.min(i);
        }
        best
    }
    fn state(&self, i: usize) -> St {
        let (a, b) = unpair(i % PAIRS);
        let i = i / PAIRS;
        St { wk: TRI[i / 4096], wb: (i / 64 % 64) as u8, wn: (i % 64) as u8, k: [a, b] }
    }
}

// ---------- the table ----------

/// `w[i]`: white to move wins in this many moves (0 = not a win, 255 =
/// not a position). `l[i]`: black to move is mated after `l[i] - 1` more
/// white moves (0 = not lost, 255 = not a position).
struct Table {
    w: Vec<AtomicU8>,
    l: Vec<AtomicU8>,
    quantum: bool,
}

fn par(f: impl Fn(usize) -> usize + Sync) -> usize {
    let next = AtomicUsize::new(0);
    let total = AtomicUsize::new(0);
    const CHUNK: usize = 1 << 16;
    std::thread::scope(|sc| {
        for _ in 0..std::thread::available_parallelism().map_or(8, |n| n.get()) {
            sc.spawn(|| {
                let mut n = 0;
                loop {
                    let start = next.fetch_add(CHUNK, Relaxed);
                    if start >= N {
                        break;
                    }
                    for i in start..(start + CHUNK).min(N) {
                        n += f(i);
                    }
                }
                total.fetch_add(n, Relaxed);
            });
        }
    });
    total.into_inner()
}

impl Table {
    /// `pass`: may black swap its halves, which changes nothing?
    fn solve(ix: &Ix, quantum: bool, pass: bool) -> Table {
        let t0 = std::time::Instant::now();
        let mk = || (0..N).map(|_| AtomicU8::new(0)).collect::<Vec<_>>();
        let tb = Table { w: mk(), l: mk(), quantum };
        // Pass 0: mark non-positions (including non-canonical copies) and mates.
        let mates = par(|i| {
            let s = ix.state(i);
            let canon = ix.index(&s) == i;
            if !canon || !s.valid(true) || (!quantum && s.split()) {
                tb.w[i].store(255, Relaxed);
            }
            if !canon || !s.valid(false) || (!quantum && s.split()) {
                tb.l[i].store(255, Relaxed);
                return 0;
            }
            if s.terminal() == 1 {
                tb.l[i].store(1, Relaxed);
                return 1;
            }
            0
        });
        eprintln!("  {mates} mates ({:.1?})", t0.elapsed());
        for n in 1u8..250 {
            let won = par(|i| {
                if tb.w[i].load(Relaxed) != 0 {
                    return 0;
                }
                let s = ix.state(i);
                let mut win = false;
                s.white_moves(&mut |t| {
                    let l = tb.l[ix.index(&t)].load(Relaxed);
                    win = l != 0 && l <= n;
                    !win
                });
                if win {
                    tb.w[i].store(n, Relaxed);
                }
                win as usize
            });
            let lost = par(|i| {
                if tb.l[i].load(Relaxed) != 0 {
                    return 0;
                }
                let s = ix.state(i);
                let (mut all, mut worst, mut any) = (true, 0u8, false);
                s.black_moves(quantum, &mut |o| {
                    if !pass && o == Out::To(s) {
                        return true;
                    }
                    any = true;
                    match o {
                        Out::Capture => all = false,
                        Out::To(t) => {
                            let w = tb.w[ix.index(&t)].load(Relaxed);
                            debug_assert!(w != 255, "{s:?} -> {t:?}");
                            if w == 0 || w == 255 {
                                all = false;
                            }
                            worst = worst.max(w);
                        }
                    }
                    all
                });
                if all && any {
                    tb.l[i].store(worst + 1, Relaxed);
                }
                (all && any) as usize
            });
            eprintln!("  mate in {n}: {won} white-to-move wins, {lost} newly lost black positions ({:.1?})", t0.elapsed());
            if won == 0 && lost == 0 {
                break;
            }
        }
        tb
    }
    fn w(&self, ix: &Ix, s: &St) -> u8 {
        self.w[ix.index(s)].load(Relaxed)
    }
    fn l(&self, ix: &Ix, s: &St) -> u8 {
        self.l[ix.index(s)].load(Relaxed)
    }
}

// ---------- the engine side ----------

fn parse_board(fen: &str) -> [Option<u8>; 4] {
    // K, B, N, k
    let mut out = [None; 4];
    for (ri, row) in fen.split('/').enumerate() {
        let mut f = 0u8;
        for c in row.chars() {
            if let Some(d) = c.to_digit(10) {
                f += d as u8;
                continue;
            }
            let sq = (7 - ri as u8) * 8 + f;
            let slot = match c {
                'K' => 0,
                'B' => 1,
                'N' => 2,
                'k' => 3,
                _ => panic!("unexpected piece {c} in {fen}"),
            };
            out[slot] = Some(sq);
            f += 1;
        }
    }
    out
}

/// The engine position as a table position (`Capture` if a white piece
/// was taken in any board).
fn from_game(g: &Game) -> Out {
    let boards: Vec<_> = g.states_fen().iter().map(|f| parse_board(f)).collect();
    if boards.iter().any(|b| b[1].is_none() || b[2].is_none()) {
        return Out::Capture;
    }
    let w = boards[0];
    assert!(boards.iter().all(|b| b[..3] == w[..3]), "white pieces differ between boards");
    let ks: BTreeSet<u8> = boards.iter().map(|b| b[3].unwrap()).collect();
    assert!(ks.len() <= 2 && boards.len() == ks.len(), "unexpected boards {:?}", g.states_fen());
    let ks: Vec<u8> = ks.into_iter().collect();
    Out::To(St { wk: w[0].unwrap(), wb: w[1].unwrap(), wn: w[2].unwrap(), k: [ks[0], *ks.last().unwrap()] })
}

fn fen_of(s: &St, white: bool) -> String {
    assert!(!s.split());
    let mut b = [['1'; 8]; 8];
    for (sq, c) in [(s.wk, 'K'), (s.wb, 'B'), (s.wn, 'N'), (s.k[0], 'k')] {
        b[7 - sq as usize / 8][sq as usize % 8] = c;
    }
    let rows: Vec<String> = b
        .iter()
        .map(|r| {
            let (mut out, mut n) = (String::new(), 0);
            for &c in r {
                if c == '1' {
                    n += 1;
                } else {
                    if n > 0 {
                        out += &n.to_string();
                        n = 0;
                    }
                    out.push(c);
                }
            }
            if n > 0 {
                out += &n.to_string();
            }
            out
        })
        .collect();
    format!("{} {} - - 0 1", rows.join("/"), if white { 'w' } else { 'b' })
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn random_start(rng: &mut Rng, white: bool) -> St {
    loop {
        let s = St { wk: rng.below(64) as u8, wb: rng.below(64) as u8, wn: rng.below(64) as u8, k: [rng.below(64) as u8; 2] };
        if s.valid(white) && s.valid(true) {
            return s;
        }
    }
}

/// Compare this file's rules with the engine along random games.
fn check(walks: usize) {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut positions, mut split_positions, mut moves) = (0, 0, 0);
    let (mut mates, mut split_mates, mut stalemates, mut collapses) = (0, 0, 0, 0);
    for w in 0..walks {
        let white = w % 2 == 0;
        let s0 = random_start(&mut rng, white);
        let mut g = Game::from_fen(&fen_of(&s0, white)).unwrap();
        for _ply in 0..60 {
            let Out::To(s) = from_game(&g) else { break };
            positions += 1;
            split_positions += s.split() as usize;
            let white = g.to_move() == qchess::Color::White;
            let kids = ai::children(&g, !white);
            let engine: BTreeSet<Out> = kids.iter().map(|c| from_game(&c.game)).collect();
            let mut mine = BTreeSet::new();
            if white {
                s.white_moves(&mut |t| {
                    mine.insert(Out::To(t));
                    true
                });
            } else {
                s.black_moves(true, &mut |o| {
                    mine.insert(o);
                    true
                });
            }
            moves += kids.len();
            if white && s.split() {
                collapses += mine.iter().filter(|o| matches!(o, Out::To(t) if !t.split())).count();
            }
            if engine != mine {
                println!("MISMATCH at {s:?} ({} to move), boards {:?}", if white { "white" } else { "black" }, g.states_fen());
                for o in engine.difference(&mine) {
                    let n: Vec<_> = kids.iter().filter(|c| from_game(&c.game) == *o).map(|c| c.notation.clone()).collect();
                    println!("  engine only: {o:?} via {n:?}");
                }
                for o in mine.difference(&engine) {
                    println!("  table only: {o:?}");
                }
                std::process::exit(1);
            }
            let status = g.status();
            let want = match if white { 0 } else { s.terminal() } {
                1 => Status::Checkmate { winner: qchess::Color::White },
                2 => Status::Stalemate,
                _ => Status::Ongoing,
            };
            assert_eq!(status, want, "status at {s:?}");
            match status {
                Status::Checkmate { .. } => {
                    mates += 1;
                    split_mates += s.split() as usize;
                }
                Status::Stalemate => stalemates += 1,
                _ => {}
            }
            if kids.is_empty() {
                break;
            }
            // White heads for the king half the time, so mates and
            // collapses come up.
            let pick = if white && rng.below(2) == 0 {
                let mut dist = |c: &ai::Child| match from_game(&c.game) {
                    Out::To(t) => t.halves().iter().map(|&h| (h % 8).abs_diff(t.wk % 8).max((h / 8).abs_diff(t.wk / 8))).sum::<u8>() as usize * 64 + rng.below(64),
                    Out::Capture => usize::MAX,
                };
                kids.iter().min_by_key(|c| dist(c)).unwrap()
            } else {
                &kids[rng.below(kids.len())]
            };
            g = pick.game.clone();
        }
    }
    println!("rules agree with the engine: {walks} games, {positions} positions ({split_positions} with a split king), {moves} moves compared");
    println!("  including {mates} checkmates ({split_mates} of a split king), {stalemates} stalemates, {collapses} white moves that collapse the king");
}

/// Checkmates of a split king: can a white move ever deliver one? Any that
/// can is built in the engine (black splits, white mates) and must be
/// checkmate there too.
fn split_mates(ix: &Ix) {
    let mates: Vec<St> = (0..N)
        .filter_map(|i| {
            let s = ix.state(i);
            (s.split() && ix.index(&s) == i && s.valid(false) && s.terminal() == 1).then_some(s)
        })
        .collect();
    let (mut reachable, mut built) = (0, 0);
    for m in &mates {
        // White's last move: one piece came from some square.
        let preds: Vec<St> = (0..64u8)
            .flat_map(|sq| [St { wk: sq, ..*m }, St { wb: sq, ..*m }, St { wn: sq, ..*m }])
            .filter(|p| p != m && p.valid(true))
            .filter(|p| {
                let mut leads = false;
                p.white_moves(&mut |t| {
                    leads = t == *m;
                    !leads
                });
                leads
            })
            .collect();
        if preds.is_empty() {
            continue;
        }
        reachable += 1;
        // Black's move before: the king split from one square.
        'found: for p in &preds {
            for from in 0..64u8 {
                let b = p.with_k(from, from);
                let mut leads = false;
                if b.valid(false) {
                    b.black_moves(true, &mut |o| {
                        leads = o == Out::To(*p);
                        !leads
                    });
                }
                if !leads {
                    continue;
                }
                let g = Game::from_fen(&fen_of(&b, false)).unwrap();
                let k1 = ai::children(&g, true).into_iter().find(|c| from_game(&c.game) == Out::To(*p)).expect("split");
                let k2 = ai::children(&k1.game, false).into_iter().find(|c| from_game(&c.game) == Out::To(*m)).expect("mating move");
                assert_eq!(k2.game.status(), Status::Checkmate { winner: qchess::Color::White });
                built += 1;
                break 'found;
            }
        }
    }
    println!(
        "{} checkmates of a split king exist on paper (up to symmetry); {reachable} can follow a white move ({built} built in the engine and confirmed)",
        mates.len()
    );
}

const TB_PATH: &str = "target/qkbn.tb";

fn load_or_solve(ix: &Ix) -> (Table, Table) {
    if let Ok(bytes) = std::fs::read(TB_PATH)
        && bytes.len() == 4 * N
    {
        let mk = |o: usize| bytes[o * N..(o + 1) * N].iter().map(|&b| AtomicU8::new(b)).collect();
        return (Table { w: mk(0), l: mk(1), quantum: false }, Table { w: mk(2), l: mk(3), quantum: true });
    }
    eprintln!("classical defender:");
    let c = Table::solve(ix, false, true);
    eprintln!("defender who may split:");
    let q = Table::solve(ix, true, true);
    let mut bytes = Vec::with_capacity(4 * N);
    for v in [&c.w, &c.l, &q.w, &q.l] {
        bytes.extend(v.iter().map(|a| a.load(Relaxed)));
    }
    if std::fs::write(TB_PATH, bytes).is_ok() {
        eprintln!("saved {TB_PATH}");
    }
    (c, q)
}

fn summary(ix: &Ix, c: &Table, q: &Table) {
    // White to move, black king on one square: the positions you can set up.
    let mut hist_c = [0usize; 256];
    let mut hist_q = [0usize; 256];
    let (mut total, mut slower, mut lost_win) = (0usize, 0usize, 0usize);
    let (mut worst, mut worst_s) = (0u8, None);
    for i in 0..N {
        let s = ix.state(i);
        if s.split() || c.w[i].load(Relaxed) == 255 {
            continue;
        }
        total += 1;
        let (a, b) = (c.w[i].load(Relaxed), q.w[i].load(Relaxed));
        hist_c[a as usize] += 1;
        hist_q[b as usize] += 1;
        if a != 0 && b == 0 {
            lost_win += 1;
        }
        if a != 0 && b > a {
            slower += 1;
        }
        if b > worst {
            worst = b;
            worst_s = Some(s);
        }
    }
    let won = |h: &[usize; 256]| h[1..255].iter().sum::<usize>();
    let max = |h: &[usize; 256]| (1..255).rev().find(|&d| h[d] > 0).unwrap_or(0);
    println!("white to move, black king on one square: {total} positions (up to symmetry)");
    println!("  classical defender: {} won, longest mate {} moves", won(&hist_c), max(&hist_c));
    println!("  defender may split: {} won, longest mate {} moves", won(&hist_q), max(&hist_q));
    println!("  wins that splitting saves: {lost_win}; wins that splitting makes longer: {slower}");
    if let Some(s) = worst_s {
        println!("  longest with splits: {} (mate in {worst}, classical {})", fen_of(&s, true), c.w(ix, &s));
    }
    let _ = (c.quantum, q.quantum);
}

/// Plays the table's fastest win from `g` through the engine. Black plays
/// its longest defence, or random legal moves with `rng`. Checks every
/// step against the table and that the engine calls the end checkmate.
/// Returns the moves and how many black moves left the king split.
fn drive(ix: &Ix, q: &Table, mut g: Game, mut rng: Option<&mut Rng>) -> (Vec<String>, usize) {
    let (mut line, mut splits) = (Vec::new(), 0);
    while g.status() == Status::Ongoing {
        let Out::To(s) = from_game(&g) else { panic!("a piece was lost after {line:?}") };
        let white = g.to_move() == qchess::Color::White;
        let kids = ai::children(&g, !white);
        let pick = if white {
            let best = kids
                .iter()
                .filter_map(|c| match from_game(&c.game) {
                    Out::To(t) if q.l(ix, &t) != 0 => Some((q.l(ix, &t), c)),
                    _ => None,
                })
                .min_by_key(|x| x.0)
                .expect("no winning move");
            // Mated after l-1 more white moves, so this move is one of w.
            assert_eq!(best.0, q.w(ix, &s), "table disagrees with itself at {s:?}");
            best.1
        } else {
            let lost = q.l(ix, &s);
            let score = |c: &ai::Child| match from_game(&c.game) {
                Out::Capture => panic!("black can capture at {s:?}, but the table says lost"),
                Out::To(t) => {
                    let w = q.w(ix, &t);
                    assert!(w != 0 && w < lost, "black escapes at {s:?} with {}", c.notation);
                    // Longest defence; among equals prefer splits.
                    w as usize * 4 + t.split() as usize
                }
            };
            let scores: Vec<usize> = kids.iter().map(score).collect();
            assert_eq!(scores.iter().max().map(|m| m / 4), Some(lost as usize - 1), "longest defence at {s:?}");
            match rng.as_deref_mut() {
                Some(r) => &kids[r.below(kids.len())],
                None => &kids[(0..kids.len()).max_by_key(|&i| scores[i]).unwrap()],
            }
        };
        if !white && matches!(from_game(&pick.game), Out::To(t) if t.split()) {
            splits += 1;
        }
        line.push(pick.notation.clone());
        g = pick.game.clone();
    }
    assert!(matches!(g.status(), Status::Checkmate { winner: qchess::Color::White }), "{:?} after {line:?}", g.status());
    (line, splits)
}

fn numbered(line: &[String], white_first: bool) -> String {
    let mut text = String::new();
    for (i, m) in line.iter().enumerate() {
        let ply = i + usize::from(!white_first);
        if ply % 2 == 0 {
            text += &format!("{}. ", ply / 2 + 1);
        } else if i == 0 {
            text += "1... ";
        }
        text += m;
        text.push(' ');
    }
    text.trim_end().to_string()
}

/// Fastest win against the longest defence, through the engine.
fn play(ix: &Ix, q: &Table, fen: &str) {
    let g = Game::from_fen(fen).unwrap();
    let Out::To(s) = from_game(&g) else { panic!() };
    let white = g.to_move() == qchess::Color::White;
    let predicted = if white { q.w(ix, &s) } else { q.l(ix, &s).saturating_sub(1) };
    if predicted == 0 || predicted == 255 {
        println!("{fen}: not a forced win against a king that may split");
        return;
    }
    println!("{fen}: mate in {predicted} against a king that may split");
    let (line, splits) = drive(ix, q, g, None);
    println!("{}", numbered(&line, white));
    println!("checkmate (the engine agrees) after {} plies; black's king was split after {splits} of its moves", line.len());
    assert_eq!(line.len(), predicted as usize * 2 - usize::from(white));
}

/// Random won positions; white plays the table, black plays at random.
fn verify(ix: &Ix, q: &Table, games: usize) {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut plies, mut splits, mut split_mates) = (0, 0, 0);
    for _ in 0..games {
        let s = loop {
            let s = random_start(&mut rng, true);
            if (1..255).contains(&q.w(ix, &s)) {
                break s;
            }
        };
        let g = Game::from_fen(&fen_of(&s, true)).unwrap();
        let mut walk = Rng(rng.next());
        let (line, n) = drive(ix, q, g.clone(), Some(&mut walk));
        assert!(line.len() < q.w(ix, &s) as usize * 2, "slower than the table promised");
        // Was the mated king split?
        let mut end = g;
        for m in &line {
            end.play(m).unwrap();
        }
        split_mates += (end.state_count() > 1) as usize;
        plies += line.len();
        splits += n;
    }
    println!("{games} random won positions: white mated a randomly moving black every time, within the predicted number of moves");
    println!("  {plies} plies through the engine, black's king split after {splits} moves, {split_mates} mates of a split king");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ix = Ix::new();
    match args.first().map(String::as_str) {
        Some("check") => {
            check(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2000));
            split_mates(&ix);
        }
        Some("solve") => {
            let _ = std::fs::remove_file(TB_PATH);
            let (c, q) = load_or_solve(&ix);
            summary(&ix, &c, &q);
        }
        Some("play") => {
            let (_, q) = load_or_solve(&ix);
            let fen = args.get(1).map_or("8/8/8/4k3/8/8/8/1NB1K3 w - - 0 1", |s| s.as_str());
            play(&ix, &q, fen);
        }
        Some("nopass") => {
            // What if a split king couldn't pass by swapping its halves?
            let (c, _) = load_or_solve(&ix);
            eprintln!("defender who may split but not pass:");
            let q = Table::solve(&ix, true, false);
            summary(&ix, &c, &q);
        }
        Some("verify") => {
            let (_, q) = load_or_solve(&ix);
            verify(&ix, &q, args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000));
        }
        _ => eprintln!("usage: qkbn check [walks] | solve | play [FEN] | verify [games] | nopass"),
    }
}
