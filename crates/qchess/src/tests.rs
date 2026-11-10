//! Engine tests, mostly taken straight from the rule sheet.

use crate::*;

fn play_all(g: &mut Game, moves: &[&str]) {
    for m in moves {
        if let Err(e) = g.play(m) {
            panic!("move {m} failed: {e}\n{}", g.board_text());
        }
    }
}

fn game(moves: &[&str]) -> Game {
    let mut g = Game::new();
    play_all(&mut g, moves);
    g
}

fn fen_game(fen: &str, moves: &[&str]) -> Game {
    let mut g = Game::from_fen(fen).unwrap();
    play_all(&mut g, moves);
    g
}

#[test]
fn sheet_example_opening() {
    // "Van't Kruijs Italian Ruy Lopez Berlin-Cozio defense"
    let g = game(&["e3/4", "e5", "Nf3", "Nc6", "Bb5/c4", "Ne7/f6", "0-0/"]);
    assert_eq!(g.state_count(), 16, "{}", g.board_text());
    assert_eq!(g.to_move(), Color::Black);
    let fens = g.states_fen();
    // Castled: king g1, rook f1. Not castled: king e1, rook f1 (a rook move).
    assert!(fens.iter().any(|f| f.ends_with("RNBQ1RK1")));
    assert!(fens.iter().any(|f| f.ends_with("RNBQKR2")));
    let text = g.board_text();
    assert!(text.contains("white king: g1 | e1") || text.contains("white king: e1 | g1"), "{text}");
}

#[test]
fn moving_through_collapses_the_piece_passed() {
    let mut g = game(&["e4", "a6", "d3/4", "a5"]);
    let r = g.play("Bc4").unwrap();
    assert_eq!(r.collapses, vec!["white pawn 4 collapsed to d4"]);
    assert_eq!(g.state_count(), 1);
}

#[test]
fn moving_onto_own_quantum_piece_collapses_it() {
    let mut g = game(&["d3/4", "a6"]);
    let r = g.play("Qd3").unwrap();
    assert_eq!(r.collapses, vec!["white pawn 4 collapsed to d4"]);
}

#[test]
fn blocked_in_every_state_is_illegal() {
    let mut g = game(&["d3/4", "a6"]);
    assert!(g.play("Qd5").is_err());
    assert!(g.play("Qd4").is_err());
    assert_eq!(g.history().len(), 2);
}

#[test]
fn pawn_capture_removes_both_halves() {
    let mut g = game(&["e4", "d5/6"]);
    assert_eq!(g.state_count(), 2);
    let r = g.play("exd5").unwrap();
    assert_eq!(r.collapses, vec!["black pawn 4 collapsed to captured (on d5)"]);
    assert_eq!(g.state_count(), 1);
    assert!(g.states_fen()[0].starts_with("rnbqkbnr/ppp1pppp/8/3P4/"));
}

#[test]
fn half_capture_forces_a_collapse() {
    // Rule sheet, "Capturing Using Quantum Pieces": the split knight moves
    // both halves, c3 takes on d5 and a3 goes to b5. The board "knight on
    // b5, pawn captured" is impossible, so either the knight collapses onto
    // d5 or the pawn onto d6 (it wasn't captured). The mover picks.
    let mut g = game(&["Nc3/a3", "d5/6"]);
    assert_eq!(g.state_count(), 4);
    let r = g.play("Ncxd5/ab5 {Nd5}").unwrap();
    assert_eq!(r.collapses, vec!["white knight 1 collapsed to d5"]);
    assert_eq!(g.state_count(), 2);
    assert!(g.board_text().contains("black pawn: captured on d5 | d6"), "{}", g.board_text());

    let mut g = game(&["Nc3/a3", "d5/6"]);
    let r = g.play("Ncxd5/ab5 {Pd6}").unwrap();
    assert_eq!(r.collapses, vec!["black pawn 4 collapsed to d6"]);
    assert!(g.board_text().contains("white knight: d5 | b5"), "{}", g.board_text());

    // Not a consistent choice.
    let mut g = game(&["Nc3/a3", "d5/6"]);
    assert!(g.play("Ncxd5/ab5 {Nb5}").is_err());
}

