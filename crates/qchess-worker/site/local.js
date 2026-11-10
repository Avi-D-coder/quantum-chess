/* A game against the computer, run by local-worker.js (WebAssembly) in this
 * browser. The worker speaks the server's protocol, so the board is the same
 * one used for live games. The game is saved after every change and resumed
 * on reload. */
(function () {
  'use strict';
  const { store } = window.QC;
  const KEY = 'qc_local_game';
  const params = new URLSearchParams(location.search);
  const el = document.getElementById('qgame');

  let settings;
  let saved = store.get(KEY);
  if (params.get('new') || !saved) {
    const color = params.get('color') || 'random';
    settings = {
      tc: params.get('tc') || '5+3',
      level: params.get('level') || 'medium',
      human_white: color === 'random' ? Math.random() < 0.5 : color === 'white',
      name: window.QC.name,
    };
    saved = null;
    store.del(KEY);
    // A reload resumes this game instead of starting another.
    history.replaceState(null, '', '/local');
  }

  const worker = new Worker('/local-worker.js', { type: 'module' });
  const msgCbs = [];
  let historyWaiters = [];
  worker.onmessage = (e) => {
    const d = e.data;
    if (d.msg) msgCbs.forEach((cb) => cb(d.msg));
    if (d.save) store.set(KEY, d.save);
    if (d.positions) { historyWaiters.forEach((f) => f(d.positions)); historyWaiters = []; }
    if (d.fatal) {
      store.del(KEY);
      el.textContent = 'Could not start the game: ' + d.fatal;
    }
  };
  worker.onerror = (e) => { el.textContent = 'The computer player failed to load: ' + (e.message || 'unknown error'); };

  const transport = {
    send(obj) { worker.postMessage({ send: obj }); return true; },
    onMessage(cb) { msgCbs.push(cb); },
    onStatus(cb) { cb('open'); },
    close() { worker.terminate(); },
  };
  window.QChess.mount(el, {
    transport,
    loadHistory: () => new Promise((resolve) => { historyWaiters.push(resolve); worker.postMessage({ positions: true }); }),
  });
  worker.postMessage({ start: saved ? { saved } : { settings } });

  document.getElementById('new-game').addEventListener('click', () => {
    let s = {};
    try { s = JSON.parse(store.get(KEY) || '{}').settings || {}; } catch (e) { /* use defaults */ }
    const color = s.human_white === undefined ? 'random' : (s.human_white ? 'white' : 'black');
    const q = new URLSearchParams({ new: '1', tc: s.tc || '5+3', level: s.level || 'medium', color });
    location.href = '/local?' + q;
  });
})();
