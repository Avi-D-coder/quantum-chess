use super::*;
use glicko2::Rating;

const W: i64 = 1;
const B: i64 = 2;
const SPEC: i64 = 99;

fn pi(id: i64, name: &str, r: Option<f64>) -> PlayerInfo {
    PlayerInfo { user_id: id, username: name.into(), rating: r.map(|r| Rating::new(r, 80.0, 0.06)) }
}

fn room_tc(tc: TimeControl, rated: bool) -> Room {
    Room::new("g1", pi(W, "avi", Some(1500.0)), pi(B, "bob", Some(1500.0)), tc, rated, 0)
}

fn room() -> Room {
    room_tc(TimeControl::new(3, 2), true)
}

fn ended(ev: &[Event]) -> Option<(GameResult, Termination)> {
    ev.iter().find_map(|e| match e {
        Event::Ended { result, termination } => Some((*result, *termination)),
        _ => None,
    })
}

fn moved(ev: &[Event]) -> &MoveRecord {
    ev.iter()
        .find_map(|e| if let Event::Moved(m) = e { Some(m) } else { None })
        .expect("a Moved event")
}

fn fools_mate(r: &mut Room, t: u64) -> Vec<Event> {
    r.play(W, "f2f3", t).unwrap();
    r.play(B, "e7e5", t + 1).unwrap();
    r.play(W, "g2g4", t + 2).unwrap();
    r.play(B, "Qd8h4", t + 3).unwrap()
}

#[test]
fn rule_sheet_opening_in_browser_format() {
    let mut r = room();
    let seq = [
        (W, "e2e3/4"),
        (B, "e7e5"),
        (W, "Ng1f3"),
        (B, "Nb8c6"),
        (W, "Bf1b5/c4"),
        (B, "Ng8e7/f6"),
        (W, "0-0/"),
    ];
    for (i, (u, m)) in seq.iter().enumerate() {
        let ev = r.play(*u, m, 1000 * (i as u64 + 1)).unwrap();
        assert_eq!(moved(&ev).ply, i + 1);
        assert_eq!(moved(&ev).notation, *m);
        assert_eq!(ended(&ev), None, "{m}");
    }
    assert_eq!(r.game().state_count(), 16);
    assert_eq!(r.plies(), 7);
    assert_eq!(r.game().history().len(), 7);
    assert_eq!(r.status(), RoomStatus::Ongoing);
    let shown: Vec<&str> = r.moves().iter().map(|m| m.display.as_str()).collect();
    assert_eq!(shown, ["P₅e3/e4", "P₅e5", "N₂f3", "N₁c6", "Bb5/c4", "N₂e7/f6", "0-0/"]);
}

#[test]
fn other_notation_forms_accepted_by_engine() {
    // forms the browser UI may generate; all accepted by the engine
    let mut r = room();
    for (u, m) in [(W, "Nb1c3/a3"), (B, "d7d5/6"), (W, "e2e4"), (B, "Ng8f6")] {
        r.play(u, m, 10).unwrap_or_else(|e| panic!("{m}: {e}"));
    }
}

#[test]
fn clock_free_first_moves_then_increment() {
    let mut r = room(); // 3+2
    // White thinks for 50s before the first move: free.
    let ev = r.play(W, "e2e4", 50_000).unwrap();
    assert_eq!(moved(&ev).white_ms, 180_000);
    assert_eq!(r.clock().running(), None);
    assert_eq!(r.next_deadline_ms(60_000), None);
    // Black too.
    let ev = r.play(B, "e7e5", 100_000).unwrap();
    assert_eq!((moved(&ev).white_ms, moved(&ev).black_ms), (180_000, 180_000));
    assert_eq!(r.clock().running(), Some(Color::White));
    // White's clock runs from black's first move.
    assert_eq!(r.clock().remaining(Color::White, 103_000), 177_000);
    let ev = r.play(W, "Ng1f3", 103_000).unwrap();
    assert_eq!(moved(&ev).white_ms, 177_000 + 2000);
    assert_eq!(moved(&ev).at_ms, 103_000);
    assert_eq!(r.clock().running(), Some(Color::Black));
    // While black thinks, black's time ticks, white's is frozen.
    assert_eq!(r.clock().remaining(Color::Black, 110_000), 180_000 - 7000);
    assert_eq!(r.clock().remaining(Color::White, 110_000), 179_000);
    let ev = r.play(B, "Nb8c6", 110_000).unwrap();
    assert_eq!(moved(&ev).black_ms, 180_000 - 7000 + 2000);
}