#[test]
fn leaving_half_a_piece_behind_is_illegal() {
    // Rule 3: on the boards where the staying half was real, nothing moved.
    for (fen, mv) in [
        (None, "Nf3/"),
        (None, "e4/"),
        (Some("4k3/8/8/8/8/8/8/2B1K3 w - - 0 1"), "Bd2/"),
        (Some("k7/4P3/8/8/8/8/8/4K3 w - - 0 1"), "e8=Q/"),
    ] {
        let mut g = match fen {
            Some(f) => Game::from_fen(f).unwrap(),
            None => Game::new(),
        };
        let e = g.play(mv).unwrap_err();
        assert!(e.0.contains("can't leave half"), "{mv}: {e}");
    }
    // A split piece can't move one half and leave the other either.
    let mut g = game(&["Nc3/a3", "e5"]);
    assert!(g.play("Ncd5/").is_err());
    // Quantum castling is the exception: the rook moves on the other boards.
    let mut g = fen_game("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", &[]);
    assert!(g.play("0-0/").is_ok());
}

#[test]
fn moving_one_half_of_a_split_piece_collapses_it_there() {
    // The other half can't make the move, so on its boards the move is
    // impossible: the piece is proved to have been on the moving half.
    let mut g = game(&["e4", "c6/5", "d4"]);
    let r = g.play("c5c4").unwrap();
    assert_eq!(r.collapses, vec!["black pawn 3 collapsed to c5"]);
    // Moving both halves keeps the superposition.
    let mut g = game(&["e4", "c6/5", "d4"]);
    let r = g.play("c6c5/c5c4").unwrap();
    assert!(r.collapses.is_empty(), "{:?}", r.collapses);
    assert_eq!(g.state_count(), 2);
}

#[test]
fn a_whole_piece_capturing_a_half_needs_no_collapse() {
    // Rule sheet, "Capturing Quantum Pieces": the twin stays, alive and dead.
    let mut g = game(&["Nc3/a3", "e5", "h3", "Bb4"]);
    let n = g.state_count();
    let r = g.play("h4").unwrap();
    assert!(r.collapses.is_empty());
    let r = g.play("Bxc3").unwrap();
    assert!(r.collapses.is_empty(), "{:?}", r.collapses);
    assert_eq!(g.state_count(), n);
    assert!(g.board_text().contains("white knight: captured on c3 | a3"), "{}", g.board_text());
}

#[test]
fn capturing_both_halves_with_both_halves_needs_no_collapse() {
    let mut g = fen_game("4k3/8/2b5/8/8/8/8/3NK3 w - - 0 1", &["Nc3/e3", "Bb5/d5"]);
    let r = g.play("Nxb5/xd5").unwrap();
    assert!(r.collapses.is_empty(), "{:?}", r.collapses);
    assert_eq!(g.state_count(), 2);
    // The bishop is gone in every possible state.
    assert!(g.states_fen().iter().all(|f| !f.contains('b')));
}

#[test]
fn capture_when_other_half_already_dead() {
    // The bishop ends up half-captured. Capturing its last half with another
    // quantum piece then needs no collapse.
    let mut g = fen_game("4k3/8/2b5/8/8/8/8/1R1NK3 w - - 0 1", &["Nc3/e3", "Bb5/d5"]);
    let r = g.play("Nexd5/ca2 {Nd5}").unwrap();
    assert_eq!(r.collapses, vec!["white knight 1 collapsed to d5"]);
    assert!(g.board_text().contains("black bishop: b5 | captured on d5"), "{}", g.board_text());
    play_all(&mut g, &["Kd8", "Rb2/a1", "Ke8"]);
    let r = g.play("Rbxb5/ab1").unwrap();
    assert!(r.collapses.is_empty(), "{:?}", r.collapses);
    assert_eq!(g.state_count(), 2);
    assert!(g.states_fen().iter().all(|f| !f.contains('b')));
}

#[test]
fn quantum_castling_on_both_sides() {
    let mut g = fen_game("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", &["0-0/"]);
    assert_eq!(g.state_count(), 2);
    play_all(&mut g, &["Kd7"]);
    let r = g.play("0-0-0/").unwrap();
    assert!(r.collapses.is_empty());
    assert_eq!(g.state_count(), 2);
    let fens = g.states_fen();
    assert!(fens.iter().any(|f| f.ends_with("2KR1R2")), "{fens:?}");
    assert!(fens.iter().any(|f| f.ends_with("3R1RK1")), "{fens:?}");
}

