import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const Q = createRequire(import.meta.url)('./board.js');
const { buildNotation: B, reduceInput, newInput } = Q;

test('buildNotation: PROTOCOL table', () => {
  assert.equal(B({ kind: 'knight', from: 'b1', to: 'c3' }), 'Nb1c3');
  assert.equal(B({ kind: 'pawn', from: 'e2', to: 'e4' }), 'e2e4');
  assert.equal(B({ kind: 'knight', from: 'b1', to: 'c3', to2: 'a3' }), 'Nb1c3/a3');
  assert.equal(B({ kind: 'knight', from: 'a3', to: 'b5', from2: 'c3', to2: 'd5' }), 'Na3b5/c3d5');
  assert.equal(B({ kind: 'pawn', from: 'e7', to: 'e8', promo: 'Q' }), 'e7e8=Q');
  assert.equal(B({ kind: 'pawn', from: 'e7', to: 'e8', to2: 'e8', promo: 'Q', promo2: 'N' }), 'e7e8=Q/=N');
  assert.equal(B({ kind: 'king', from: 'e1', to: 'g1' }), '0-0');
  assert.equal(B({ kind: 'king', from: 'e1', to: 'c1' }), '0-0-0');
  assert.equal(B({ kind: 'king', from: 'e8', to: 'g8' }), '0-0');
  assert.equal(B({ kind: 'king', from: 'e1', to: 'g1', stay: true }), '0-0/');
  assert.equal(B({ kind: 'king', from: 'e8', to: 'c8', stay: true }), '0-0-0/');
  assert.equal(B({ kind: 'king', from: 'e1', to: 'e2' }), 'Ke1e2');
  assert.equal(B({ kind: 'bishop', from: 'f1', to: 'b5', to2: 'c4' }), 'Bf1b5/c4');
  assert.equal(B({ kind: 'N', from: 'b1', to: 'c3' }), 'Nb1c3');
});

const pcs = [
  { id: 1, color: 'white', split: false, halves: [{ square: 'e7', kind: 'pawn', alive: true }] },
  { id: 2, color: 'white', split: true, halves: [{ square: 'a3', kind: 'knight', alive: true }, { square: 'c3', kind: 'knight', alive: true }] },
  { id: 3, color: 'white', split: false, halves: [{ square: 'b1', kind: 'knight', alive: true }, { square: 'x1', kind: 'knight', alive: false }] },
  { id: 4, color: 'white', split: false, halves: [{ square: 'e1', kind: 'king', alive: true }] },
];
const targets = { e7: ['e8', 'd8'], a3: ['b5'], c3: ['d5'], b1: ['c3x', 'd2'], e1: ['g1', 'c1', 'e2'] };
const ctx = { you: 'white', pieces: pcs, targets };
const run = (acts) => {
  let ui = newInput(), out;
  for (const a of acts) { const r = reduceInput(ui, a, ctx); ui = r.ui; if (r.notation) out = r.notation; }
  return { ui, out };
};
const c = (sq) => ({ t: 'click', sq });