#[test]
fn flag_via_tick() {
    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.play(W, "e2e4", 0).unwrap();
    r.play(B, "e7e5", 1000).unwrap(); // white's clock starts at 1000
    assert_eq!(r.next_deadline_ms(2000), Some(61_000));
    assert_eq!(r.next_deadline_ms(70_000), Some(70_000));
    assert!(r.tick(60_999).is_empty());
    assert_eq!(r.status(), RoomStatus::Ongoing);
    let ev = r.tick(61_000);
    assert_eq!(ended(&ev), Some((GameResult::BlackWins, Termination::Timeout)));
    assert!(r.is_over());
    assert_eq!(r.clock().remaining(Color::White, 99_999), 0);
    assert_eq!(r.next_deadline_ms(61_000), None);
    assert!(r.tick(70_000).is_empty());
}

#[test]
fn black_flag_via_tick() {
    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.play(W, "e2e4", 0).unwrap();
    r.play(B, "e7e5", 0).unwrap();
    r.play(W, "Ng1f3", 500).unwrap();
    assert_eq!(r.next_deadline_ms(500), Some(60_500));
    let ev = r.tick(60_500);
    assert_eq!(ended(&ev), Some((GameResult::WhiteWins, Termination::Timeout)));
}

#[test]
fn flag_via_late_move() {
    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.play(W, "e2e4", 0).unwrap();
    r.play(B, "e7e5", 0).unwrap();
    let ev = r.play(W, "Ng1f3", 61_000).unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ended(&ev), Some((GameResult::BlackWins, Termination::Timeout)));
    assert_eq!(r.plies(), 2, "late move is not played");
    assert_eq!(r.game().history().len(), 2);
}

#[test]
fn late_move_by_black_loses_for_black() {
    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.play(W, "e2e4", 0).unwrap();
    r.play(B, "e7e5", 0).unwrap();
    r.play(W, "Ng1f3", 1000).unwrap();
    let ev = r.play(B, "Nb8c6", 1000 + 60_000).unwrap();
    assert_eq!(ended(&ev), Some((GameResult::WhiteWins, Termination::Timeout)));
}

#[test]
fn timeout_vs_lone_king_is_draw() {
    // white flags, black has only a king -> draw
    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.game = Game::from_fen("4k3/8/8/8/8/8/8/R3K3 w - - 0 1").unwrap();
    r.clock.start(Color::White, 0);
    r.moves = vec![
        MoveRecord { ply: 1, notation: "x".into(), display: "x".into(), collapses: vec![], white_ms: 0, black_ms: 0, at_ms: 0 };
        2
    ];
    let ev = r.tick(60_000);
    assert_eq!(ended(&ev), Some((GameResult::Draw, Termination::Timeout)));
    assert_eq!(r.rating_updates().map(|(w, b)| w.rating == b.rating), Some(true));
}