#[test]
fn quantum_check_is_reported_and_must_be_resolved() {
    let mut g = Game::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 1").unwrap();
    let r = g.play("Qe2/d2+?").unwrap();
    assert_eq!(r.check, Check::Quantum);
    // Kd7 is only legal if the queen is not on d2.
    let r = g.play("Kd7").unwrap();
    assert_eq!(r.collapses, vec!["white queen collapsed to e2"]);
}

#[test]
fn pinned_piece_collapses_the_shield() {
    // Bb4 pins Nd2 unless the c-pawn is on c3.
    let mut g = fen_game("4k3/8/8/8/1b6/8/2PN4/4K3 w - - 0 1", &["c3/4", "Kd8"]);
    let r = g.play("Nf3").unwrap();
    assert_eq!(r.collapses, vec!["white pawn 1 collapsed to c3"]);
}

#[test]
fn quantum_promotion() {
    let g = fen_game("8/4P3/8/8/8/8/8/k6K w - - 0 1", &["e8=Q/N"]);
    assert_eq!(g.state_count(), 2);
    assert!(g.board_text().contains("queen e8 | knight e8"), "{}", g.board_text());
}

#[test]
fn split_piece_merges_when_both_halves_move_to_one_square() {
    let mut g = game(&["Nf3/h3", "e5"]);
    assert_eq!(g.state_count(), 2);
    play_all(&mut g, &["Ng5"]);
    assert_eq!(g.state_count(), 1);
}

#[test]
fn fools_mate() {
    let g = game(&["f3", "e5", "g4", "Qh4#"]);
    assert_eq!(g.status(), Status::Checkmate { winner: Color::Black });
    assert_eq!(g.check(), Check::Full);
}

#[test]
fn ambiguity_and_disambiguation() {
    let mut g = Game::from_fen("4k3/8/8/8/8/8/8/1N2KN2 w - - 0 1").unwrap();
    assert!(g.play("Nd2").unwrap_err().0.contains("ambiguous"));
    g.play("Nbd2").unwrap();
}

#[test]
fn en_passant() {
    let g = game(&["e4", "a6", "e5", "d5", "exd6"]);
    assert!(g.states_fen()[0].starts_with("rnbqkbnr/1pp1pppp/p2P4/8/"), "{:?}", g.states_fen());
}

#[test]
fn illegal_and_garbage() {
    let mut g = Game::new();
    assert!(g.play("e5").is_err());
    assert!(g.play("Nf3/f3").is_err());
    assert!(g.play("hello").is_err());
    assert!(g.play("0-0").is_err());
    assert_eq!(g.history().len(), 0);
}

// ---------------------------------------------------------------------
// UI helpers
// ---------------------------------------------------------------------

fn sorted(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn pieces_start_position() {
    let g = Game::new();
    let ps = g.pieces();
    assert_eq!(ps.len(), 32);
    assert!(ps.iter().all(|p| !p.split && p.halves.len() == 1 && p.halves[0].alive));
    assert_eq!(ps.iter().filter(|p| p.color == Color::White).count(), 16);
    let ids: std::collections::HashSet<usize> = ps.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), 32);
    let king = ps.iter().find(|p| p.color == Color::Black && p.halves[0].kind == Kind::King).unwrap();
    assert_eq!(king.halves[0].square, "e8");
}

#[test]
fn pieces_split_captured_and_gone() {
    let g = game(&["e3/4"]);
    let pawn = g.pieces().into_iter().find(|p| p.split).unwrap();
    assert_eq!(pawn.color, Color::White);
    let mut sq: Vec<&str> = pawn.halves.iter().map(|h| h.square.as_str()).collect();
    sq.sort();
    assert_eq!(sq, ["e3", "e4"]);
    assert!(pawn.halves.iter().all(|h| h.alive && h.kind == Kind::Pawn));

    // Half-captured bishop: still listed, with a dead half.
    let g = fen_game("4k3/8/2b5/8/8/8/8/1R1NK3 w - - 0 1", &["Nc3/e3", "Bb5/d5", "Nexd5/ca2 {Nd5}"]);
    let b = g.pieces().into_iter().find(|p| p.halves[0].kind == Kind::Bishop).unwrap();
    assert!(b.split);
    assert_eq!(b.halves.iter().filter(|h| !h.alive).count(), 1);
    assert_eq!(b.halves.iter().find(|h| !h.alive).unwrap().square, "d5");

    // Fully captured: skipped.
    let g = game(&["e4", "d5", "exd5"]);
    let ps = g.pieces();
    assert_eq!(ps.len(), 31);
    assert_eq!(ps.iter().filter(|p| p.color == Color::Black).count(), 15);
}

