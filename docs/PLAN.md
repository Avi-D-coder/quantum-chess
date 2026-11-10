# quantum-chess design

A playable, lichess-style quantum chess site built on the `qchess` engine
(`crates/qchess`) with [Topcoat](https://github.com/tokio-rs/topcoat) (tokio-rs,
v0.11.x, experimental, so read the crate source for the exact API). This
document describes how the pieces fit. For setup and commands see the README.

## Scope

- **Framework:** Topcoat.
- **Login:** OAuth only, with no email/password and no mail server. Providers:
  **GitHub, Google, Lichess, Apple.** (A username/password form exists behind
  the `test-auth` feature, for local testing only.)
- **Features:**
  1. Lobby + live games: create or accept seeks, real-time play, resign, offer a draw.
  2. Clocks + ratings: time controls (1+0, 2+1, 3+0, 3+2, 5+0, 5+3, 10+0, 10+5,
     15+10, 30+0) and Glicko-2 ratings per speed (bullet/blitz/rapid/classical).
  3. History + replay: the profile lists past games and stats, and any game
     can be stepped through move by move.
  4. Friend link + spectating: challenge by shareable link, watch live games,
     and in-game chat.
  5. Casual games against the computer (QBot, three levels).

## Layout

```
crates/
  qchess/        engine (std only): rules, notation, AI
  qchess-play/   framework-free game logic: rooms, clocks, lobby (time is passed in)
  glicko2/       Glicko-2 ratings
  qchess-web/    Topcoat app
    src/main.rs        startup, 250 ms ticker for clocks and timeouts
    src/app.rs         router, root layout (<html>, nav, login state)
    src/app/...        pages and routes: play.rs, auth_routes.rs, profile.rs
    src/auth.rs        generic OAuth code + PKCE flow, providers, Apple client secret
    src/db.rs          SQLite (rusqlite); the schema is created on start
    src/hub.rs         shared state: lobby, live rooms, broadcast channels
    src/bot.rs         computer opponents and their time budgets
    src/json.rs        wire format (see PROTOCOL.md)
    assets/board.js    quantum board UI (vanilla JS)
```

### Pages and routes

| Route | What |
|---|---|
| `/` | lobby: create a seek (time control, color), open seeks, live games, "challenge a friend", play the computer |
| `/login` | provider buttons |
| `/auth/{provider}/start`, `/auth/{provider}/callback` | OAuth (Apple's callback is a POST, `response_mode=form_post`) |
| `/logout` | POST |
| `/welcome` | pick a username after first login |
| `/@/{username}` | profile: avatar, ratings per speed, W/L/D, recent games (paged) |
| `/game/{id}` | play / spectate: board, clocks, move list, resign/draw, chat; replay controls |
| `/challenge/{token}` | friend-link landing: accept → game |
| `/tv` | the highest-rated ongoing game |
| `/game/{id}/ws` | real-time moves, clocks, chat, draw offers |
| `/api/...` | JSON endpoints used by the pages: lobby, seeks, challenges, bot games, game positions and archived state |

### Data (SQLite)

- `users(id, username UNIQUE, display_name, avatar_url, created_at)`
- `identities(provider, subject, user_id, email NULL)` links each OAuth account
  to a user; several providers can map to one user.
- `passwords(user_id, hash)`: only used with the `test-auth` feature.
- `sessions(token_hash, user_id, expires_at)`
- `ratings(user_id, speed, rating, rd, volatility, games)`
- `games(id, white_id, black_id, initial_ms, increment_ms, status, result,
  termination, created_at, ended_at, rated)`
- `moves(game_id, ply, notation, collapses_json, white_ms, black_ms, at)`
- `bots(level, user_id)`: the account behind each QBot level.
- Seeks and challenges live in memory (the `hub.rs` lobby); only games and
  moves persist. Replay = re-run `moves` through `qchess::Game`.

### Auth notes

- Generic authorization-code flow with PKCE and `state`; the verifier is kept
  server-side by `state`. Then upsert the identity, start a session, and
  redirect to `/welcome` if the user has no username yet.
- **GitHub:** OAuth app; scope `read:user user:email`; profile from `api.github.com/user`.
- **Google:** OIDC; scope `openid email profile`; read `sub` from the userinfo endpoint.
- **Lichess:** PKCE, no client secret; any client_id string; scope empty; `lichess.org/api/account`.
- **Apple:** Sign in with Apple: `response_mode=form_post` (POST callback), client
  secret = ES256 JWT signed with the `.p8` key (team id, key id, services id).
  Needs an HTTPS, registered domain; localhost won't work, so put it behind a tunnel
  in dev. Name/email arrive only on the first login, in the `user` form field.
- Providers without env vars configured are hidden from `/login`. The env vars
  are listed in the README.

### Game server

- One room per live game (`qchess-play`): `qchess::Game`, clocks, chat, draw
  offer state, and in `hub.rs` a `tokio::sync::broadcast` channel. Moves are
  validated by the engine, persisted, then broadcast as a full `state` message
  (see PROTOCOL.md).
- The clock is server-authoritative: deduct elapsed time on each move, add the
  increment, and a ticker flags the game on timeout. The client shows a local
  countdown. The clock starts after both sides have moved.
- Results: checkmate, stalemate, resign, timeout, draw agreement, abort (before
  two plies; also automatic when no move is made within 30 s). The game ends →
  update Glicko-2 for rated games.

### Engine API used by the UI

- `Game::pieces()`: every piece with its 1–2 half squares, kind and color
  (so the board can draw quantum halves at 50% opacity).
- `Game::targets(from)`: squares a piece on `from` can reach in at least one
  state, for click-to-move highlighting.
- The board UI builds notation: click a piece, then one square for a normal
  move, shift-click two squares for a split (`Bb5/c4`), a "stay" button for
  `X/`, a promotion picker, and castling by dragging the king.
- If a move has several minimal collapses, show the options returned by the
  engine and resubmit with `{...}`.