#[test]
fn timeout_with_material_is_a_win() {
    // white flags but black has a rook -> black wins
    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.game = Game::from_fen("r3k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
    r.clock.start(Color::White, 0);
    r.moves = vec![
        MoveRecord { ply: 1, notation: "x".into(), display: "x".into(), collapses: vec![], white_ms: 0, black_ms: 0, at_ms: 0 };
        2
    ];
    assert!(!r.has_only_king(Color::Black));
    assert!(r.has_only_king(Color::White));
    let ev = r.tick(60_000);
    assert_eq!(ended(&ev), Some((GameResult::BlackWins, Termination::Timeout)));
}

#[test]
fn lone_king_rule_for_black_flag_and_pawn_counts() {
    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.game = Game::from_fen("4k3/8/8/8/8/8/4P3/4K3 b - - 0 1").unwrap();
    r.clock.start(Color::Black, 0);
    r.moves = vec![
        MoveRecord { ply: 1, notation: "x".into(), display: "x".into(), collapses: vec![], white_ms: 0, black_ms: 0, at_ms: 0 };
        2
    ];
    // white has a pawn: sufficient material
    assert_eq!(ended(&r.tick(60_000)), Some((GameResult::WhiteWins, Termination::Timeout)));

    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.game = Game::from_fen("4k3/8/8/8/8/8/8/4K3 b - - 0 1").unwrap();
    r.clock.start(Color::Black, 0);
    r.moves = vec![
        MoveRecord { ply: 1, notation: "x".into(), display: "x".into(), collapses: vec![], white_ms: 0, black_ms: 0, at_ms: 0 };
        2
    ];
    assert_eq!(ended(&r.tick(60_000)), Some((GameResult::Draw, Termination::Timeout)));
}

#[test]
fn has_only_king_in_start_position_is_false() {
    let r = room();
    assert!(!r.has_only_king(Color::White));
    let mut r = room();
    r.game = Game::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
    assert!(r.has_only_king(Color::White) && r.has_only_king(Color::Black));
}

#[test]
fn collapse_choice_then_play() {
    let mut r = room();
    r.play(W, "Nb1c3/a3", 1).unwrap();
    r.play(B, "d7d5/6", 2).unwrap();
    let ev = r.play(W, "Nc3d5/a3b5", 3).unwrap();
    assert_eq!(ev.len(), 1);
    let Event::NeedsCollapseChoice { notation, options } = &ev[0] else { panic!("{ev:?}") };
    assert_eq!(notation, "Nc3d5/a3b5");
    assert_eq!(options.len(), 2);
    assert!(options.contains(&"{Pd6}".to_string()) && options.contains(&"{Nd5}".to_string()));
    // nothing changed
    assert_eq!(r.plies(), 2);
    assert_eq!(r.game().to_move(), Color::White);

    let full = format!("{notation} {}", options[0]);
    let ev = r.play(W, &full, 4).unwrap();
    assert_eq!(moved(&ev).notation, full);
    assert_eq!(moved(&ev).ply, 3);
    assert!(!moved(&ev).collapses.is_empty());
    assert_eq!(r.plies(), 3);
}

#[test]
fn collapse_choice_second_option_also_plays() {
    let mut r = room();
    r.play(W, "Nb1c3/a3", 1).unwrap();
    r.play(B, "d7d5/6", 2).unwrap();
    let ev = r.play(W, "Nc3d5/a3b5", 3).unwrap();
    let Event::NeedsCollapseChoice { options, .. } = &ev[0] else { panic!() };
    r.play(W, &format!("Nc3d5/a3b5 {}", options[1]), 4).unwrap();
    assert_eq!(r.plies(), 3);
}

#[test]
fn bad_collapse_choice_is_illegal() {
    let mut r = room();
    r.play(W, "Nb1c3/a3", 1).unwrap();
    r.play(B, "d7d5/6", 2).unwrap();
    let e = r.play(W, "Nc3d5/a3b5 {Qd1}", 3).unwrap_err();
    assert!(matches!(e, RoomError::IllegalMove(_)), "{e:?}");
    assert_eq!(r.plies(), 2);
}

#[test]
fn illegal_and_garbage_moves() {
    let mut r = room();
    assert!(matches!(r.play(W, "e2e5", 1), Err(RoomError::IllegalMove(_))));
    assert!(matches!(r.play(W, "zzz", 1), Err(RoomError::IllegalMove(_))));
    assert!(matches!(r.play(W, "", 1), Err(RoomError::IllegalMove(_))));
    assert_eq!(r.plies(), 0);
    assert!(RoomError::IllegalMove("x".into()).to_string().contains("illegal move"));
}

#[test]
fn turn_and_player_checks() {
    let mut r = room();
    assert_eq!(r.play(B, "e7e5", 1), Err(RoomError::NotYourTurn));
    assert_eq!(r.play(SPEC, "e2e4", 1), Err(RoomError::NotAPlayer));
    r.play(W, "e2e4", 1).unwrap();
    assert_eq!(r.play(W, "d2d4", 2), Err(RoomError::NotYourTurn));
    assert_eq!(r.play(SPEC, "e7e5", 2), Err(RoomError::NotAPlayer));
    assert_eq!(r.resign(SPEC, 3), Err(RoomError::NotAPlayer));
    assert_eq!(r.abort(SPEC, 3), Err(RoomError::NotAPlayer));
    assert_eq!(r.offer_draw(SPEC), Err(RoomError::NotAPlayer));
    assert_eq!(r.accept_draw(SPEC, 3), Err(RoomError::NotAPlayer));
    assert_eq!(r.decline_draw(SPEC), Err(RoomError::NotAPlayer));
    assert_eq!(r.color_of(W), Some(Color::White));
    assert_eq!(r.color_of(SPEC), None);
}

#[test]
fn resign() {
    let mut r = room();
    r.play(W, "e2e4", 1).unwrap();
    let ev = r.resign(W, 5).unwrap();
    assert_eq!(ended(&ev), Some((GameResult::BlackWins, Termination::Resign)));
    assert!(r.is_over());
    assert_eq!(r.resign(B, 6), Err(RoomError::GameOver));
    assert_eq!(r.play(B, "e7e5", 6), Err(RoomError::GameOver));
    assert_eq!(r.offer_draw(B), Err(RoomError::GameOver));

    // out-of-turn resignation is fine
    let mut r = room();
    let ev = r.resign(B, 1).unwrap();
    assert_eq!(ended(&ev), Some((GameResult::WhiteWins, Termination::Resign)));
}

#[test]
fn abort_window() {
    let mut r = room();
    let ev = r.abort(B, 1).unwrap();
    assert_eq!(ended(&ev), Some((GameResult::Draw, Termination::Abort)));
    assert_eq!(r.abort(W, 2), Err(RoomError::GameOver));

    let mut r = room();
    r.play(W, "e2e4", 1).unwrap();
    assert!(r.abort(W, 2).is_ok(), "one ply: still abortable");

    let mut r = room();
    r.play(W, "e2e4", 1).unwrap();
    r.play(B, "e7e5", 2).unwrap();
    assert_eq!(r.abort(W, 3), Err(RoomError::TooLateToAbort));
    assert!(!r.is_over());
}

#[test]
fn draw_offer_accept() {
    let mut r = room();
    r.play(W, "e2e4", 1).unwrap();
    assert_eq!(r.offer_draw(B).unwrap(), vec![Event::DrawOffered(Color::Black)]);
    assert_eq!(r.draw_offer(), Some(Color::Black));
    assert_eq!(r.offer_draw(B), Err(RoomError::DrawAlreadyOffered));
    assert_eq!(r.offer_draw(W), Err(RoomError::DrawAlreadyOffered));
    assert_eq!(r.accept_draw(B, 2), Err(RoomError::NoDrawOffer), "cannot accept own offer");
    let ev = r.accept_draw(W, 3).unwrap();
    assert_eq!(ended(&ev), Some((GameResult::Draw, Termination::DrawAgreement)));
    assert_eq!(r.draw_offer(), None);
}

#[test]
fn draw_offer_decline() {
    let mut r = room();
    assert_eq!(r.decline_draw(B), Err(RoomError::NoDrawOffer));
    assert_eq!(r.accept_draw(B, 1), Err(RoomError::NoDrawOffer));
    r.offer_draw(W).unwrap();
    assert_eq!(r.decline_draw(W), Err(RoomError::NoDrawOffer));
    assert_eq!(r.decline_draw(B).unwrap(), vec![Event::DrawDeclined]);
    assert_eq!(r.draw_offer(), None);
    // may offer again afterwards
    assert!(r.offer_draw(W).is_ok());
}

#[test]
fn a_move_clears_draw_offer() {
    let mut r = room();
    r.play(W, "e2e4", 1).unwrap();
    r.offer_draw(W).unwrap();
    let ev = r.play(B, "e7e5", 2).unwrap();
    assert_eq!(ev[0], Event::DrawDeclined);
    assert!(matches!(ev[1], Event::Moved(_)));
    assert_eq!(r.draw_offer(), None);
    assert_eq!(r.accept_draw(W, 3), Err(RoomError::NoDrawOffer));
}

#[test]
fn accept_draw_after_flag_is_timeout() {
    let mut r = room_tc(TimeControl::new(1, 0), true);
    r.play(W, "e2e4", 0).unwrap();
    r.play(B, "e7e5", 0).unwrap();
    r.play(W, "Ng1f3", 10).unwrap();
    r.offer_draw(W).unwrap();
    let ev = r.accept_draw(B, 61_000).unwrap();
    assert_eq!(ended(&ev), Some((GameResult::WhiteWins, Termination::Timeout)));
}

#[test]
fn chat_rules() {
    let mut r = room();
    assert_eq!(r.chat(W, "x", "   ", 1), Err(RoomError::ChatEmpty));
    assert_eq!(r.chat(W, "x", "", 1), Err(RoomError::ChatEmpty));
    let long = "a".repeat(141);
    assert_eq!(r.chat(W, "x", &long, 1), Err(RoomError::ChatTooLong));
    let ok = "a".repeat(140);
    assert!(r.chat(W, "x", &ok, 1).is_ok());
    // multi-byte characters count as one each
    assert!(r.chat(W, "x", &"é".repeat(140), 1).is_ok());
    assert_eq!(r.chat(W, "x", &"é".repeat(141), 1), Err(RoomError::ChatTooLong));
    // trimmed; spectators allowed; players use room username
    let ev = r.chat(W, "ignored", "  gg  ", 42).unwrap();
    assert_eq!(
        ev,
        vec![Event::Chat(ChatLine { user_id: W, username: "avi".into(), text: "gg".into(), ts: 42 })]
    );
    let ev = r.chat(SPEC, "carol", "hi", 43).unwrap();
    assert_eq!(
        ev,
        vec![Event::Chat(ChatLine { user_id: SPEC, username: "carol".into(), text: "hi".into(), ts: 43 })]
    );
    assert_eq!(r.chat_log().len(), 4);
}

#[test]
fn chat_cap_200_and_after_game_over() {
    let mut r = room();
    for i in 0..205 {
        r.chat(W, "avi", &format!("m{i}"), i).unwrap();
    }
    assert_eq!(r.chat_log().len(), CHAT_CAP);
    assert_eq!(r.chat_log()[0].text, "m5");
    assert_eq!(r.chat_log().last().unwrap().text, "m204");
    r.resign(W, 1).unwrap();
    assert!(r.chat(B, "bob", "gg", 2).is_ok());
}

#[test]
fn fools_mate_checkmate_and_ratings() {
    let mut r = room();
    let ev = fools_mate(&mut r, 1000);
    assert_eq!(moved(&ev).ply, 4);
    assert_eq!(ended(&ev), Some((GameResult::BlackWins, Termination::Checkmate)));
    assert_eq!(r.status(), RoomStatus::Ended { result: GameResult::BlackWins, termination: Termination::Checkmate });
    assert_eq!(r.clock().running(), None);
    let (w, b) = r.rating_updates().unwrap();
    assert!(w.rating < 1500.0, "loser down: {w:?}");
    assert!(b.rating > 1500.0, "winner up: {b:?}");
    assert!(w.deviation < 80.0 && b.deviation < 80.0);
}

#[test]
fn rating_updates_conditions() {
    // ongoing
    let mut r = room();
    assert!(r.rating_updates().is_none());
    // aborted
    r.abort(W, 1).unwrap();
    assert!(r.rating_updates().is_none());
    // casual
    let mut r = room_tc(TimeControl::new(3, 2), false);
    r.resign(W, 1).unwrap();
    assert!(r.rating_updates().is_none());
    // rated resignation by black; unrated players use the default rating
    let mut r = Room::new("g", pi(W, "a", None), pi(B, "b", None), TimeControl::new(5, 0), true, 0);
    r.resign(B, 1).unwrap();
    let (w, b) = r.rating_updates().unwrap();
    assert!(w.rating > 1500.0 && b.rating < 1500.0);
    assert!(w.deviation < 350.0);
}

#[test]
fn rating_updates_draw_favours_lower_rated() {
    let mut r = Room::new("g", pi(W, "a", Some(1800.0)), pi(B, "b", Some(1400.0)), TimeControl::new(5, 0), true, 0);
    r.play(W, "e2e4", 1).unwrap();
    r.offer_draw(W).unwrap();
    r.accept_draw(B, 2).unwrap();
    let (w, b) = r.rating_updates().unwrap();
    assert!(w.rating < 1800.0);
    assert!(b.rating > 1400.0);
}

#[test]
fn snapshot_initial() {
    let r = room();
    let s = r.snapshot(Some(W), 5000);
    assert_eq!(s.id, "g1");
    assert_eq!(s.you, Some(Color::White));
    assert_eq!(s.you_str(), Some("white"));
    assert_eq!(s.white, PlayerView { username: "avi".into(), rating: 1500, provisional: false });
    assert_eq!(s.black.username, "bob");
    assert_eq!(
        s.clock,
        ClockView {
            initial_ms: 180_000,
            increment_ms: 2000,
            white_ms: 180_000,
            black_ms: 180_000,
            running: None,
            server_now_ms: 5000
        }
    );
    assert_eq!(s.to_move, Color::White);
    assert_eq!(s.status, StatusView::Ongoing);
    assert_eq!(s.status.kind(), "ongoing");
    assert_eq!(s.check_str(), "none");
    assert_eq!(s.state_count, 1);
    assert_eq!(s.pieces.len(), 32);
    assert!(s.moves.is_empty() && s.last_move.is_empty());
    assert_eq!(s.draw_offer, None);
    assert!(s.rated);
    let t: std::collections::HashMap<_, _> = s.targets.iter().cloned().collect();
    assert_eq!(t["g1"], ["f3", "h3"]);
    assert_eq!(t["b1"], ["a3", "c3"]);
    assert!(t["e2"].contains(&"e4".to_string()));
    assert_eq!(s.targets.len(), 10, "8 pawns + 2 knights");
    assert!(s.targets.windows(2).all(|w| w[0].0 < w[1].0), "sorted");

    assert_eq!(r.snapshot(Some(B), 0).you, Some(Color::Black));
    assert_eq!(r.snapshot(Some(SPEC), 0).you, None);
    assert_eq!(r.snapshot(None, 0).you, None);
    assert_eq!(r.snapshot(None, 0).you_str(), None);
}

#[test]
fn snapshot_midgame_and_provisional() {
    let mut r = Room::new("g", pi(W, "a", None), pi(B, "b", Some(1612.4)), TimeControl::new(3, 2), false, 0);
    r.play(W, "e2e3/4", 1000).unwrap();
    r.play(B, "e7e5", 2000).unwrap();
    r.offer_draw(W).unwrap();
    let s = r.snapshot(None, 4000);
    assert_eq!(s.white.rating, 1500);
    assert!(s.white.provisional);
    assert_eq!(s.black.rating, 1612);
    assert_eq!(s.to_move, Color::White);
    assert_eq!(s.clock.running, Some(Color::White));
    assert_eq!(s.clock.white_ms, 178_000);
    assert_eq!(s.clock.black_ms, 180_000);
    assert_eq!(s.clock.server_now_ms, 4000);
    assert_eq!(s.state_count, 2);
    assert_eq!(s.moves.len(), 2);
    assert_eq!(s.moves[0].notation, "e2e3/4");
    assert_eq!(s.draw_offer, Some(Color::White));
    assert!(!s.rated);
    assert!(s.last_move.contains(&"e7".to_string()) && s.last_move.contains(&"e5".to_string()));
    assert!(s.pieces.iter().any(|p| p.split && p.color == Color::White));
    // targets are for white (side to move) only
    let from: Vec<_> = s.targets.iter().map(|t| t.0.as_str()).collect();
    assert!(from.contains(&"g1") && from.contains(&"d2"));
    assert!(!from.contains(&"e5") && !from.contains(&"e7") && !from.contains(&"b8"));
}

#[test]
fn snapshot_after_end() {
    let mut r = room();
    fools_mate(&mut r, 100);
    let s = r.snapshot(Some(W), 9999);
    assert_eq!(s.status, StatusView::Ended { result: "0-1", termination: "checkmate" });
    assert_eq!(s.status.kind(), "ended");
    assert!(s.targets.is_empty());
    assert_eq!(s.check_str(), "full");
    assert_eq!(s.moves.len(), 4);
    assert_eq!(s.clock.running, None);

    let mut r = room();
    r.play(W, "e2e4", 1).unwrap();
    r.resign(B, 2).unwrap();
    assert_eq!(r.snapshot(None, 3).status, StatusView::Ended { result: "1-0", termination: "resign" });
}

#[test]
fn protocol_strings() {
    assert_eq!(GameResult::WhiteWins.as_str(), "1-0");
    assert_eq!(GameResult::BlackWins.as_str(), "0-1");
    assert_eq!(GameResult::Draw.as_str(), "1/2-1/2");
    let t = [
        (Termination::Checkmate, "checkmate"),
        (Termination::Stalemate, "stalemate"),
        (Termination::Resign, "resign"),
        (Termination::Timeout, "timeout"),
        (Termination::DrawAgreement, "draw"),
        (Termination::Abort, "abort"),
    ];
    for (term, s) in t {
        assert_eq!(term.as_str(), s);
    }
    assert_eq!(crate::color_str(Color::Black), "black");
    assert_eq!(crate::check_str(qchess::Check::Quantum), "quantum");
    assert_eq!(crate::check_str(qchess::Check::None), "none");
}

#[test]
fn check_state_reported() {
    // classical check: 1.e4 f6 2.Qh5+ -> black in check; 'full'
    let mut r = room();
    for (u, m) in [(W, "e2e4"), (B, "f7f6"), (W, "Qd1h5")] {
        r.play(u, m, 1).unwrap();
    }
    assert_eq!(r.snapshot(None, 1).check_str(), "full");
    assert!(!r.is_over());
}

#[test]
fn moves_log_records_clock_and_time() {
    let mut r = room_tc(TimeControl::new(5, 3), true);
    r.play(W, "e2e4", 10).unwrap();
    r.play(B, "e7e5", 20).unwrap();
    r.play(W, "Ng1f3", 1020).unwrap();
    let m = &r.moves()[2];
    assert_eq!((m.ply, m.at_ms), (3, 1020));
    assert_eq!(m.white_ms, 300_000 - 1000 + 3000);
    assert_eq!(m.black_ms, 300_000);
    assert!(m.collapses.is_empty());
}

#[test]
fn room_accessors() {
    let r = room();
    assert_eq!(r.id(), "g1");
    assert_eq!(r.white().username, "avi");
    assert_eq!(r.black().user_id, B);
    assert_eq!(r.player(Color::Black).username, "bob");
    assert_eq!(r.time_control(), TimeControl::new(3, 2));
    assert!(r.rated());
    assert_eq!(r.created_ms(), 0);
    assert!(r.chat_log().is_empty());
    assert!(r.moves().is_empty());
}