#[test]
fn pieces_promotion_halves_have_two_kinds() {
    let g = fen_game("8/4P3/8/8/8/8/8/k6K w - - 0 1", &["e8=Q/N"]);
    let p = g.pieces().into_iter().find(|p| p.split).unwrap();
    let mut kinds: Vec<Kind> = p.halves.iter().map(|h| h.kind).collect();
    kinds.sort();
    assert_eq!(kinds, [Kind::Knight, Kind::Queen]);
    assert!(p.halves.iter().all(|h| h.square == "e8"));
}

#[test]
fn targets_basic() {
    let g = Game::new();
    assert_eq!(g.targets("e2"), sorted(&["e3", "e4"]));
    assert_eq!(g.targets("g1"), sorted(&["f3", "h3"]));
    assert!(g.targets("e7").is_empty(), "enemy piece");
    assert!(g.targets("e4").is_empty(), "empty square");
    assert!(g.targets("z9").is_empty());
    assert!(g.targets("").is_empty());
    assert!(g.targets("e22").is_empty());
}

#[test]
fn targets_for_each_split_half() {
    let g = fen_game("4k3/8/8/8/8/8/8/R3K3 w - - 0 1", &["Ra3/a5", "Kd8"]);
    // Each half is reasoned about in the states where it is the real one,
    // so the other half's square is empty there.
    let t = g.targets("a3");
    assert_eq!(
        t,
        sorted(&["a1", "a2", "a4", "a5", "a6", "a7", "a8", "b3", "c3", "d3", "e3", "f3", "g3", "h3"])
    );
    let t = g.targets("a5");
    assert!(t.contains(&"a1".to_string()) && t.contains(&"h5".to_string()));
    assert!(!t.contains(&"a5".to_string()));
    assert!(g.targets("a1").is_empty(), "no half left on a1");
    // Sorted and unique.
    let mut s = t.clone();
    s.sort();
    s.dedup();
    assert_eq!(s, t);
}

#[test]
fn targets_blocked_by_quantum_piece() {
    let g = fen_game("4k3/8/8/8/8/8/3P4/3RK3 w - - 0 1", &["d3/4", "Kf8"]);
    // The pawn is on d3 or d4: the rook always reaches d2, and d3 when the
    // pawn is on d4.
    assert_eq!(g.targets("d1"), sorted(&["a1", "b1", "c1", "d2", "d3"]));
}

#[test]
fn targets_castling_and_promotion() {
    let g = Game::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
    assert_eq!(g.targets("e1"), sorted(&["c1", "d1", "d2", "e2", "f1", "f2", "g1"]));
    // Only queen-side rights: no g1.
    let g = Game::from_fen("r3k2r/8/8/8/8/8/8/R3K3 w Qkq - 0 1").unwrap();
    let t = g.targets("e1");
    assert!(t.contains(&"c1".to_string()) && !t.contains(&"g1".to_string()), "{t:?}");

    // Promotion square appears once.
    let g = Game::from_fen("1n2k3/P7/8/8/8/8/8/4K3 w - - 0 1").unwrap();
    assert_eq!(g.targets("a7"), sorted(&["a8", "b8"]));
}

#[test]
fn targets_respects_pins_and_check() {
    // Knight is pinned by the bishop in every state.
    let g = Game::from_fen("4k3/8/8/8/1b6/8/3N4/4K3 w - - 0 1").unwrap();
    assert!(g.targets("d2").is_empty());
    // Pinned only in some states: pawn c3/c2 shield.
    let g = fen_game("4k3/8/8/8/1b6/8/2PN4/4K3 w - - 0 1", &["c3/4", "Kd8"]);
    assert!(!g.targets("d2").is_empty());
    // Game over: no targets.
    let g = game(&["f3", "e5", "g4", "Qh4#"]);
    assert!(g.targets("a2").is_empty());
}

#[test]
fn targets_are_playable() {
    // Every target of a knight/bishop/rook/queen half is accepted by play().
    let g = game(&["e3/4", "e5", "Nf3", "Nc6", "Bb5/c4", "Ne7/f6"]);
    let mut tried = 0;
    for p in g.pieces() {
        if p.color != Color::White || matches!(p.halves[0].kind, Kind::Pawn | Kind::King) {
            continue;
        }
        for h in &p.halves {
            for to in g.targets(&h.square) {
                let mut c = g.clone();
                let mv = format!("{}{}{}", h.kind.letter(), h.square, to);
                c.play(&mv).unwrap_or_else(|e| panic!("{mv}: {e}"));
                tried += 1;
            }
        }
    }
    assert!(tried > 10);
}

