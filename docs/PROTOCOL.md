# Live game protocol (browser ⇄ server)

JSON messages over one WebSocket per game page, at `/game/{id}/ws`. The server
code is in `crates/qchess-web/src/json.rs` and `hub.rs`.

## Server → client

### `state` (sent on connect and after every change)

```json
{
  "type": "state",
  "id": "k3J9xQ2a",
  "you": "white",                     // "white" | "black" | null (spectator)
  "players": {
    "white": { "username": "avi", "rating": 1512, "provisional": false },
    "black": { "username": "bob", "rating": 1500, "provisional": true }
  },
  "clock": { "initial_ms": 180000, "increment_ms": 2000,
             "white_ms": 171234, "black_ms": 175000,
             "running": "black",           // "white" | "black" | null
             "server_now_ms": 1794030691000 },
  "to_move": "black",
  "status": { "kind": "ongoing" },
  // or { "kind": "ended", "result": "1-0" | "0-1" | "1/2-1/2",
  //      "termination": "checkmate" | "stalemate" | "resign" | "timeout" | "draw" | "abort" }
  "check": "none",                     // "none" | "quantum" | "full"  (side to move)
  "state_count": 16,
  "pieces": [                          // qchess::Game::pieces()
    { "id": 12, "color": "white", "split": true,
      "origin": "pawn", "num": 5,      // numbered: pawns 1-8 by file, rooks/knights 1-2 from the a-file; else null
      "halves": [ { "square": "e3", "kind": "pawn", "alive": true },
                  { "square": "e4", "kind": "pawn", "alive": true } ] }
  ],
  "targets": { "g1": ["f3", "h3"], "b5": ["a4", "c6"] },  // side to move only; from-square → destinations
  "last_move": ["e2", "e3", "e4"],     // squares to highlight
  "moves": [ { "ply": 1, "notation": "e2e3/4", "display": "P₅e3/e4", "collapses": [] } ],
                                       // display: numbered pieces as subscripts; a promoted pawn is e.g. Qₚ₅
  "draw_offer": null,                  // "white" | "black" | null
  "rated": true
}
```

Finished games also carry `"positions"`: the start position plus one entry per
ply, each `{ "pieces": [...], "last_move": [...] }`, for stepping through the
game. `GET /api/game/{id}/positions` returns the same list for a game in
progress.

### Other server messages

```json
{ "type": "choose_collapse", "notation": "Nc3d5/", "options": ["{Nd5}", "{Pd6}"] }
{ "type": "chat", "user": "bob", "text": "gg", "ts": 1794030691000 }
{ "type": "error", "message": "illegal move: ..." }
```

## Client → server

```json
{ "type": "move", "notation": "Bf1b5/c4" }      // may end with " {Pd6}" after choose_collapse
{ "type": "resign" }
{ "type": "abort" }
{ "type": "draw_offer" }
{ "type": "draw_accept" }
{ "type": "draw_decline" }
{ "type": "chat", "text": "gg" }
```

## Notation the board UI generates

The full origin square always comes first, as disambiguation (the engine
accepts `[file][rank]` hints):

| UI action | notation |
|---|---|
| move piece b1→c3 | `Nb1c3` |
| pawn e2→e4 | `e2e4` |
| split b1→c3 and a3 | `Nb1c3/a3` |
| split; one half goes, the other stays | `Nb1c3/` |
| split piece: half on a3→b5, half on c3→d5 | `Na3b5/c3d5` |
| promotion | `e7e8=Q`; split promotion `e7e8=Q/=N` |
| castling (king e1→g1 or c1) | `0-0`, `0-0-0`; split castling `0-0/` |

If the server answers `choose_collapse`, show the options and resend the same
notation plus `" " + option`.
