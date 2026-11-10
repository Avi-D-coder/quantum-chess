//! King, bishop and knight against a lone king, with White's king allowed
//! to split too. (`qkbn.rs` solves the version where White never splits.)
//!
//! `cargo run --release -p qchess --example qkbn2 -- <mode>`:
//! * `check [walks]`: compares these rules with the engine on random games.
//! * `solve [classical-white]`: solves every position (about 2.5 billion per
//!   side up to symmetry), prints the result, and checks the wins through
//!   the engine. `classical-white` forbids White's king splits, which must
//!   reproduce `qkbn.rs`.
//!
//! Positions: White's king and Black's king each on one square or split
//! across two; bishop and knight on one square each. A position is every
//! combination of the kings' halves, and each must be a legal board.
//!
//! Instead of special cases, a move is worked out the way the engine does
//! it (`quantum.rs`): the moving piece's halves get their new squares,
//! every board a legal classical branch reaches is kept, and if some
//! combination of the new halves isn't one of them, pieces collapse: the
//! fewest pieces that leave only reachable boards, the mover choosing when
//! there's more than one way. Captures collapse too, and taking the bishop
//! or knight ends the win.
//!
//! White never splits the bishop or knight. That limit only removes White
//! options, so every win here is a real win.
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

const fn rays() -> [[u64; 4]; 64] {
    let mut t = [[0u64; 4]; 64];
    let d = [(1i32, 1i32), (1, -1), (-1, 1), (-1, -1)];
    let mut s = 0;
    while s < 64 {
        let mut i = 0;
        while i < 4 {
            let (mut f, mut r) = ((s % 8) as i32 + d[i].0, (s / 8) as i32 + d[i].1);
            while f >= 0 && f < 8 && r >= 0 && r < 8 {
                t[s][i] |= 1 << (r * 8 + f);
                f += d[i].0;
                r += d[i].1;
            }
            i += 1;
        }
        s += 1;
    }
    t
}
const RAYS: [[u64; 4]; 64] = rays();