#[test]
fn replay_and_from_history() {
    let moves = ["e4", "e5", "Nf3"];
    let games = Game::replay(&moves).unwrap();
    assert_eq!(games.len(), 4);
    assert_eq!(games[0].history().len(), 0);
    assert_eq!(games[3].history().len(), 3);
    assert_eq!(games[1].to_move(), Color::Black);
    assert_eq!(games[3].states_fen(), Game::from_history(&moves).unwrap().states_fen());

    // Owned strings and empty input work too.
    let owned: Vec<String> = moves.iter().map(|s| s.to_string()).collect();
    assert_eq!(Game::replay(&owned).unwrap().len(), 4);
    let none: [&str; 0] = [];
    assert_eq!(Game::replay(&none).unwrap().len(), 1);
    assert_eq!(Game::from_history(&none).unwrap().history().len(), 0);

    let e = Game::replay(&["e4", "e4"]).unwrap_err();
    assert!(e.0.contains("move 2"), "{e}");
    assert!(Game::from_history(&["nonsense"]).is_err());
}

#[test]
fn last_move_squares_tracking() {
    let mut g = Game::new();
    assert!(g.last_move_squares().is_empty());
    g.play("e4").unwrap();
    assert_eq!(g.last_move_squares(), ["e2", "e4"]);
    g.play("Nc6").unwrap();
    assert_eq!(g.last_move_squares(), ["b8", "c6"]);

    // Split move: one origin, two destinations.
    let g = game(&["e3/4"]);
    let mut v = g.last_move_squares();
    v.sort();
    assert_eq!(v, ["e2", "e3", "e4"]);

    // Slide split: bishop g... origin plus both destinations.
    let g = game(&["e4", "e5", "Bb5/c4"]);
    let mut v = g.last_move_squares();
    v.sort();
    assert_eq!(v, ["b5", "c4", "f1"]);

    // Castling includes the rook.
    let g = fen_game("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", &["0-0"]);
    let mut v = g.last_move_squares();
    v.sort();
    assert_eq!(v, ["e1", "f1", "g1", "h1"]);

    // Failed moves leave it unchanged.
    let mut g = game(&["e4"]);
    assert!(g.play("e4").is_err());
    assert_eq!(g.last_move_squares(), ["e2", "e4"]);
}

#[test]
fn collapse_options_lists_choices_without_mutating() {
    let g = game(&["Nc3/a3", "d5/6"]);
    let before = g.states_fen();
    let opts = g.collapse_options("Ncxd5/ab5").unwrap();
    assert_eq!(opts.len(), 2, "{opts:?}");
    assert!(opts.contains(&"{Pd6}".to_string()), "{opts:?}");
    assert!(opts.contains(&"{Nd5}".to_string()), "{opts:?}");
    assert_eq!(g.states_fen(), before);
    assert_eq!(g.history().len(), 2);
}

#[test]
fn collapse_options_round_trip_through_play() {
    let g = game(&["Nc3/a3", "d5/6"]);
    for opt in g.collapse_options("Ncxd5/ab5").unwrap() {
        let mut c = g.clone();
        c.play(&format!("Ncxd5/ab5 {opt}")).unwrap_or_else(|e| panic!("{opt}: {e}"));
    }
    // Quantum check scenario: Kd7 must collapse the queen.
    let g = {
        let mut g = Game::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 1").unwrap();
        g.play("Qe2/d2+?").unwrap();
        g
    };
    let opts = g.collapse_options("Kd7").unwrap();
    assert!(!opts.is_empty());
    for opt in opts {
        let mut c = g.clone();
        c.play(&format!("Kd7 {opt}")).unwrap_or_else(|e| panic!("{opt}: {e}"));
    }
    // Pinned shield.
    let g = fen_game("4k3/8/8/8/1b6/8/2PN4/4K3 w - - 0 1", &["c3/4", "Kd8"]);
    let opts = g.collapse_options("Nf3").unwrap();
    assert_eq!(opts, ["{Pc3}"]);
    let mut c = g.clone();
    c.play("Nf3 {Pc3}").unwrap();
}

