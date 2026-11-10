// Web Worker for games against the computer: holds a qchess-wasm LocalGame,
// answers the board's messages, keeps the clock and plays the computer's
// moves. The search blocks this worker (not the page) for up to a few
// seconds; messages sent meanwhile are handled right after.
import init, { LocalGame } from '/pkg/qchess_wasm.js';

const ready = init();
let game = null;
let tickTimer = null;
let botTimer = null;

function post(json) {
  for (const msg of JSON.parse(json)) self.postMessage({ msg });
  self.postMessage({ save: game.save() });
  schedule();
}

function schedule() {
  clearTimeout(tickTimer);
  const at = game.next_deadline(Date.now());
  if (at >= 0) tickTimer = setTimeout(() => post(game.tick(Date.now())), Math.max(0, at - Date.now()) + 5);
  if (game.bot_to_move() && !botTimer) botTimer = setTimeout(botMove, 50);
}

function botMove() {
  botTimer = null;
  if (!game.bot_to_move()) return;
  const notation = game.think(Date.now(), Math.floor(Math.random() * 2 ** 52));
  if (notation) post(game.play_bot(notation, Date.now()));
}

self.onmessage = async (e) => {
  await ready;
  const d = e.data;
  if (d.start) {
    try {
      if (d.start.saved) game = LocalGame.restore(d.start.saved);
      else {
        const s = d.start.settings;
        game = new LocalGame(s.tc, s.human_white, s.level, s.name, Date.now());
      }
    } catch (err) {
      self.postMessage({ fatal: String(err && err.message || err) });
      return;
    }
    post(game.hello(Date.now()));
  } else if (!game) {
    // Not started yet.
  } else if (d.send) {
    post(game.message(JSON.stringify(d.send), Date.now()));
  } else if (d.positions) {
    self.postMessage({ positions: JSON.parse(game.positions()) });
  }
};