fn bishop(s: u8, occ: u64) -> u64 {
    let mut a = 0;
    for (i, &ray) in RAYS[s as usize].iter().enumerate() {
        let blockers = ray & occ;
        a |= if blockers == 0 {
            ray
        } else {
            // Rays 0 and 2 go up the board (lowest blocker first), 1 and 3 down.
            let first = if i % 2 == 0 { blockers.trailing_zeros() } else { 63 - blockers.leading_zeros() };
            ray & !RAYS[first as usize][i]
        };
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

/// `w`: White's king halves, `k`: Black's (equal when not split, else
/// ascending); `b`, `n`: bishop and knight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Q {
    w: [u8; 2],
    b: u8,
    n: u8,
    k: [u8; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Out {
    /// Black took the bishop or knight.
    Capture,
    To(Q),
}

fn pair(a: u8, b: u8) -> [u8; 2] {
    [a.min(b), a.max(b)]
}
fn halves(p: &[u8; 2]) -> &[u8] {
    if p[0] == p[1] { &p[..1] } else { &p[..] }
}

/// One classical board: white king, black king, bishop, knight.
#[derive(Clone, Copy, PartialEq, Eq)]
struct B {
    wk: u8,
    bk: u8,
    b: u8,
    n: u8,
}

impl B {
    fn distinct(&self) -> bool {
        let s = [self.wk, self.bk, self.b, self.n];
        (0..4).all(|i| (i + 1..4).all(|j| s[i] != s[j]))
    }
    /// Is `x` attacked by white (black's king off the board)?
    fn attacked(&self, x: u8) -> bool {
        let occ = bit(self.wk) | bit(self.b) | bit(self.n);
        KING[self.wk as usize] & bit(x) != 0
            || (x != self.n && KNIGHT[self.n as usize] & bit(x) != 0)
            || (x != self.b && bishop(self.b, occ) & bit(x) != 0)
    }
    fn black_in_check(&self) -> bool {
        self.attacked(self.bk)
    }
    /// Legal destinations of black's king (captures included).
    fn black_dests(&self) -> u64 {
        bits(KING[self.bk as usize] & !KING[self.wk as usize] & !bit(self.wk)).filter(|&t| !self.attacked(t)).fold(0, |m, t| m | bit(t))
    }
    fn white_king_dests(&self) -> u64 {
        KING[self.wk as usize] & !KING[self.bk as usize] & !bit(self.bk) & !bit(self.b) & !bit(self.n)
    }
    fn bishop_dests(&self) -> u64 {
        bishop(self.b, bit(self.wk) | bit(self.n) | bit(self.bk)) & !bit(self.wk) & !bit(self.n) & !bit(self.bk)
    }
    fn knight_dests(&self) -> u64 {
        KNIGHT[self.n as usize] & !bit(self.wk) & !bit(self.b) & !bit(self.bk)
    }
    fn legal(&self, white_to_move: bool) -> bool {
        self.distinct() && KING[self.wk as usize] & bit(self.bk) == 0 && !(white_to_move && self.black_in_check())
    }
}

impl Q {
    fn boards(&self) -> impl Iterator<Item = B> + '_ {
        halves(&self.w).iter().flat_map(move |&wk| halves(&self.k).iter().map(move |&bk| B { wk, bk, b: self.b, n: self.n }))
    }
    fn valid(&self, white_to_move: bool) -> bool {
        self.boards().all(|b| b.legal(white_to_move))
    }
    fn split_w(&self) -> bool {
        self.w[0] != self.w[1]
    }
    fn split_k(&self) -> bool {
        self.k[0] != self.k[1]
    }
    /// Black to move: 1 = checkmate, 2 = stalemate, 0 = neither.
    fn terminal(&self) -> u8 {
        if self.boards().any(|b| b.black_dests() != 0) {
            0
        } else if self.boards().all(|b| b.black_in_check()) {
            1
        } else {
            2
        }
    }
}

/// Which piece moves.
#[derive(Clone, Copy, PartialEq)]
enum Mover {
    WK,
    BK,
    Bishop,
    Knight,
}

/// A branch: the moving half on `from` goes to `to`. (`None`, staying put, is
/// never a legal move; it is kept only so the resolver stays general.)
type Branch = (u8, Option<u8>);

/// Works out one move the way the engine does and reports each outcome
/// (one per collapse the mover may choose). `new` is the moving piece's
/// halves afterwards.
fn resolve(q: &Q, mover: Mover, new: [u8; 2], branches: &[Branch], f: &mut impl FnMut(Out) -> bool) -> bool {
    let white = mover != Mover::BK;
    // Boards reachable by a legal branch.
    let mut reach = [B { wk: 0, bk: 0, b: 0, n: 0 }; 8];
    let mut nr = 0;
    let mut real = false;
    let mut capture = false;
    for bd in q.boards() {
        let at = match mover {
            Mover::WK => bd.wk,
            Mover::BK => bd.bk,
            Mover::Bishop => bd.b,
            Mover::Knight => bd.n,
        };
        for &(from, to) in branches {
            if from != at {
                continue;
            }
            match to {
                None => {
                    let check = if white { false } else { bd.black_in_check() };
                    if !check {
                        reach[nr] = bd;
                        nr += 1;
                    }
                }
                Some(t) => {
                    let ok = match mover {
                        Mover::WK => bd.white_king_dests() & bit(t) != 0,
                        Mover::BK => bd.black_dests() & bit(t) != 0,
                        Mover::Bishop => bd.bishop_dests() & bit(t) != 0,
                        Mover::Knight => bd.knight_dests() & bit(t) != 0,
                    };
                    if ok {
                        real = true;
                        let mut nb = bd;
                        match mover {
                            Mover::WK => nb.wk = t,
                            Mover::BK => nb.bk = t,
                            Mover::Bishop => nb.b = t,
                            Mover::Knight => nb.n = t,
                        }
                        if mover == Mover::BK && (t == bd.b || t == bd.n) {
                            capture = true;
                        }
                        reach[nr] = nb;
                        nr += 1;
                    }
                }
            }
        }
    }
    if !real {
        return true;
    }
    if mover == Mover::BK && new.iter().any(|&t| t == q.b || t == q.n) {
        // The piece is gone from every board, so only the capturing branch
        // is consistent: legal only if that branch was.
        return if capture { f(Out::Capture) } else { true };
    }
    let mut next = *q;
    match mover {
        Mover::WK => next.w = new,
        Mover::BK => next.k = new,
        Mover::Bishop => next.b = new[0],
        Mover::Knight => next.n = new[0],
    }
    // Split pieces of the result: bit 0 white king, bit 1 black king.
    let ws = halves(&next.w);
    let ks = halves(&next.k);
    let good = |wi: usize, ki: usize| {
        let bd = B { wk: ws[wi], bk: ks[ki], b: next.b, n: next.n };
        bd.distinct() && reach[..nr].contains(&bd)
    };
    let (nw, nk) = (ws.len(), ks.len());
    let g = [[good(0, 0), nk > 1 && good(0, 1)], [nw > 1 && good(1, 0), nw > 1 && nk > 1 && good(1, 1)]];
    let all = (0..nw).all(|i| (0..nk).all(|j| g[i][j]));
    if all {
        return f(Out::To(next));
    }
    if !(0..nw).any(|i| (0..nk).any(|j| g[i][j])) {
        return true;
    }
    // Fewest pieces to fix so that only good boards remain.
    let fix = |wi: Option<usize>, ki: Option<usize>| {
        let wr = wi.map_or(0..nw, |i| i..i + 1);
        let kr = ki.map_or(0..nk, |j| j..j + 1);
        wr.clone().all(|i| kr.clone().all(|j| g[i][j])).then(|| {
            let mut o = next;
            if let Some(i) = wi {
                o.w = [ws[i]; 2];
            }
            if let Some(j) = ki {
                o.k = [ks[j]; 2];
            }
            o
        })
    };
    let mut one = [None; 4];
    let mut m = 0;
    for i in (0..nw).filter(|_| nw > 1) {
        one[m] = fix(Some(i), None);
        m += 1;
    }
    for j in (0..nk).filter(|_| nk > 1) {
        one[m] = fix(None, Some(j));
        m += 1;
    }
    if one.iter().all(Option::is_none) {
        m = 0;
        for i in 0..nw {
            for j in 0..nk {
                one[m] = fix(Some(i), Some(j));
                m += 1;
            }
        }
    }
    one.into_iter().flatten().all(|o| f(Out::To(o)))
}

/// Destinations the king half on `h` can legally reach in some board.
fn king_targets(q: &Q, white: bool, h: u8) -> u64 {
    q.boards()
        .filter(|bd| if white { bd.wk == h } else { bd.bk == h })
        .fold(0, |m, bd| m | if white { bd.white_king_dests() } else { bd.black_dests() })
}

/// Every move of the side to move's king (as the engine lists them).
fn king_moves(q: &Q, white: bool, quantum: bool, f: &mut impl FnMut(Out) -> bool) -> bool {
    let (mover, p) = if white { (Mover::WK, q.w) } else { (Mover::BK, q.k) };
    let hs = halves(&p);
    let mut ts = [0u64; 2];
    for (i, &h) in hs.iter().enumerate() {
        ts[i] = king_targets(q, white, h);
    }
    // Whole moves: every half tries.
    for t in bits(ts[0] | ts[1]) {
        let br = [(hs[0], Some(t)), (hs[hs.len() - 1], Some(t))];
        if !resolve(q, mover, [t, t], &br[..hs.len()], f) {
            return false;
        }
    }
    if !quantum {
        return true;
    }
    if hs.len() == 1 {
        let h = hs[0];
        for t in bits(ts[0]) {
            for t2 in bits(ts[0] & !(bit(t + 1) - 1)) {
                if !resolve(q, mover, pair(t, t2), &[(h, Some(t)), (h, Some(t2))], f) {
                    return false;
                }
            }
        }
    } else {
        for t0 in bits(ts[0]) {
            for t1 in bits(ts[1] & !bit(t0)) {
                if !resolve(q, mover, pair(t0, t1), &[(hs[0], Some(t0)), (hs[1], Some(t1))], f) {
                    return false;
                }
            }
        }
    }
    true
}

fn white_moves(q: &Q, king_splits: bool, f: &mut impl FnMut(Out) -> bool) {
    if !king_moves(q, true, king_splits, f) {
        return;
    }
    let bt = q.boards().fold(0, |m, bd| m | bd.bishop_dests());
    for t in bits(bt) {
        if !resolve(q, Mover::Bishop, [t, t], &[(q.b, Some(t))], f) {
            return;
        }
    }
    let nt = q.boards().fold(0, |m, bd| m | bd.knight_dests());
    for t in bits(nt) {
        if !resolve(q, Mover::Knight, [t, t], &[(q.n, Some(t))], f) {
            return;
        }
    }
}

fn black_moves(q: &Q, f: &mut impl FnMut(Out) -> bool) {
    king_moves(q, false, true, f);
}

// ---------- indexing (8-fold symmetry) ----------

fn kpair(p: [u8; 2]) -> usize {
    let (a, b) = (p[0] as usize, p[1] as usize);
    if a == b { a } else { 64 + b * (b - 1) / 2 + a }
}
fn unpair(i: usize) -> [u8; 2] {
    if i < 64 {
        return [i as u8; 2];
    }
    let i = i - 64;
    let mut b = 1;
    while (b + 1) * b / 2 <= i {
        b += 1;
    }
    [(i - b * (b - 1) / 2) as u8, b as u8]
}
const PAIRS: usize = 64 + 64 * 63 / 2;

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
    xf: [[u8; 64]; 8],
    /// For each white king pair: its class and the transforms onto the
    /// class representative.
    class: Vec<(u32, Vec<u8>)>,
    reps: Vec<[u8; 2]>,
}

impl Ix {
    fn new() -> Ix {
        let xft: [[u8; 64]; 8] = std::array::from_fn(|t| std::array::from_fn(|s| xf(t as u8, s as u8)));
        let img = |t: usize, p: [u8; 2]| kpair(pair(xft[t][p[0] as usize], xft[t][p[1] as usize]));
        let rep_of: Vec<usize> = (0..PAIRS).map(|i| (0..8).map(|t| img(t, unpair(i))).min().unwrap()).collect();
        let mut reps: Vec<usize> = rep_of.clone();
        reps.sort_unstable();
        reps.dedup();
        let class = (0..PAIRS)
            .map(|i| {
                let id = reps.binary_search(&rep_of[i]).unwrap() as u32;
                (id, (0..8u8).filter(|&t| img(t as usize, unpair(i)) == rep_of[i]).collect())
            })
            .collect();
        Ix { xf: xft, class, reps: reps.into_iter().map(unpair).collect() }
    }
    fn len(&self) -> usize {
        self.reps.len() * 64 * 64 * PAIRS
    }
    fn index(&self, q: &Q) -> usize {
        let (id, ts) = &self.class[kpair(q.w)];
        let mut best = usize::MAX;
        for &t in ts {
            let m = &self.xf[t as usize];
            let i = ((*id as usize * 64 + m[q.b as usize] as usize) * 64 + m[q.n as usize] as usize) * PAIRS
                + kpair(pair(m[q.k[0] as usize], m[q.k[1] as usize]));
            best = best.min(i);
        }
        best
    }
    fn state(&self, i: usize) -> Q {
        let k = unpair(i % PAIRS);
        let i = i / PAIRS;
        Q { w: self.reps[i / 4096], b: (i / 64 % 64) as u8, n: (i % 64) as u8, k }
    }
}

// ---------- solving ----------

/// Zeroed atomic bytes (the pages are only touched when written).
fn zeroed(n: usize) -> Vec<AtomicU8> {
    let v = vec![0u8; n];
    let mut v = std::mem::ManuallyDrop::new(v);
    // SAFETY: AtomicU8 has the same size and alignment as u8.
    unsafe { Vec::from_raw_parts(v.as_mut_ptr() as *mut AtomicU8, v.len(), v.capacity()) }
}

fn par(n: usize, f: impl Fn(usize) -> usize + Sync) -> usize {
    let next = AtomicUsize::new(0);
    let total = AtomicUsize::new(0);
    const CHUNK: usize = 1 << 14;
    std::thread::scope(|sc| {
        for _ in 0..std::thread::available_parallelism().map_or(8, |n| n.get()) {
            sc.spawn(|| {
                let mut c = 0;
                loop {
                    let start = next.fetch_add(CHUNK, Relaxed);
                    if start >= n {
                        break;
                    }
                    for i in start..(start + CHUNK).min(n) {
                        c += f(i);
                    }
                }
                total.fetch_add(c, Relaxed);
            });
        }
    });
    total.into_inner()
}

/// `w[i]`: white to move mates within `w[i]` moves (0 = no win found,
/// 255 = not a position). `l[i]`: black to move is mated within `l[i] - 1`
/// more white moves (0 = not lost, 255 = not a position). Values are
/// bounds along a winning strategy, strictly falling along it.
struct Table {
    w: Vec<AtomicU8>,
    l: Vec<AtomicU8>,
    king_splits: bool,
}

impl Table {
    fn solve(ix: &Ix, king_splits: bool) -> Table {
        let t0 = std::time::Instant::now();
        let n = ix.len();
        let tb = Table { w: zeroed(n), l: zeroed(n), king_splits };
        let mates = par(n, |i| {
            let q = ix.state(i);
            let skip = ix.index(&q) != i || (!king_splits && q.split_w());
            if skip || !q.valid(true) {
                tb.w[i].store(255, Relaxed);
            }
            if skip || !q.valid(false) {
                tb.l[i].store(255, Relaxed);
                return 0;
            }
            if q.terminal() == 1 {
                tb.l[i].store(1, Relaxed);
                return 1;
            }
            0
        });
        eprintln!("  {n} indices, {mates} checkmates ({:.0?})", t0.elapsed());
        // Repeat until nothing changes; each value only ever goes from 0 to
        // proven, using values proven earlier (so it is a least fixed point:
        // exactly the forced wins).
        for pass in 1.. {
            let changed = par(n, |i| {
                let mut c = 0;
                if tb.w[i].load(Relaxed) == 0 {
                    let q = ix.state(i);
                    let mut best = 255u8;
                    white_moves(&q, king_splits, &mut |o| {
                        if let Out::To(t) = o {
                            let l = tb.l[ix.index(&t)].load(Relaxed);
                            if l != 0 && l < best {
                                best = l;
                            }
                        }
                        best > 1
                    });
                    if best < 255 {
                        tb.w[i].store(best, Relaxed);
                        c += 1;
                    }
                }
                if tb.l[i].load(Relaxed) == 0 {
                    let q = ix.state(i);
                    let (mut all, mut worst, mut any) = (true, 0u8, false);
                    black_moves(&q, &mut |o| {
                        any = true;
                        match o {
                            Out::Capture => all = false,
                            Out::To(t) => {
                                let w = tb.w[ix.index(&t)].load(Relaxed);
                                if w == 0 || w == 255 {
                                    all = false;
                                }
                                worst = worst.max(w);
                            }
                        }
                        all
                    });
                    if all && any && worst < 254 {
                        tb.l[i].store(worst + 1, Relaxed);
                        c += 1;
                    }
                }
                c
            });
            eprintln!("  pass {pass}: {changed} newly proven ({:.0?})", t0.elapsed());
            if changed == 0 {
                break;
            }
        }
        tb
    }
    fn w(&self, ix: &Ix, q: &Q) -> u8 {
        self.w[ix.index(q)].load(Relaxed)
    }
    fn l(&self, ix: &Ix, q: &Q) -> u8 {
        self.l[ix.index(q)].load(Relaxed)
    }
}

// ---------- the engine side ----------

fn parse_board(fen: &str) -> [Option<u8>; 4] {
    let mut out = [None; 4];
    for (ri, row) in fen.split('/').enumerate() {
        let mut f = 0u8;
        for c in row.chars() {
            if let Some(d) = c.to_digit(10) {
                f += d as u8;
                continue;
            }
            let slot = match c {
                'K' => 0,
                'B' => 1,
                'N' => 2,
                'k' => 3,
                _ => panic!("unexpected piece {c} in {fen}"),
            };
            out[slot] = Some((7 - ri as u8) * 8 + f);
            f += 1;
        }
    }
    out
}

fn from_game(g: &Game) -> Out {
    let boards: Vec<_> = g.states_fen().iter().map(|f| parse_board(f)).collect();
    if boards.iter().any(|b| b[1].is_none() || b[2].is_none()) {
        return Out::Capture;
    }
    let (b, n) = (boards[0][1].unwrap(), boards[0][2].unwrap());
    assert!(boards.iter().all(|x| x[1] == Some(b) && x[2] == Some(n)), "bishop or knight split");
    let ws: BTreeSet<u8> = boards.iter().map(|x| x[0].unwrap()).collect();
    let ks: BTreeSet<u8> = boards.iter().map(|x| x[3].unwrap()).collect();
    assert!(ws.len() <= 2 && ks.len() <= 2 && boards.len() == ws.len() * ks.len(), "not a product: {:?}", g.states_fen());
    let v = |s: BTreeSet<u8>| {
        let v: Vec<u8> = s.into_iter().collect();
        [v[0], *v.last().unwrap()]
    };
    Out::To(Q { w: v(ws), b, n, k: v(ks) })
}

fn fen_of(q: &Q, white: bool) -> String {
    assert!(!q.split_w() && !q.split_k());
    let mut b = [['1'; 8]; 8];
    for (sq, c) in [(q.w[0], 'K'), (q.b, 'B'), (q.n, 'N'), (q.k[0], 'k')] {
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

/// The engine's moves for this side, without bishop or knight splits.
fn engine_kids(g: &Game) -> Vec<ai::Child> {
    ai::children(g, true)
        .into_iter()
        .filter(|c| {
            let m = c.notation.split('{').next().unwrap();
            !(m.starts_with('B') || m.starts_with('N')) || !m.contains('/')
        })
        .collect()
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

fn random_start(rng: &mut Rng) -> Q {
    loop {
        let k = rng.below(64) as u8;
        let w = rng.below(64) as u8;
        let q = Q { w: [w; 2], b: rng.below(64) as u8, n: rng.below(64) as u8, k: [k; 2] };
        if q.valid(true) {
            return q;
        }
    }
}

fn check(walks: usize) {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut positions, mut both_split, mut moves, mut mates, mut choices) = (0, 0, 0, 0, 0);
    for w in 0..walks {
        let white = w % 2 == 0;
        let q0 = random_start(&mut rng);
        let mut g = Game::from_fen(&fen_of(&q0, white)).unwrap();
        for _ in 0..50 {
            let Out::To(q) = from_game(&g) else { break };
            positions += 1;
            both_split += (q.split_w() && q.split_k()) as usize;
            let white = g.to_move() == qchess::Color::White;
            let kids = engine_kids(&g);
            let engine: BTreeSet<Out> = kids.iter().map(|c| from_game(&c.game)).collect();
            choices += kids.iter().filter(|c| c.notation.contains('{')).count();
            let mut mine = BTreeSet::new();
            let mut add = |o| {
                mine.insert(o);
                true
            };
            if white {
                white_moves(&q, true, &mut add);
            } else {
                black_moves(&q, &mut add);
            }
            moves += kids.len();
            if engine != mine {
                println!("MISMATCH at {q:?} ({} to move), boards {:?}", if white { "white" } else { "black" }, g.states_fen());
                for o in engine.difference(&mine) {
                    let n: Vec<_> = kids.iter().filter(|c| from_game(&c.game) == *o).map(|c| c.notation.clone()).collect();
                    println!("  engine only: {o:?} via {n:?}");
                }
                for o in mine.difference(&engine) {
                    println!("  rules only: {o:?}");
                }
                std::process::exit(1);
            }
            let want = match if white { 0 } else { q.terminal() } {
                1 => Status::Checkmate { winner: qchess::Color::White },
                2 => Status::Stalemate,
                _ => Status::Ongoing,
            };
            assert_eq!(g.status(), want, "status at {q:?}");
            mates += matches!(want, Status::Checkmate { .. }) as usize;
            if kids.is_empty() {
                break;
            }
            // White steers its king towards black's half the time.
            let pick = if white && rng.below(2) == 0 {
                let d = |c: &ai::Child, r: &mut Rng| match from_game(&c.game) {
                    Out::To(t) => {
                        let dist = |a: u8, b: u8| (a % 8).abs_diff(b % 8).max((a / 8).abs_diff(b / 8)) as usize;
                        halves(&t.w).iter().map(|&a| halves(&t.k).iter().map(|&b| dist(a, b)).sum::<usize>()).sum::<usize>() * 64 + r.below(64)
                    }
                    Out::Capture => usize::MAX,
                };
                let scores: Vec<usize> = kids.iter().map(|c| d(c, &mut rng)).collect();
                &kids[(0..kids.len()).min_by_key(|&i| scores[i]).unwrap()]
            } else {
                &kids[rng.below(kids.len())]
            };
            g = pick.game.clone();
        }
    }
    println!("rules agree with the engine: {walks} games, {positions} positions ({both_split} with both kings split), {moves} moves");
    println!("  {choices} moves with a collapse choice, {mates} checkmates");
}

/// Plays the table's win through the engine (black: longest defence, or
/// random with `rng`), checking the table at every step.
fn drive(ix: &Ix, tb: &Table, mut g: Game, mut rng: Option<&mut Rng>) -> Vec<String> {
    let mut line = Vec::new();
    while g.status() == Status::Ongoing {
        let Out::To(q) = from_game(&g) else { panic!("piece lost after {line:?}") };
        let white = g.to_move() == qchess::Color::White;
        let kids: Vec<ai::Child> = if white && !tb.king_splits { ai::children(&g, false) } else { engine_kids(&g) };
        let pick = if white {
            let (l, c) = kids
                .iter()
                .filter_map(|c| match from_game(&c.game) {
                    Out::To(t) if tb.l(ix, &t) != 0 => Some((tb.l(ix, &t), c)),
                    _ => None,
                })
                .min_by_key(|x| x.0)
                .expect("no winning move");
            assert!(l <= tb.w(ix, &q), "table bound broken at {q:?}");
            c
        } else {
            let lost = tb.l(ix, &q);
            let scores: Vec<u8> = kids
                .iter()
                .map(|c| match from_game(&c.game) {
                    Out::Capture => panic!("black can capture at {q:?}"),
                    Out::To(t) => {
                        let w = tb.w(ix, &t);
                        assert!(w != 0 && w < lost, "black escapes at {q:?} with {}", c.notation);
                        w
                    }
                })
                .collect();
            match rng.as_deref_mut() {
                Some(r) => &kids[r.below(kids.len())],
                None => &kids[(0..kids.len()).max_by_key(|&i| scores[i]).unwrap()],
            }
        };
        line.push(pick.notation.clone());
        g = pick.game.clone();
    }
    assert!(matches!(g.status(), Status::Checkmate { winner: qchess::Color::White }), "{:?} after {line:?}", g.status());
    line
}

fn report(ix: &Ix, tb: &Table) {
    // Set-up positions: white to move, nothing split, won in normal chess.
    let classical = load_classical(ix);
    let (mut total, mut won_classically, mut won, mut won_of_classical, mut longest) = (0usize, 0usize, 0usize, 0usize, 0u8);
    let mut longest_q = None;
    let mut drawn = Vec::new();
    for (i, wv) in tb.w.iter().enumerate() {
        let q = ix.state(i);
        let w = wv.load(Relaxed);
        if q.split_w() || q.split_k() || w == 255 {
            continue;
        }
        total += 1;
        let c = classical.as_ref().map(|f| f(&q));
        if w != 0 {
            won += 1;
            if w > longest {
                longest = w;
                longest_q = Some(q);
            }
        }
        if c == Some(true) {
            won_classically += 1;
            if w != 0 {
                won_of_classical += 1;
            } else if drawn.len() < 5 {
                drawn.push(q);
            }
        }
    }
    let rules = if tb.king_splits { "White's king may split too" } else { "White never splits" };
    println!("{rules}: white to move, nothing split yet: {total} positions (up to symmetry)");
    println!("  won: {won}; longest win found: {longest} moves{}", longest_q.map(|q| format!(" ({})", fen_of(&q, true))).unwrap_or_default());
    if classical.is_some() {
        println!("  of the {won_classically} won in normal chess, won here: {won_of_classical} ({:.2}%)", 100.0 * won_of_classical as f64 / won_classically as f64);
        for q in &drawn {
            println!("  still a draw, e.g. {}", fen_of(q, true));
        }
    }
    // Every win through the engine against random defence.
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut games, mut plies) = (0, 0);
    while games < 300 {
        let q = random_start(&mut rng);
        if !(1..255).contains(&tb.w(ix, &q)) {
            continue;
        }
        let mut r = Rng(rng.next());
        plies += drive(ix, tb, Game::from_fen(&fen_of(&q, true)).unwrap(), Some(&mut r)).len();
        games += 1;
    }
    println!("  {games} random wins played through the engine against random defence: all checkmate ({plies} plies)");
    if let Some(q) = longest_q {
        let line = drive(ix, tb, Game::from_fen(&fen_of(&q, true)).unwrap(), None);
        println!("  longest, against the longest defence ({} plies): {}", line.len(), line.join(" "));
    }
}

/// Normal-chess results from `qkbn.rs`'s table (`target/qkbn.tb`), if built.
#[allow(clippy::type_complexity)]
fn load_classical(ix: &Ix) -> Option<Box<dyn Fn(&Q) -> bool>> {
    let _ = ix;
    let bytes = std::fs::read("target/qkbn.tb").ok()?;
    // qkbn.rs layout: [classical w][classical l][quantum w][quantum l], each
    // 10 * 64 * 64 * 2080, white king in the a1-d1-d4 triangle.
    const TRI: [u8; 10] = [0, 1, 2, 3, 9, 10, 11, 18, 19, 27];
    let n = 10 * 64 * 64 * PAIRS;
    if bytes.len() != 4 * n {
        return None;
    }
    Some(Box::new(move |q: &Q| {
        let mut best = usize::MAX;
        for t in 0..8 {
            let m = |s: u8| xf(t, s);
            if let Some(tri) = TRI.iter().position(|&s| s == m(q.w[0])) {
                let i = ((tri * 64 + m(q.b) as usize) * 64 + m(q.n) as usize) * PAIRS + kpair(pair(m(q.k[0]), m(q.k[1])));
                best = best.min(i);
            }
        }
        let v = bytes[best];
        v != 0 && v != 255
    }))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => check(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2000)),
        Some("solve") => {
            let ix = Ix::new();
            let king_splits = args.get(1).map(String::as_str) != Some("classical-white");
            eprintln!("{} classes of white king placement, {} indices per side", ix.reps.len(), ix.len());
            let tb = Table::solve(&ix, king_splits);
            report(&ix, &tb);
        }
        _ => eprintln!("usage: qkbn2 check [walks] | solve [classical-white]"),
    }
}