#[test]
fn collapse_options_empty_or_error() {
    let g = Game::new();
    assert!(g.collapse_options("e4").unwrap().is_empty());
    assert!(g.collapse_options("e5").is_err());
    assert!(g.collapse_options("hello").is_err());
    let g = game(&["f3", "e5", "g4", "Qh4#"]);
    assert!(g.collapse_options("a3").is_err());
    // Existing {..} in the notation is ignored.
    let g = game(&["Nc3/a3", "d5/6"]);
    assert_eq!(g.collapse_options("Ncxd5/ab5 {Pd6}").unwrap().len(), 2);
}

#[test]
fn collapse_options_multi_piece_round_trip() {
    // Moving through two quantum pieces needs a combined spec.
    let g = game(&["d3/4", "a6", "e3/4", "a5"]);
    let opts = g.collapse_options("Qd3").unwrap();
    assert!(!opts.is_empty());
    for opt in opts {
        let mut c = g.clone();
        c.play(&format!("Qd3 {opt}")).unwrap_or_else(|e| panic!("{opt}: {e}"));
    }
}

#[test]
fn game_is_send_sync_clone() {
    fn is<T: Clone + Send + Sync>() {}
    is::<Game>();
    is::<PieceView>();
    is::<HalfView>();
}

#[test]
fn random_play_keeps_helpers_consistent() {
    // Deterministic pseudo-random games: targets are never rejected as
    // illegal, and every collapse option round-trips through play().
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let (mut played, mut with_collapse) = (0, 0);
    for _ in 0..40 {
        let mut g = Game::new();
        for _ply in 0..40 {
            if g.status() != Status::Ongoing {
                break;
            }
            let me = g.to_move();
            let mut cands = Vec::new();
            for p in g.pieces().iter().filter(|p| p.color == me) {
                for h in p.halves.iter().filter(|h| h.alive) {
                    for to in g.targets(&h.square) {
                        let letter = if h.kind == Kind::Pawn { String::new() } else { h.kind.letter().to_string() };
                        cands.push(format!("{letter}{}{to}", h.square));
                    }
                }
            }
            if cands.is_empty() {
                break;
            }
            let mut mv = cands[next(cands.len())].clone();
            if g.state_count() < 16 && next(4) == 0 {
                // A split: a second destination for the same half.
                let from = &mv[mv.len() - 4..mv.len() - 2];
                let others: Vec<String> = g.targets(from).into_iter().filter(|t| !mv.ends_with(t.as_str())).collect();
                if !others.is_empty() {
                    mv = format!("{mv}/{}", others[next(others.len())]);
                }
            }
            let before = g.states_fen();
            let Ok(opts) = g.collapse_options(&mv) else {
                // Ambiguous or otherwise unplayable spelling: play must agree.
                assert!(g.clone().play(&mv).is_err(), "{mv}");
                continue;
            };
            assert_eq!(g.states_fen(), before);
            for o in &opts {
                let mut c = g.clone();
                c.play(&format!("{mv} {o}")).unwrap_or_else(|e| panic!("{mv} {o}: {e}\n{}", g.board_text()));
            }
            with_collapse += usize::from(!opts.is_empty());
            g.play(&mv).unwrap();
            played += 1;
            assert!(!g.last_move_squares().is_empty());
        }
    }
    assert!(played > 200 && with_collapse > 0, "{played} {with_collapse}");
}

#[test]
fn pieces_are_numbered_from_the_a_file() {
    let g = Game::new();
    let label = |sq: &str| {
        let p = g.pieces().into_iter().find(|p| p.halves[0].square == sq).unwrap();
        (p.origin, p.num)
    };
    assert_eq!(label("a2"), (Kind::Pawn, Some(1)));
    assert_eq!(label("h7"), (Kind::Pawn, Some(8)));
    assert_eq!(label("a1"), (Kind::Rook, Some(1)));
    assert_eq!(label("h8"), (Kind::Rook, Some(2)));
    assert_eq!(label("b1"), (Kind::Knight, Some(1)));
    assert_eq!(label("g8"), (Kind::Knight, Some(2)));
    assert_eq!(label("c1"), (Kind::Bishop, None));
    assert_eq!(label("e1"), (Kind::King, None));
}

