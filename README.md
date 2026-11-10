# quantum-chess

A lichess-style site for playing quantum chess online, with a rules engine, a computer opponent
and an exact endgame solver, all in Rust.

In quantum chess a piece can **split** into two halves that sit on two squares at once. A
position is a set of possible ordinary boards, and a move that rules some of them out
**collapses** the pieces involved. Check is check on every board, checkmate means no board has
a legal move. The full rules, with notation, are in [docs/RULES.md](docs/RULES.md).

## Quick start

You need Rust 1.99 or newer ([rustup](https://rustup.rs)) and a C compiler. Then, from the
repository root:

```sh
cargo run --release -p qchess-web --features test-auth
```

Open <http://localhost:3000>, click **Sign in**, and **Register** any username and password. To
play another person, open a second browser profile (or a private window), register a second
user, and accept the seek you made from the first. To play the computer, pick **Play the
computer** in the lobby.

`test-auth` replaces OAuth with a local password form so you can try the site without
registering any OAuth apps. Don't use it on a public server. For a real deployment see
[Configuration](#configuration).

Or try the engine on its own:

```sh
cargo run -p qchess --example sheet   # plays a short quantum opening and prints the board
```

## Layout

| Crate | What |
|---|---|
| `crates/qchess` | the rules engine (std only) |
| `crates/qchess-play` | framework-free game logic: rooms, clocks, lobby, draw offers, chat |
| `crates/glicko2` | Glicko-2 ratings |
| `crates/qchess-web` | the site, built with [Topcoat](https://github.com/tokio-rs/topcoat), SQLite and a vanilla-JS board (`assets/board.js`) |

Docs: [RULES.md](docs/RULES.md) (the game), [PLAN.md](docs/PLAN.md) (how the site is put
together) and [PROTOCOL.md](docs/PROTOCOL.md) (the WebSocket messages between board and server).

## Features

- **Sign in with** GitHub, Google, Lichess or Apple. There are no passwords or emails. Providers
  that are not configured are hidden.
- **Lobby:** create a seek (1+0, 2+1, 3+0, 3+2, 5+0, 5+3, 10+0, 10+5, 15+10 or 30+0; rated or casual; pick a color) or accept someone else's.
  Seeks vanish when their owner leaves the lobby.
- **Live games:** server-authoritative clocks, resign, draw offers, abort, chat, spectating,
  and `/tv` for the highest-rated game in progress. A game that hasn't started within 30 s is aborted.
- **Friend links:** "Challenge a friend" gives you a link; whoever opens it and accepts starts the game.
- **Play the computer:** casual games against QBot at Easy (depth 1, sometimes picks a weaker
  move), Medium (depth 2) or Hard (pruned search up to depth 6, up to 4 s a move, less when its
  clock is short). It
  plays splits, collapses and quantum castling, and it declines draw offers.
- **Ratings:** Glicko-2 per speed (bullet, blitz, rapid, classical).
- **History and replay:** `/@/<username>` shows ratings, results and past games. Any game,
  including one in progress, can be stepped through move by move: click a move in the list, use
  the ⏮ ◀ ▶ ⏭ buttons, or the arrow keys (← → one move, ↑/Home the start, ↓/End the current
  position). In your own game, clicking the board while looking at an earlier move jumps back to
  the current position.
- Pawns, rooks and knights carry a number (pawns 1-8 by starting file, rooks and knights 1-2 from
  the a-file side) shown as a badge and used in the move list, e.g. `P₅e3/e4`, `N₂f3`, `R₁a7`.
  A split piece is drawn as half of its icon on each of its two squares.
- The board supports splits (shift-click a second target), "stay" moves, quantum castling,
  promotion, and collapse choices. See `docs/PROTOCOL.md` for the wire format.

## Requirements

- Rust 1.99 or newer (Topcoat 0.11 needs it). The code uses let-chains, which are stable since
  Rust 1.88 in edition 2024, and no nightly-only features, so the stable toolchain is enough.
  The repository has no `rust-toolchain` file. `rust-version` in `Cargo.toml` is a lower bound
  only.
- A C compiler, for the bundled SQLite.
- Node.js, only for the board tests (`node:test`, so a current version).
- `cargo-fuzz` and a nightly toolchain, only for coverage-guided fuzzing (see below).

## Run

```sh
cargo run -p qchess-web
```

Open <http://localhost:3000>. `HOST` (default `127.0.0.1`) and `PORT` (default `3000`) change the
bind address. State is kept in the
SQLite file `qchess.db` (`DATABASE_URL`, a plain path or `sqlite:path`), created on first start.
Games that were in progress when the server stopped are closed as aborted.

### Testing without OAuth

Without OAuth keys no provider is configured, so there is nothing to sign in with. Build with the
`test-auth` feature to get a username and password form (with Register) on `/login`:

```sh
cargo run -p qchess-web --features test-auth
```

Passwords are hashed with Argon2. The feature is off by default, so it is not compiled into normal
builds. It has no rate limiting or password reset: never enable it on a public server. Use a
second browser profile for the second player.

### Deploying

Build with `cargo build --release -p qchess-web` and run `target/release/qchess-web` behind an
HTTPS reverse proxy (Caddy, nginx, `tailscale serve`, ...) that passes WebSocket upgrades
through. Keep `HOST=127.0.0.1` so only the proxy can reach it. Set `PUBLIC_URL` to the public
`https://` origin and configure at least one sign-in provider (below). Back up the SQLite file,
and run a single instance.

## Configuration

| Variable | Meaning |
|---|---|
| `PUBLIC_URL` | public origin, used for OAuth redirects and challenge links. Default `http://localhost:$PORT` |
| `DATABASE_URL` | SQLite path, default `qchess.db` |
| `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | GitHub |
| `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | Google |
| `LICHESS_CLIENT_ID` | Lichess (any unique string; PKCE, no secret) |
| `APPLE_SERVICES_ID`, `APPLE_TEAM_ID`, `APPLE_KEY_ID`, `APPLE_PRIVATE_KEY_PATH` | Apple |

The redirect (callback) URL for each provider is `$PUBLIC_URL/auth/<provider>/callback`, with
`<provider>` one of `github`, `google`, `lichess`, `apple`.

### GitHub
Settings, Developer settings, OAuth Apps, New OAuth App. Set the callback URL to
`$PUBLIC_URL/auth/github/callback`, then copy the client ID and generate a client secret.

### Google
Google Cloud Console, APIs and Services, Credentials, Create credentials, OAuth client ID
(Web application). Add `$PUBLIC_URL/auth/google/callback` as an authorized redirect URI.

### Lichess
Nothing to register. Choose any unique `LICHESS_CLIENT_ID` (for example `quantum-chess-yourdomain`).
Lichess shows the redirect host to the user on the consent screen.

### Apple
Needs an Apple Developer account and a public **HTTPS** domain (localhost does not work; use a
tunnel in development). Create an App ID with Sign in with Apple, then a Services ID (this is
`APPLE_SERVICES_ID`) and under its Sign in with Apple settings add your domain and the return URL
`$PUBLIC_URL/auth/apple/callback`. Create a Sign in with Apple key: its ID is `APPLE_KEY_ID`, its
`.p8` file is `APPLE_PRIVATE_KEY_PATH`, and your Team ID is `APPLE_TEAM_ID`. Apple posts back to the
callback (`response_mode=form_post`), so that one route is exempt from the same-origin check.
Apple only sends the user's name on their very first sign-in.

## Notes

- Sessions live in SQLite. When `PUBLIC_URL` is `https://`, the session cookie is
  `__Host-session` (Secure, HttpOnly, SameSite=Lax). Over plain `http://` (local development) it
  is `qc_session` without `Secure`, so every browser, including Safari, accepts it.
- OAuth is tested against a local mock provider (`crates/qchess-web/src/auth_mock_tests.rs`): state and PKCE checks,
  code exchange, profile loading for all four providers, and Apple's ES256 client secret.
- Chat is plain text, up to 140 characters; spectators can chat too.
- The server is a single process: lobby and live rooms are in memory, finished games and ratings
  are in SQLite. Run one instance.

## Tests

Run these from the repository root:

```sh
cargo test --workspace --features qchess-web/test-auth
node crates/qchess-web/assets/board.test.mjs
```

### Fuzzing the engine

The harness in `crates/qchess/src/fuzzing.rs` plays a sequence of legal moves, then one arbitrary
token (raw bytes or a mutated legal move). After every move it checks the engine against the rule
sheet, using [shakmaty](https://crates.io/crates/shakmaty) as an independent chess oracle. Every
resulting board must be reachable by a legal move (rule 3). A move that is legal on some board
must be accepted (rule 1). Check, mate and stalemate must match the boards, and replay must be
deterministic.

A quick deterministic run is part of `cargo test`. For more cases:

```sh
QCHESS_FUZZ_CASES=5000 cargo test -p qchess --release fuzzing
```

Coverage-guided fuzzing with libFuzzer (needs `cargo install cargo-fuzz` and a nightly toolchain):

```sh
cd crates/qchess/fuzz && cargo +nightly fuzz run -O seq_token -- -max_total_time=600 -fork=6
```

Crashes land in `crates/qchess/fuzz/artifacts/seq_token/`. Group them by panic message:

```sh
cargo run --release -p qchess --features fuzzing --example triage -- crates/qchess/fuzz/artifacts/seq_token/*
```

## The computer player

`crates/qchess/src/ai.rs` is a small search engine on the Stockfish model: iterative deepening
alpha-beta, a transposition table, captures-first move ordering with killer moves, and a
quiescence search. The eval is material plus piece-square tables, with each half of a split
piece counted as half the piece. There's no chance node, because the mover chooses the
collapse, so each collapse option is searched as its own move. There's no opening book.

Self-play games run the same per-move checks as the fuzzer. The AI picks at random among moves
within 40 centipawns of its best, so each seed gives a different game. Unlike random moves,
these games reach exchanges, promotions and mates:

```sh
QCHESS_SELFPLAY_GAMES=20 cargo test -p qchess --release smoke_self_play
cargo run --release -p qchess --features fuzzing --example selfplay -- 3 2 200   # games depth plies
```

Playing a quantum move is expensive: every possible board has to be checked. What keeps the
search fast:

- Boards are bitboards with an incrementally updated hash, so legality is tested without
  copying a board (`board.rs` is checked against the old square-scanning board, kept as
  `board_ref.rs`, on thousands of random positions).
- Moves are data, not text: notation is only written for the move that is played.
- A move is worked out once for all its collapse options, and the boards of the resulting
  position are handed over (or filtered, after a collapse) instead of listed again. Pieces
  that aren't split are placed once per position, not once per board.
- The search plays a move only when it reaches it, and the transposition table's move before
  the others are listed. It uses late move reductions, null move pruning, futility and delta
  pruning, and late move pruning near the horizon (`Config::pruning`, on by default).
- The web server uses mimalloc, which is about a fifth faster than the system allocator for
  the search's many small allocations.

`cargo run --release -p qchess --example ai_bench` times searches from a fixed middlegame.
`cargo run --release -p qchess --example ai_match -- <pairs> <depth> [plain depth]` plays the
pruned search against the unpruned one from random openings. At equal depth pruning is a
little weaker (it scored 38% at depth 3) but about 2x faster; one ply deeper it wins easily
(83% for pruned depth 4 against plain depth 3). So Easy and Medium search without pruning, and
Hard uses it to search deeper in the same time.

## Endgame research: K+B+N against a king that may split

`crates/qchess/examples/qkbn.rs` solves this ending exactly (it scores every
position, 85 million per side up to symmetry, in about 4 minutes on 16 cores). Black's king may be
on one square or split across two; White plays ordinary, unsplit moves. The solver has its own
compact move rules, and `check` compares them with the engine move by move (tens of millions
of moves, every resulting set of boards).

| Defender | White to move, positions won | Longest mate |
|---|---|---|
| Classical king | 1,352,913 of 1,359,578 | 33 moves (the known result) |
| King that may split | 102,109 (7.5%) | 41 moves |
| May split, but may not swap its halves (a pass) | 114,946 (8.5%) | 49 moves |
| May split, and White's king may split too (`qkbn2`) | 104,246 (7.7%) | 31 moves |

So against a splitting king, K+B+N is a draw from most positions. With White's king unsplit,
White's only way to collapse Black's king is to step next to one half, which drops that board,
so the king always lands on the half White *isn't* next to. A split king can't be checkmated at
all here: the only mates of a split king on paper need two different checks at once, and no
single move gives both. Every win ends by collapsing the king first.

`crates/qchess/examples/qkbn2.rs` lets White's king split as well, which lets White choose
how some collapses fall. That wins 2,137 more positions, and the longest win gets shorter
(31 moves), but the answer stays a draw from about 92% of the positions that are won in normal
chess. It has 2.5 billion positions per side and takes about 3 hours on 16 cores. White's
bishop and knight never split in either solver. That only takes options away from White, so
every win counted is a real forced win, but some draws might not be. Wins are replayed
through the real engine (`qkbn verify`, and 300 random wins at the end of every `qkbn2 solve`),
and every one has ended in checkmate.

```sh
cargo run --release -p qchess --example qkbn -- check 30000   # rules vs the engine
cargo run --release -p qchess --example qkbn -- solve         # build target/qkbn.tb, print the table above
cargo run --release -p qchess --example qkbn -- play "8/2B5/8/8/8/8/k4N2/2K5 w - - 0 1"
cargo run --release -p qchess --example qkbn -- verify 3000   # table vs random defence, through the engine
cargo run --release -p qchess --example qkbn2 -- check 20000  # rules with both kings split vs the engine
cargo run --release -p qchess --example qkbn2 -- solve        # White's king may split; compares with normal chess if target/qkbn.tb exists
```

`play` plays the fastest win against the longest defence through the engine and checks that
the engine calls the end checkmate, on exactly the predicted move. `verify` does the same from
random won positions with black moving at random.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
