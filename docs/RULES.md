# Rules

Quantum chess as implemented by the `qchess` engine. It is ordinary chess,
plus one new idea: a piece can **split** into two halves that sit on two
squares at once. Until something forces it to choose, the piece is on both.

## Pieces and boards

Every piece has two halves. An ordinary piece has both halves on one square.
A split piece has its halves on two different squares.

A position is really a set of ordinary chess boards: one for each way of
picking one half of every split piece. With *n* split pieces there are up
to 2^*n* possible boards. A board that couldn't have come about legally is
dropped (see Collapse below).

A piece that is already split cannot split again until it has collapsed.

## Moves

| Notation | Move |
|---|---|
| `Nf3`, `exd5` | **Whole move**: the piece moves as in ordinary chess. For a split piece, each half that can reach the square goes there, and the piece is whole again; if only one half can, the move proves the piece was there, so it collapses onto that half. |
| `Na3b5/c3d5` | **Both halves of a split piece move**, each to its own square. |
| `Bb5/c4` | **Split**: one half goes to b5, the other to c4. Each must be a legal destination for the piece. |
| `e3/4` | Shorthand split that keeps the file: e3 and e4. |
| `0-0/`, `0-0-0/` | **Quantum castling**: the rook moves, and half the king castles; the other half stays home and keeps its castling rights. |
| `e8=Q/N` | Promote into two different pieces on e8, one per half. |
| `Ncd5/eg6` | Disambiguation, given separately for each half. |

A move is legal if it is a legal ordinary-chess move on at least one of the
possible boards. Your own king must be safe on the boards that survive.

Every board that survives must come from a previous board by a legal
ordinary-chess move. So a piece can never leave half of itself behind (on
those boards nothing would have moved); the one exception is quantum
castling, where the rook moves on the boards where the king stayed.

Trailing `+`, `#`, `?` and `!` are ignored. See [`notation.rs`](../crates/qchess/src/notation.rs)
for the full grammar.

## Collapse

After a move, the engine works out every board you could get by playing it
legally, under ordinary chess rules, from one of the previous boards. Any
other combination of halves is impossible, so pieces **collapse**: a split
piece becomes whole on one of its two squares. That repeats until every
remaining board is possible.

- As few pieces collapse as possible.
- When there is more than one smallest way to collapse, **the player who
  moved chooses**. In notation the choice is written in braces, naming the
  half each collapsed piece keeps: `{d6}`, `{Nc3}`, or `{xBd5}` for "the half
  captured on d5".

Some consequences:

- **Captures.** Moving onto a square takes every enemy half on it.
  Capturing only one half of a split piece forces a collapse.
- **Pawn captures** take both halves.
- **Moving through a half.** A bishop, rook or queen may move past a square
  where a split piece might be. On the boards where that piece is in the
  way the move is impossible, so the blocking piece collapses onto its
  other square.
- **Splitting the king.** A split king has to be safe on the boards that
  survive the move. If an enemy king steps next to one half, that board is
  impossible, so the king collapses onto its other half.

## Check and the end of the game

- **Check** (`+`): the king is attacked on every possible board.
- **Quantum check** (`+?`, shown as "Check?"): the king is attacked on some
  boards but not all. The reply must leave the king safe on every board
  that survives.
- **Checkmate**: no board has a legal move, and the king is in check on
  every board.
- **Stalemate**: no board has a legal move, but the king is not in check on
  every board. It is a draw.

Resignation, draw offers, clocks and aborts work as on any chess site.

## An example

```
1. e3/4    e5        (the e-pawn splits to e3 and e4)
2. Nf3     Nc6
3. Bb5/c4  Ne7/f6    (bishop and knight split)
4. 0-0/              (quantum castling: half of the king castles)
```

After these moves the pawn, the bishop, the knight and the king are each
split, so there are 2^4 = 16 possible boards. Run it with
`cargo run -p qchess --example sheet`.