#[test]
fn display_notation_names_the_numbered_piece() {
    let mut g = Game::new();
    let shown: Vec<String> = ["e3/4", "e5", "Nf3", "Nc6", "Bb5/c4", "Ne7/f6", "0-0/", "Ra8b8", "a3"]
        .iter()
        .map(|m| g.play(m).unwrap().display)
        .collect();
    assert_eq!(shown, ["P₅e3/e4", "P₅e5", "N₂f3", "N₁c6", "Bb5/c4", "N₂e7/f6", "0-0/", "R₁b8", "P₁a3"]);
    assert_eq!(g.display_history(), shown.as_slice());
    // Numbers follow the piece, not the square it stands on.
    let rook = g.pieces().into_iter().find(|p| p.halves.iter().any(|h| h.square == "f1")).unwrap();
    assert_eq!((rook.origin, rook.num), (Kind::Rook, Some(2)));
}

#[test]
fn promoted_pawn_keeps_its_number() {
    let mut g = Game::from_fen("8/4P3/8/8/8/8/k7/4K3 w - - 0 1").unwrap();
    let r = g.play("e8=Q").unwrap();
    assert_eq!(r.display, "P₁e8=Q");
    g.play("Ka3").unwrap();
    assert_eq!(g.play("Qe7").unwrap().display, "Qₚ₁e7");
    let q = g.pieces().into_iter().find(|p| p.halves[0].square == "e7").unwrap();
    assert_eq!((q.origin, q.num, q.halves[0].kind), (Kind::Pawn, Some(1), Kind::Queen));
}

#[test]
fn moving_a_split_piece_whole_reports_its_collapse() {
    // Only the c3 half can reach d5, so the knight collapses to c3 first.
    let mut g = game(&["Nc3/a3", "d5/6"]);
    let r = g.play("Nc3d5").unwrap();
    assert_eq!(r.collapses, vec!["white knight 1 collapsed to c3"]);
    // Both halves can reach b5 (from c3 and a3): no collapse.
    let mut g = game(&["Nc3/a3", "e5"]);
    let r = g.play("Nb5").unwrap();
    assert!(r.collapses.is_empty(), "{:?}", r.collapses);
}

#[test]
fn collapse_options_name_the_piece_when_two_share_a_square() {
    // The g1 knight splits onto a3 and d2; the b1 knight then splits onto
    // the same two squares.
    let h = ["d2d4", "a7a6", "Ng1f3", "a6a5", "Nf3e5", "h7h6", "Ne5c4", "h6h5", "Nc4a3/d2", "g7g6"];
    let g = Game::from_history(&h).unwrap();
    let opts = g.collapse_options("Nb1a3/d2").unwrap();
    assert!(opts.len() > 1, "{opts:?}");
    assert!(opts.iter().any(|o| o.contains("N1") || o.contains("N2")), "{opts:?}");
    for o in &opts {
        let mut t = g.clone();
        t.play(&format!("Nb1a3/d2 {o}")).unwrap_or_else(|e| panic!("{o}: {e}"));
    }
}

#[test]
fn en_passant_after_a_genuine_double_push() {
    // A genuine double push allows en passant.
    let mut g = Game::from_history(&["e2e4", "a7a6", "e4e5", "d7d5"]).unwrap();
    assert!(g.targets("e5").contains(&"d6".to_string()));
    g.play("e5d6").unwrap();
}

#[test]
fn pawn_moving_forward_onto_a_quantum_piece_collapses_it() {
    // Rule sheet, "Moving Through Quantum Pieces": "Pawns can also move
    // forward onto quantum pieces in this way." The black knight is split
    // e5 | d4; pushing the e4 pawn to e5 forces the knight onto d4.
    let mut g = game(&["e4", "Nc6", "a3", "Nc6e5/d4"]);
    assert_eq!(g.state_count(), 2);
    let r = g.play("e5").unwrap();
    assert_eq!(r.collapses, vec!["black knight 1 collapsed to d4"]);
    assert_eq!(g.state_count(), 1);
    assert!(g.states_fen()[0].contains("4P3"), "{:?}", g.states_fen());
}