test('reducer: both halves are always placed; same square is an ordinary move', () => {
  assert.equal(run([c('b1'), c('d2')]).ui.phase, 'second');
  assert.equal(run([c('b1'), c('d2'), c('d2')]).out, 'Nb1d2');
  assert.equal(run([c('b1'), c('b1')]).ui.phase, 'idle');
  assert.equal(run([c('b1'), { t: 'cancel' }]).ui.phase, 'idle');
  assert.equal(run([c('h5')]).ui.phase, 'idle');
  // A second square that isn't a target cancels.
  assert.equal(run([c('b1'), c('d2'), c('h5')]).ui.phase, 'idle');
});
test('reducer: split and split pieces', () => {
  assert.equal(run([c('b1'), c('d2'), c('c3x')]).out, 'Nb1d2/c3x');
  // Split piece: each half moves; the second click is the twin's destination.
  assert.equal(run([c('a3'), c('b5'), c('d5')]).out, 'Na3b5/c3d5');
  // The twin can't reach b5, so moving only this half is offered explicitly.
  const r = run([c('a3'), c('b5')]);
  assert.equal(r.ui.phase, 'second');
  assert.equal(reduceInput(r.ui, { t: 'only' }, ctx).notation, 'Na3b5');
  // There is no stay move.
  assert.equal(reduceInput(run([c('b1'), c('d2')]).ui, { t: 'only' }, ctx).notation, undefined);
});
test('reducer: promotion + castling', () => {
  assert.equal(run([c('e7'), c('e8')]).ui.phase, 'promo1');
  assert.equal(run([c('e7'), c('e8'), { t: 'promo', p: 'Q' }, c('e8'), { t: 'promo', p: 'Q' }]).out, 'e7e8=Q');
  assert.equal(run([c('e7'), c('e8'), { t: 'promo', p: 'Q' }, c('e8'), { t: 'promo', p: 'N' }]).out, 'e7e8=Q/=N');
  assert.equal(run([c('e7'), c('e8'), { t: 'promo', p: 'Q' }, c('d8'), { t: 'promo', p: 'R' }]).out, 'e7e8=Q/d8=R');
  assert.equal(run([c('e1'), c('g1'), c('g1')]).out, '0-0');
  assert.equal(run([c('e1'), c('c1'), c('e1')]).out, '0-0-0/');
  // Castling can't be split with a king step.
  assert.deepEqual(Q.secondTargets(run([c('e1'), c('e2')]).ui, ctx), ['e2']);
});
test('describeOption', () => {
  assert.equal(Q.describeOption('{Pd6}'), 'Pawn stays on d6');
  assert.equal(Q.describeOption('{xPd5}'), 'Pawn captured on d5');
  assert.equal(Q.describeOption('{Nd5}'), 'Knight on d5');
});
test('clocks', () => {
  assert.equal(Q.formatClock(171234), '2:51');
  assert.equal(Q.formatClock(9400), '0:09.4');
  assert.equal(Q.formatClock(3725000), '1:02:05');
  const clock = { white_ms: 10000, black_ms: 5000, running: 'white', server_now_ms: 1000 };
  assert.equal(Q.clockMs(clock, 'white', 5500, 500), 6000); // serverNow=5000, elapsed 4000
  assert.equal(Q.clockMs(clock, 'black', 9999, 0), 5000);
});
test('groupMoves / helpers', () => {
  const rows = Q.groupMoves([{ ply: 1, notation: 'a' }, { ply: 2, notation: 'b' }, { ply: 3, notation: 'c' }]);
  assert.equal(rows.length, 2); assert.equal(rows[1].w.notation, 'c'); assert.equal(rows[1].b, null);
  assert.equal(Q.resultBanner({ kind: 'ended', result: '1-0', termination: 'checkmate' }), '1-0 • checkmate');
  assert.deepEqual(Q.twinsOf(pcs, 'a3'), ['c3']);
  assert.equal(Q.describeSquare('a3', Q.squareMap(pcs).a3), 'a3 white knight (quantum)');
});

test('numbered pieces: badges, names and collapse options', () => {
  const pawn = { id: 1, color: 'white', origin: 'pawn', num: 5, halves: [{ square: 'e3', kind: 'pawn', alive: true }, { square: 'e4', kind: 'pawn', alive: true }] };
  const queen = { id: 2, color: 'white', origin: 'pawn', num: 1, halves: [{ square: 'a8', kind: 'queen', alive: true }] };
  const bishop = { id: 3, color: 'white', origin: 'bishop', num: null, halves: [{ square: 'c1', kind: 'bishop', alive: true }] };
  assert.equal(Q.badgeText(pawn, 'pawn'), '5');
  assert.equal(Q.badgeText(queen, 'queen'), 'p1');
  assert.equal(Q.badgeText(bishop, 'bishop'), '');
  assert.equal(Q.pieceName(queen, 'queen'), 'queen (pawn 1)');
  const map = Q.squareMap([pawn, queen, bishop]);
  assert.equal(Q.describeOption('{Pe4}', map), 'Pawn 5 stays on e4');
  assert.equal(Q.describeSquare('e3', map.e3), 'e3 white pawn 5 (quantum)');
});

test('a half-captured piece is still described as uncertain', () => {
  const pawn = { id: 1, color: 'black', origin: 'pawn', num: 4, split: true,
    halves: [{ square: 'd5', kind: 'pawn', alive: false }, { square: 'd6', kind: 'pawn', alive: true }] };
  assert.equal(Q.describeSquare('d6', Q.squareMap([pawn]).d6), 'd6 black pawn 4 (maybe captured)');
});

test('numbered collapse options pick the right piece', () => {
  const n1 = { id: 1, color: 'white', origin: 'knight', num: 1, split: true, halves: [{ square: 'd2', kind: 'knight', alive: true }, { square: 'a3', kind: 'knight', alive: true }] };
  const n2 = { id: 2, color: 'white', origin: 'knight', num: 2, split: true, halves: [{ square: 'a3', kind: 'knight', alive: true }, { square: 'd2', kind: 'knight', alive: true }] };
  const map = Q.squareMap([n1, n2]);
  assert.equal(Q.describeOption('{N1d2,N2a3}', map), 'Knight 1 on d2, knight 2 on a3');
  assert.equal(Q.describeOption('{N2d2}'), 'Knight 2 on d2');
});