#[test]
fn quantum_promotion_halves_stay_distinct_until_forced() {
    // Rule sheet, "Quantum Promotion": the two promoted pieces "can remain
    // distinct until some move forces them to collapse".
    let mut g = fen_game("8/4P3/8/8/8/8/8/k6K w - - 0 1", &["e8=Q/N", "Ka2"]);
    assert_eq!(g.state_count(), 2);
    // A move only the queen can make collapses the piece to the queen.
    let r = g.play("Qe8a4").unwrap();
    assert_eq!(r.collapses, vec!["white queen (pawn 1) collapsed to a4"]);
    let p = g.pieces().into_iter().find(|p| p.halves.iter().any(|h| h.square == "a4")).unwrap();
    assert!(!p.split);
    assert_eq!(p.halves[0].kind, Kind::Queen);
}

#[test]
fn ending_your_turn_in_quantum_check_collapses_the_checker() {
    // White queen is half on e2 (checking down the e-file) and half on d2.
    let g = fen_game("4k3/p7/8/8/8/8/8/3QK3 w - - 0 1", &["Qe2/d2+?"]);
    assert_eq!(g.check(), Check::Quantum);
    // Black ignores the check: the queen must collapse away from e2.
    let mut c = g.clone();
    let r = c.play("a6").unwrap();
    assert_eq!(r.collapses, vec!["white queen collapsed to d2"]);
    assert_eq!(c.state_count(), 1);
    // Moving the king works too: Kd8 escapes the e2 half, but walks into
    // the d2 half, so this time the queen collapses to e2.
    let mut c = g.clone();
    let r = c.play("Kf8").unwrap();
    assert!(r.collapses.is_empty(), "f8 is safe from both halves");
    let mut c = g.clone();
    let r = c.play("Kd8").unwrap();
    assert_eq!(r.collapses, vec!["white queen collapsed to e2"]);
    // Check in every board can't be ignored.
    let g = fen_game("4k3/p7/8/8/8/8/8/4QK2 w - - 0 1", &["Qe2/e3"]);
    assert_eq!(g.check(), Check::Full);
    assert!(g.clone().play("a6").is_err());
}

#[test]
fn a_piece_never_shares_a_square_on_any_board() {
    // White pawn split e3 | e4. A knight landing on e3 proves the pawn was
    // on e4; a knight splitting onto e3 needs a collapse, because the board
    // with both on e3 can't exist and the rest isn't a product of halves.
    let f = "4k3/8/8/8/8/8/4P1N1/4K3 w - - 0 1";
    let mut g = fen_game(f, &["e3/4", "Kd7"]);
    let r = g.play("Ng2e3").unwrap();
    assert_eq!(r.collapses, vec!["white pawn 1 collapsed to e4"]);
    let g = fen_game(f, &["e3/4", "Kd7"]);
    let mut opts = g.collapse_options("Ng2e3/h4").unwrap();
    opts.sort();
    assert_eq!(opts, ["{Nh4}", "{Pe4}"]);
    for o in opts {
        let mut t = g.clone();
        t.play(&format!("Ng2e3/h4 {o}")).unwrap();
        for b in t.states_fen() {
            let row3 = b.split('/').nth(5).unwrap();
            assert!(row3.matches(['N', 'P']).count() <= 1, "two pieces on one square: {b}");
        }
    }
    // An enemy half: a split knight taking half a pawn is the rule sheet's
    // "capturing using quantum pieces": either the knight collapses onto the
    // capture square or the pawn onto its other square.
    let mut g = Game::from_fen("4k3/4p3/8/8/6N1/8/8/4K3 b - - 0 1").unwrap();
    play_all(&mut g, &["e6/5"]);
    let mut opts = g.collapse_options("Ng4xe5/f6").unwrap();
    opts.sort();
    assert_eq!(opts, ["{Ne5}", "{Pe6}"]);
}

#[test]
fn every_possible_board_has_a_real_chess_history() {
    // The defining rule: collapse every superposition any way the game still
    // allows, and the moves played must form an ordinary chess game. Checked
    // move by move against an independent chess library (shakmaty): every
    // board after a move must come from a board before it by a legal move of
    // the piece that moved. By induction, every board's whole history is a
    // legal chess game. Random quantum games (splits, two-half moves,
    // collapse choices, castling, promotions) plus a few AI games.
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut byte = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 24) as u8
    };
    // Debug builds are much slower; release (`cargo test --release`) runs more.
    let games = if cfg!(debug_assertions) { 60 } else { 300 };
    for _ in 0..games {
        let data: Vec<u8> = (0..400).map(|_| byte()).collect();
        crate::fuzzing::run(&data, 60);
    }
    for s in 0..3 {
        crate::fuzzing::self_play(s, 60, 1, 60);
    }
}
