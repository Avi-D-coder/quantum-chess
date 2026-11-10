/* Quantum chess board UI. Plain ES2020, no dependencies.
 * Auto-mounts on #qgame (data-game-id, data-ws-url | data-replay-url).
 * Public API: window.QChess = { mount, GameClient, buildNotation, reduceInput, ... } */
(function (root, factory) {
  'use strict';
  const api = factory(root);
  if (typeof module === 'object' && module.exports) module.exports = api;
  if (root) root.QChess = Object.assign(root.QChess || {}, api);
  if (root && root.document) {
    const go = () => {
      const el = root.document.getElementById('qgame');
      if (el && !el.hasAttribute('data-manual') && !el.__qchess) api.autoMount(el);
    };
    if (root.document.readyState === 'loading') root.document.addEventListener('DOMContentLoaded', go);
    else go();
  }
})(typeof window !== 'undefined' ? window : null, function (root) {
  'use strict';

  /* ------------------------------------------------------------------ pure helpers */
  const FILES = 'abcdefgh';
  const KIND_LETTER = { pawn: '', knight: 'N', bishop: 'B', rook: 'R', queen: 'Q', king: 'K' };
  const KIND_FROM_LETTER = { P: 'pawn', N: 'knight', B: 'bishop', R: 'rook', Q: 'queen', K: 'king' };

  function kindLetter(k) {
    if (!k) return '';
    k = String(k);
    if (Object.prototype.hasOwnProperty.call(KIND_LETTER, k.toLowerCase())) return KIND_LETTER[k.toLowerCase()];
    k = k.toUpperCase();
    return k === 'P' ? '' : k;
  }

  /** Build the PROTOCOL notation string. kind: 'knight'|'N'|...; squares like 'b1'. */
  function buildNotation(o) {
    const { kind, from, to, to2, stay, promo, promo2, from2 } = o;
    const L = kindLetter(kind);
    if (L === 'K' && from && to && !to2 && from[1] === to[1] && from[0] === 'e' &&
        (from[1] === '1' || from[1] === '8') && Math.abs(to.charCodeAt(0) - 101) === 2) {
      const base = to[0] === 'g' ? '0-0' : '0-0-0';
      return stay ? base + '/' : base;
    }
    let s = L + from + to;
    if (promo) s += '=' + String(promo).toUpperCase();
    if (to2 || promo2) {
      s += '/';
      if (from2) s += from2;
      if (to2 && !(to2 === to && promo2 && !from2)) s += to2;
      if (promo2) s += '=' + String(promo2).toUpperCase();
    }
    return s;
  }

  const sqFile = (sq) => FILES.indexOf(sq[0]);
  const sqRank = (sq) => parseInt(sq[1], 10) - 1;

  function squareMap(pieces) {
    const m = {};
    for (const p of pieces || []) for (const h of p.halves) {
      if (h.alive === false) continue;
      (m[h.square] = m[h.square] || []).push({ piece: p, half: h });
    }
    return m;
  }
  const aliveHalves = (p) => p.halves.filter((h) => h.alive !== false);
  /** In superposition: two live halves, or one live and one captured ("alive and dead"). */
  const isQuantum = (p) => p.halves.length > 1 && (p.split !== false);

  /** Own half on sq → {piece, half, other} (other = square of the twin or null). */
  function ownHalfAt(pieces, you, sq) {
    for (const p of pieces || []) {
      if (p.color !== you) continue;
      const al = aliveHalves(p);
      for (const h of al) if (h.square === sq) {
        const o = al.find((x) => x !== h);
        return { piece: p, half: h, other: o ? o.square : null };
      }
    }
    return null;
  }
  function twinsOf(pieces, sq) {
    const out = [];
    for (const p of pieces || []) {
      const al = aliveHalves(p);
      if (al.length === 2) for (const h of al) if (h.square === sq) out.push(al.find((x) => x !== h).square);
    }
    return out;
  }

  const needsPromo = (kind, to, you) =>
    kind === 'pawn' && ((you === 'white' && to[1] === '8') || (you === 'black' && to[1] === '1'));

  /* Move input. Every move says where both halves of a piece go, so there
   * is no "normal move" default and no split mode:
   *   1. pick a piece (one of its halves) and where that half goes;
   *   2. pick where the other half goes: the same square is an ordinary
   *      move (or, for a split piece, the halves rejoining); another square
   *      splits the piece (or moves each half of a split piece).
   * Quantum castling offers the king's own square as the second square.
   * Moving only one half of a split piece is legal but collapses the piece
   * onto that half; it is a separate, explicit action ('only'). Leaving half
   * of a piece behind is never legal (except quantum castling). */
  function newInput() {
    return { phase: 'idle', from: null, to: null, from2: null, to2: null,
      stay: false, promo: null, promo2: null, kind: null, other: null };
  }

  const isCastle = (ui) => ui.kind === 'king' && ui.from && ui.to && !ui.other &&
    (ui.from === 'e1' || ui.from === 'e8') && ui.to[1] === ui.from[1] &&
    Math.abs(ui.to.charCodeAt(0) - ui.from.charCodeAt(0)) === 2;

  /** Squares the second half may go to (phase 'second'). */
  function secondTargets(ui, ctx) {
    const tg = ctx.targets || {};
    if (ui.other) return tg[ui.other] || [];
    if (isCastle(ui)) return [ui.to, ui.from];
    // A split can't combine castling with a king step.
    return (tg[ui.from] || []).filter((t) => t === ui.to || !(ui.kind === 'king' && isCastle({ kind: 'king', from: ui.from, to: t })));
  }
  /** For a split piece: can the move be made by moving only the chosen half? */
  const onlyThisHalf = (ui, ctx) => ui.phase === 'second' && !!ui.other && !(((ctx.targets || {})[ui.other] || []).includes(ui.to));

  function finish(ui, ctx) {
    if (needsPromo(ui.kind, ui.to, ctx.you) && !ui.promo) { ui.phase = 'promo1'; return { ui }; }
    if (ui.phase === 'promo1' || ui.phase === 'first') { ui.phase = 'second'; return { ui }; }
    if (ui.to2 && !ui.stay && needsPromo(ui.kind, ui.to2, ctx.you) && !ui.promo2) { ui.phase = 'promo2'; return { ui }; }
    const o = Object.assign({}, ui);
    if (o.to2 === o.to && !o.from2) {
      // Both halves on one square: an ordinary move, unless a pawn promotes
      // into two different pieces.
      if (!(o.promo2 && o.promo2 !== o.promo)) { o.to2 = null; o.promo2 = null; }
    } else if (o.from2 && o.to2 === o.to) {
      // The halves of a split piece rejoin: a whole move.
      o.from2 = null; o.to2 = null; o.promo2 = null;
    }
    return { ui: newInput(), notation: buildNotation(o) };
  }

  /** Pure input state machine. ctx = {you, pieces, targets}.
   *  actions: {t:'click', sq}, {t:'only'}, {t:'promo', p}, {t:'cancel'}
   *  returns {ui, notation?}. */
  function reduceInput(ui0, a, ctx) {
    const ui = Object.assign({}, ui0 || newInput());
    const tg = ctx.targets || {};
    switch (a.t) {
      case 'cancel':
        return { ui: newInput() };
      case 'only':
        if (!onlyThisHalf(ui, ctx)) return { ui };
        return { ui: newInput(), notation: buildNotation({ kind: ui.kind, from: ui.from, to: ui.to, promo: ui.promo }) };
      case 'promo':
        if (ui.phase === 'promo1') { ui.promo = a.p; return finish(ui, ctx); }
        if (ui.phase === 'promo2') { ui.promo2 = a.p; return finish(ui, ctx); }
        return { ui };
      case 'click': {
        const sq = a.sq;
        if (ui.phase === 'idle' || ui.phase === 'first') {
          if (ui.phase === 'first' && (tg[ui.from] || []).includes(sq)) {
            ui.to = sq;
            return finish(ui, ctx);
          }
          const own = ownHalfAt(ctx.pieces, ctx.you, sq);
          if (own && ((tg[sq] || []).length || (own.other && (tg[own.other] || []).length))) {
            if (ui.phase === 'first' && ui.from === sq) return { ui: newInput() };
            if (!(tg[sq] || []).length) return { ui: newInput() };
            Object.assign(ui, newInput(), { from: sq, kind: own.half.kind || own.piece.kind, other: own.other, phase: 'first' });
            return { ui };
          }
          return { ui: newInput() };
        }
        if (ui.phase === 'second') {
          if (!secondTargets(ui, ctx).includes(sq)) return { ui: newInput() };
          if (isCastle(ui) && sq === ui.from) { ui.stay = true; ui.to2 = null; return finish(ui, ctx); }
          if (ui.other) ui.from2 = ui.other;
          ui.to2 = sq;
          return finish(ui, ctx);
        }
        return { ui }; // promo phases: ignore board clicks
      }
      default:
        return { ui };
    }
  }

  const NAMES = { K: 'king', Q: 'queen', R: 'rook', B: 'bishop', N: 'knight', P: 'pawn' };

  /** Badge text for a numbered piece: "5", or "p5" for a piece promoted from pawn 5; '' if unnumbered. */
  function badgeText(piece, kind) {
    if (!piece || piece.num == null) return '';
    return (piece.origin && kind && piece.origin !== kind ? 'p' : '') + piece.num;
  }
  /** "pawn 5", "rook 2", "queen (pawn 5)", "bishop". */
  function pieceName(piece, kind) {
    kind = kind || (piece && piece.kind) || '';
    if (!piece || piece.num == null) return kind;
    return piece.origin && piece.origin !== kind ? kind + ' (' + piece.origin + ' ' + piece.num + ')' : kind + ' ' + piece.num;
  }
  /** "{xPd5}" → "Pawn captured on d5" */
  function describeOption(opt, map) {
    const inner = String(opt).trim().replace(/^\{|\}$/g, '');
    // Token: [x][Kind][number][square]; the number picks one of several pieces sharing a square.
    const re = /(x)?([KQRBNP])?(\d+)?([a-h][1-8])/g;
    const parts = [];
    let m;
    while ((m = re.exec(inner))) {
      const kind = NAMES[m[2] || 'P'], num = m[3] != null ? Number(m[3]) : null, sq = m[4];
      const here = map && (map[sq] || []).find((e) => (e.half.kind || e.piece.kind) === kind &&
        (num == null || (e.piece.num == null ? 0 : e.piece.num) === num));
      const name = here ? pieceName(here.piece, kind) : (num ? kind + ' ' + num : kind);
      if (m[1]) parts.push(name + ' captured on ' + sq);
      else if (!m[2] || m[2] === 'P') parts.push(name + ' stays on ' + sq);
      else parts.push(name + ' on ' + sq);
    }
    if (!parts.length) return String(opt);
    const s = parts.join(', ');
    return s[0].toUpperCase() + s.slice(1);
  }

  /** Remaining ms for `color` at local time `now`; offset = localNow - server_now_ms measured on receipt. */
  function clockMs(clock, color, now, offset) {
    if (!clock) return null;
    const base = color === 'white' ? clock.white_ms : clock.black_ms;
    if (clock.running !== color) return Math.max(0, base);
    const elapsed = Math.max(0, (now - (offset || 0)) - clock.server_now_ms);
    return Math.max(0, base - elapsed);
  }
  function formatClock(ms) {
    if (ms == null) return '--:--';
    ms = Math.max(0, ms);
    if (ms < 10000) return '0:0' + Math.floor(ms / 1000) + '.' + Math.floor((ms % 1000) / 100);
    const t = Math.floor(ms / 1000), s = t % 60, m = Math.floor(t / 60) % 60, h = Math.floor(t / 3600);
    const p = (n) => (n < 10 ? '0' : '') + n;
    return h ? h + ':' + p(m) + ':' + p(s) : m + ':' + p(s);
  }

  /** moves → rows [{n, w, b}] where w/b = move or null */
  function groupMoves(moves) {
    const rows = [];
    for (const mv of moves || []) {
      const i = Math.floor((mv.ply - 1) / 2);
      rows[i] = rows[i] || { n: i + 1, w: null, b: null };
      if (mv.ply % 2 === 1) rows[i].w = mv; else rows[i].b = mv;
    }
    return Array.from(rows).filter(Boolean);
  }

  function resultBanner(status) {
    if (!status || status.kind !== 'ended') return null;
    return status.result + ' • ' + status.termination;
  }
  function resultText(status) {
    if (!status || status.kind !== 'ended') return '';
    if (status.result === '1-0') return 'White wins';
    if (status.result === '0-1') return 'Black wins';
    return 'Draw';
  }
  function statusLine(s) {
    if (!s) return '';
    if (s.status && s.status.kind === 'ended') return resultText(s.status);
    const parts = [];
    if (s.check === 'quantum') parts.push('Check?');
    else if (s.check === 'full') parts.push('Check!');
    return parts.join(' ');
  }
  function describeSquare(sq, entries) {
    if (!entries || !entries.length) return sq + ' empty';
    return sq + ' ' + entries.map((e) => e.piece.color + ' ' + pieceName(e.piece, e.half.kind || e.piece.kind) +
      (aliveHalves(e.piece).length > 1 ? ' (quantum)' : isQuantum(e.piece) ? ' (maybe captured)' : '')).join(', ');
  }
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  /* ------------------------------------------------------------------ piece glyphs (original) */
  // Drawn in a 100x100 box. SHAPE = silhouette (filled + outlined), DETAIL = inner strokes/dots.
  const SHAPES = {
    pawn: '<circle cx="50" cy="29" r="11"/><path d="M41 42 H59 C58 50 62 56 66 64 H34 C38 56 42 50 41 42Z"/><path d="M27 83 V77 Q27 68 36 64 H64 Q73 68 73 77 V83Z"/>',
    rook: '<path d="M27 84 V76 L33 70 V44 L28 38 V16 H40 V25 H46 V16 H54 V25 H60 V16 H72 V38 L67 44 V70 L73 76 V84Z"/>',
    knight: '<path d="M27 84 V77 C27 62 33 52 42 44 C35 44 30 47 25 53 L18 47 C23 37 31 30 38 24 L40 13 L48 19 C63 19 77 31 77 52 C77 64 74 72 74 77 V84Z"/>',
    bishop: '<circle cx="50" cy="14" r="6"/><path d="M50 21 C65 30 70 43 63 55 C61 59 58 61 58 65 H67 V71 H33 V65 H42 C42 61 39 59 37 55 C30 43 35 30 50 21Z"/><path d="M26 84 V77 Q26 72 33 71 H67 Q74 72 74 77 V84Z"/>',
    queen: '<path d="M26 84 V75 L31 68 L20 30 L34 48 L35 22 L44 45 L50 16 L56 45 L65 22 L66 48 L80 30 L69 68 L74 75 V84Z"/><circle cx="20" cy="28" r="5"/><circle cx="35" cy="20" r="5"/><circle cx="50" cy="14" r="5"/><circle cx="65" cy="20" r="5"/><circle cx="80" cy="28" r="5"/>',
    king: '<path d="M46 8 H54 V15 H61 V22 H54 V30 H46 V22 H39 V15 H46Z"/><path d="M50 30 C68 30 76 44 70 58 L74 68 H26 L30 58 C24 44 32 30 50 30Z"/><path d="M24 84 V76 Q24 69 32 68 H68 Q76 69 76 76 V84Z"/>',
  };
  const DETAILS = {
    pawn: '<path d="M34 64 H66 M30 77 H70"/>',
    rook: '<path d="M33 44 H67 M33 70 H67 M30 38 H70"/>',
    knight: '<circle cx="50" cy="33" r="2.4" fill="currentColor"/><path d="M27 77 H74 M26 53 Q34 50 40 45"/>',
    bishop: '<path d="M50 30 V44 M43 37 H57 M33 65 H67 M30 77 H70"/>',
    queen: '<path d="M30 70 H70 M27 77 H73"/>',
    king: '<path d="M50 34 V44 M44 39 H56 M30 68 H70 M27 76 H73"/>',
  };
  const KINDS = ['pawn', 'knight', 'bishop', 'rook', 'queen', 'king'];

  function defsSvg(pfx) {
    let s = '<defs>';
    for (const k of KINDS) {
      s += '<g id="' + pfx + 's-' + k + '">' + SHAPES[k] + '</g><g id="' + pfx + 'd-' + k + '">' + DETAILS[k] + '</g>';
    }
    s += '<radialGradient id="' + pfx + 'chk" cx="50%" cy="50%" r="55%"><stop offset="0" stop-color="#ff2b2b" stop-opacity=".95"/>' +
      '<stop offset=".45" stop-color="#e01919" stop-opacity=".6"/><stop offset="1" stop-color="#a00" stop-opacity="0"/></radialGradient>';
    s += '<radialGradient id="' + pfx + 'qchk" cx="50%" cy="50%" r="55%"><stop offset="0" stop-color="#ff4d4d" stop-opacity=".55"/>' +
      '<stop offset=".5" stop-color="#e01919" stop-opacity=".3"/><stop offset="1" stop-color="#a00" stop-opacity="0"/></radialGradient>';
    s += '<clipPath id="' + pfx + 'cl"><rect x="-10" y="-10" width="60" height="120"/></clipPath>' +
      '<clipPath id="' + pfx + 'cr"><rect x="50" y="-10" width="60" height="120"/></clipPath>';
    return s + '</defs>';
  }
  function pieceUse(pfx, color, kind, extra) {
    return '<g class="qc-glyph c-' + color + (extra || '') + '">' +
      '<use href="#' + pfx + 's-' + kind + '" class="o"/><use href="#' + pfx + 's-' + kind + '" class="f"/>' +
      '<use href="#' + pfx + 'd-' + kind + '" class="d"/></g>';
  }

  /* ------------------------------------------------------------------ transport */
  class GameClient {
    constructor(url) {
      this.url = url; this.msgCbs = []; this.statusCbs = []; this.attempt = 0; this.ws = null; this.closed = false;
      this.status = 'connecting';
      this.connect();
    }
    resolve() {
      const u = this.url;
      if (/^wss?:\/\//.test(u)) return u;
      const loc = root.location;
      const proto = loc.protocol === 'https:' ? 'wss://' : 'ws://';
      if (u.startsWith('/')) return proto + loc.host + u;
      const abs = new URL(u, loc.href);
      abs.protocol = loc.protocol === 'https:' ? 'wss:' : 'ws:';
      return abs.href;
    }
    setStatus(s) { this.status = s; this.statusCbs.forEach((cb) => cb(s)); }
    connect() {
      if (this.closed) return;
      this.setStatus(this.attempt ? 'reconnecting' : 'connecting');
      let ws;
      try { ws = new root.WebSocket(this.resolve()); } catch (e) { this.scheduleReconnect(); return; }
      this.ws = ws;
      ws.onopen = () => { this.attempt = 0; this.setStatus('open'); };
      ws.onmessage = (ev) => {
        let m; try { m = JSON.parse(ev.data); } catch (e) { return; }
        this.msgCbs.forEach((cb) => cb(m));
      };
      ws.onclose = () => { if (!this.closed) this.scheduleReconnect(); };
      ws.onerror = () => { try { ws.close(); } catch (e) { /* ignore */ } };
    }
    scheduleReconnect() {
      this.setStatus('reconnecting');
      const delay = Math.min(10000, 500 * Math.pow(2, this.attempt++)) + Math.random() * 250;
      this.timer = setTimeout(() => this.connect(), delay);
    }
    send(obj) {
      if (this.ws && this.ws.readyState === 1) { this.ws.send(JSON.stringify(obj)); return true; }
      return false;
    }
    onMessage(cb) { this.msgCbs.push(cb); }
    onStatus(cb) { this.statusCbs.push(cb); cb(this.status); }
    close() { this.closed = true; clearTimeout(this.timer); if (this.ws) this.ws.close(); }
  }

  /* ------------------------------------------------------------------ the game view */
  let MOUNTS = 0;

  class Game {
    constructor(el, opts) {
      this.el = el; this.opts = opts || {};
      this.pfx = 'qc' + (++MOUNTS) + '-';
      this.transport = opts.transport || null;
      this.replayMode = !!opts.replay;
      this.state = null; this.view = null; this.flip = false;
      // Positions of a game in progress (start + one per move), kept up to
      // date from each state message; `null` until loaded.
      this.hist = null; this.histLoading = false;
      this.input = newInput();
      this.hover = null; this.chat = []; this.collapse = null; this.toast = null;
      this.confirmResign = false; this.conn = 'open'; this.offset = 0;
      this.build();
      if (this.transport) {
        this.transport.onMessage((m) => this.handle(m));
        if (this.transport.onStatus) this.transport.onStatus((s) => { this.conn = s; this.renderConn(); });
      }
      this.onKey = (e) => this.key(e);
      document.addEventListener('keydown', this.onKey);
      this.timer = setInterval(() => this.tickClocks(), 100);
      if (opts.state) this.handle(opts.state);
    }

    destroy() {
      clearInterval(this.timer); clearTimeout(this.toastTimer);
      document.removeEventListener('keydown', this.onKey);
      if (this.transport && this.transport.close) this.transport.close();
      this.el.innerHTML = ''; this.el.__qchess = null;
    }

    /* ---- helpers */
    get you() { return this.replayMode ? null : (this.state && this.state.you) || null; }
    get positions() { return (this.state && this.state.positions) || this.hist; }
    get lastIdx() { return this.positions ? this.positions.length - 1 : 0; }
    get viewIdx() { return this.view == null ? this.lastIdx : this.view; }
    get isLive() { return !this.positions || this.view == null || this.view >= this.lastIdx; }
    get ended() { return !!this.state && this.state.status && this.state.status.kind === 'ended'; }
    get canMove() {
      const s = this.state;
      return !!(s && this.you && !this.ended && s.to_move === this.you && this.isLive);
    }
    get flipped() { return (this.you === 'black') !== this.flip; }
    send(obj) {
      if (this.opts.onSend) this.opts.onSend(obj);
      if (!this.transport) return;
      const r = this.transport.send(obj);
      if (r === false) this.showToast('Not connected — reconnecting…');
    }
    showToast(msg) {
      this.toast = msg; this.renderStatus();
      clearTimeout(this.toastTimer);
      this.toastTimer = setTimeout(() => { this.toast = null; this.renderStatus(); }, 5000);
    }

    /* ---- message handling */
    handle(m) {
      if (!m || !m.type) return;
      if (m.type === 'state') {
        this.state = m;
        if (!m.positions) this.trackHistory(m);
        if (m.clock) this.offset = Date.now() - m.clock.server_now_ms;
        this.input = newInput();
        this.collapse = null; this.confirmResign = false;
        if (this.positions && this.view != null && this.view > this.lastIdx) this.view = null;
        this.renderAll();
      } else if (m.type === 'choose_collapse') {
        this.collapse = m; this.input = newInput(); this.renderModal(); this.renderBoard();
      } else if (m.type === 'chat') {
        this.chat.push(m); if (this.chat.length > 200) this.chat.shift(); this.renderChat();
      } else if (m.type === 'error') {
        this.collapse = null; this.input = newInput();
        this.showToast(m.message || 'Error'); this.renderModal(); this.renderBoard();
      }
    }

    /* ---- history of a game in progress */
    trackHistory(m) {
      const n = (m.moves || []).length, cur = { pieces: m.pieces, last_move: m.last_move || [] };
      if (n === 0) this.hist = [cur];
      else if (this.hist && this.hist.length === n) this.hist.push(cur);
      else if (this.hist && this.hist.length === n + 1) this.hist[n] = cur;
      else { this.hist = null; this.loadHistory(); }
    }
    loadHistory() {
      // opts.loadHistory (a function returning a promise of the positions)
      // replaces fetching opts.historyUrl, e.g. for a game run in the browser.
      const url = this.opts.historyUrl, load = this.opts.loadHistory;
      if ((!url || !root || !root.fetch) && !load) return;
      if (this.histLoading) return;
      this.histLoading = true;
      const p0 = load ? load() : root.fetch(url).then((r) => (r.ok ? r.json() : null));
      p0.then((p) => {
        this.histLoading = false;
        const n = this.state && !this.state.positions ? (this.state.moves || []).length : -1;
        if (Array.isArray(p) && p.length === n + 1) {
          this.hist = p;
          this.renderMoves(); this.renderReplay();
        } else if (Array.isArray(p) && p.length < n + 1) {
          this.loadHistory(); // a move arrived meanwhile
        }
      }).catch(() => { this.histLoading = false; });
    }

    /* ---- DOM skeleton */
    build() {
      const el = this.el;
      el.__qchess = this;
      el.classList.add('qc-root');
      el.innerHTML =
        '<div class="qc-boardcol">' +
          '<div class="qc-player qc-top"></div>' +
          '<div class="qc-boardwrap"><svg class="qc-board" viewBox="0 0 800 800" role="grid" aria-label="Chess board"></svg>' +
          '<div class="qc-promo" hidden></div></div>' +
          '<div class="qc-player qc-bottom"></div>' +
          '<div class="qc-bar"></div>' +
        '</div>' +
        '<div class="qc-side">' +
          '<div class="qc-status"></div>' +
          '<div class="qc-moves" aria-label="Moves"></div>' +
          '<div class="qc-replay"></div>' +
          '<div class="qc-buttons"></div>' +
          '<div class="qc-chat"><div class="qc-chatlog" aria-live="polite"></div>' +
          '<form class="qc-chatform"><input type="text" maxlength="140" placeholder="Chat…" aria-label="Chat message" autocomplete="off">' +
          '<button type="submit">Send</button></form></div>' +
        '</div>' +
        '<div class="qc-modal" hidden></div>';
      const q = (s) => el.querySelector(s);
      this.svg = q('.qc-board'); this.promoEl = q('.qc-promo');
      this.topEl = q('.qc-top'); this.botEl = q('.qc-bottom'); this.barEl = q('.qc-bar');
      this.statusEl = q('.qc-status'); this.movesEl = q('.qc-moves'); this.replayEl = q('.qc-replay');
      this.buttonsEl = q('.qc-buttons'); this.chatEl = q('.qc-chat'); this.logEl = q('.qc-chatlog');
      this.modalEl = q('.qc-modal');
      if (this.replayMode) this.chatEl.hidden = true;

      this.svg.addEventListener('click', (e) => {
        const t = e.target.closest('[data-sq]');
        if (t) this.clickSquare(t.getAttribute('data-sq'), e.shiftKey);
      });
      this.svg.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          const t = e.target.closest('[data-sq]');
          if (t) { e.preventDefault(); this.clickSquare(t.getAttribute('data-sq'), e.shiftKey); }
        }
      });
      this.svg.addEventListener('mouseover', (e) => {
        const t = e.target.closest('[data-sq]');
        this.setHover(t ? t.getAttribute('data-sq') : null);
      });
      this.svg.addEventListener('mouseleave', () => this.setHover(null));
      el.addEventListener('click', (e) => {
        const a = e.target.closest('[data-action]');
        if (a) this.action(a.getAttribute('data-action'), a);
      });
      q('.qc-chatform').addEventListener('submit', (e) => {
        e.preventDefault();
        const inp = e.target.querySelector('input');
        const text = inp.value.trim().slice(0, 140);
        if (text) { this.send({ type: 'chat', text }); inp.value = ''; }
      });
      this.svg.addEventListener('click', (e) => { if (e.target === this.svg) this.dispatch({ t: 'cancel' }); });
    }

    renderAll() {
      this.renderBoard(); this.renderPlayers(); this.renderBar(); this.renderStatus();
      this.renderMoves(); this.renderReplay(); this.renderButtons(); this.renderModal(); this.renderChat();
      this.renderConn();
    }

    /* ---- input */
    ctx() {
      const s = this.state;
      return { you: this.you, pieces: s.pieces, targets: s.targets || {} };
    }
    dispatch(a) {
      if (!this.state) return;
      const before = this.input.phase;
      if (a.t !== 'cancel' && !this.canMove) return;
      const r = reduceInput(this.input, a, this.ctx());
      this.input = r.ui;
      if (r.notation) this.sendMove(r.notation);
      this.renderBoard(); this.renderBar();
      if (before !== this.input.phase || a.t === 'cancel') this.renderPromo();
    }
    sendMove(notation) {
      this.pendingNotation = notation;
      this.send({ type: 'move', notation });
    }
    clickSquare(sq, shift) {
      // Looking at an earlier move of your own game: a click on the board
      // brings you back to the current position.
      if (!this.isLive && this.you && !this.ended) { this.gotoPly(this.lastIdx); return; }
      this.dispatch({ t: 'click', sq, shift });
    }
    action(name, node) {
      switch (name) {
        case 'only': this.dispatch({ t: 'only' }); break;
        case 'flip': this.flip = !this.flip; this.renderBoard(); this.renderPlayers(); break;
        case 'promo': this.dispatch({ t: 'promo', p: node.getAttribute('data-p') }); break;
        case 'resign': this.confirmResign = true; this.renderButtons(); break;
        case 'resign-no': this.confirmResign = false; this.renderButtons(); break;
        case 'resign-yes': this.confirmResign = false; this.send({ type: 'resign' }); this.renderButtons(); break;
        case 'abort': this.send({ type: 'abort' }); break;
        case 'draw-offer': this.send({ type: 'draw_offer' }); break;
        case 'draw-accept': this.send({ type: 'draw_accept' }); break;
        case 'draw-decline': this.send({ type: 'draw_decline' }); break;
        case 'collapse': {
          const opt = node.getAttribute('data-opt');
          const n = this.collapse.notation;
          this.collapse = null; this.renderModal();
          this.send({ type: 'move', notation: n + ' ' + opt });
          break;
        }
        case 'collapse-cancel': this.collapse = null; this.renderModal(); break;
        case 'go': this.goto(node.getAttribute('data-to')); break;
        case 'ply': this.gotoPly(parseInt(node.getAttribute('data-ply'), 10)); break;
        default: break;
      }
    }
    key(e) {
      if (!this.state) return;
      if (e.key === 'Escape') {
        if (this.collapse) { this.collapse = null; this.renderModal(); }
        else if (this.input.phase !== 'idle') this.dispatch({ t: 'cancel' });
        return;
      }
      const tag = (e.target && e.target.tagName) || '';
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;
      if (!this.positions || e.altKey || e.ctrlKey || e.metaKey) return;
      if (e.key === 'ArrowLeft') { this.goto('prev'); e.preventDefault(); }
      else if (e.key === 'ArrowRight') { this.goto('next'); e.preventDefault(); }
      else if (e.key === 'ArrowUp' || e.key === 'Home') { this.goto('first'); e.preventDefault(); }
      else if (e.key === 'ArrowDown' || e.key === 'End') { this.goto('last'); e.preventDefault(); }
    }
    goto(where) {
      if (!this.positions) return;
      let i = this.viewIdx;
      if (where === 'first') i = 0; else if (where === 'prev') i--; else if (where === 'next') i++; else i = this.lastIdx;
      this.gotoPly(i);
    }
    gotoPly(i) {
      if (!this.positions) return;
      i = Math.max(0, Math.min(this.lastIdx, i));
      this.view = i >= this.lastIdx ? null : i;
      this.input = newInput();
      this.renderBoard(); this.renderMoves(); this.renderReplay(); this.renderBar(); this.renderPromo();
    }
    setHover(sq) {
      if (sq === this.hover) return;
      this.hover = sq; this.renderHover();
    }

    /* ---- board */
    displayPos() {
      const s = this.state;
      if (this.positions && this.view != null && this.positions[this.view]) {
        const p = this.positions[this.view];
        return { pieces: p.pieces, last: p.last_move || [], live: false };
      }
      return { pieces: s.pieces, last: s.last_move || [], live: true };
    }
    sqXY(sq) {
      const f = sqFile(sq), r = sqRank(sq);
      return this.flipped ? { x: (7 - f) * 100, y: r * 100 } : { x: f * 100, y: (7 - r) * 100 };
    }
    renderBoard() {
      if (!this.state) return;
      const s = this.state, pfx = this.pfx, pos = this.displayPos();
      const map = squareMap(pos.pieces), inp = this.input, tg = s.targets || {};
      const interactive = this.canMove;
      const own = (sq) => interactive && ownHalfAt(s.pieces, this.you, sq);
      let targets = [], extra = {};
      if (interactive) {
        if (inp.phase === 'first') targets = tg[inp.from] || [];
        else if (inp.phase === 'second') targets = secondTargets(inp, this.ctx());
      }
      if (inp.from) extra[inp.from] = 'sel';
      if (inp.to) extra[inp.to] = 'sel2';
      if (inp.from2) extra[inp.from2] = 'sel';
      if (inp.phase === 'second' && inp.other) extra[inp.other] = 'sel';

      let sqs = '', hl = '', coords = '', pcs = '', conn = '', tgt = '';
      for (let f = 0; f < 8; f++) for (let r = 0; r < 8; r++) {
        const sq = FILES[f] + (r + 1), { x, y } = this.sqXY(sq);
        const dark = (f + r) % 2 === 0;
        const mine = own(sq) && (tg[sq] || []).length;
        sqs += '<rect class="qc-sq ' + (dark ? 'dark' : 'light') + (extra[sq] ? ' ' + extra[sq] : '') +
          '" x="' + x + '" y="' + y + '" width="100" height="100" data-sq="' + sq + '" role="gridcell"' +
          ' aria-label="' + esc(describeSquare(sq, map[sq])) + '"' + (mine ? ' tabindex="0"' : '') + '/>';
        if (pos.last.includes(sq)) hl += '<rect class="qc-last" x="' + x + '" y="' + y + '" width="100" height="100"/>';
        if (extra[sq]) hl += '<rect class="qc-' + extra[sq] + '" x="' + x + '" y="' + y + '" width="100" height="100"/>';
      }
      // check marks
      if (pos.live && s.check && s.check !== 'none' && !this.ended) {
        for (const p of s.pieces) if (p.color === s.to_move) {
          for (const h of aliveHalves(p)) if (h.kind === 'king') {
            const { x, y } = this.sqXY(h.square);
            if (s.check === 'full') hl += '<rect x="' + x + '" y="' + y + '" width="100" height="100" fill="url(#' + pfx + 'chk)"/>';
            else hl += '<rect x="' + x + '" y="' + y + '" width="100" height="100" fill="url(#' + pfx + 'qchk)"/>' +
              '<circle class="qc-qcheck" cx="' + (x + 50) + '" cy="' + (y + 50) + '" r="42"/>';
          }
        }
      }
      // coordinates
      for (let i = 0; i < 8; i++) {
        const fsq = this.flipped ? FILES[7 - i] : FILES[i];
        const bottomRank = this.flipped ? 8 : 1;
        const rsq = this.flipped ? i + 1 : 8 - i;
        const fdark = ((FILES.indexOf(fsq)) + bottomRank - 1) % 2 === 0;
        coords += '<text class="qc-coord ' + (fdark ? 'on-dark' : 'on-light') + '" x="' + (i * 100 + 91) + '" y="793" text-anchor="end">' + fsq + '</text>';
        const rdark = ((this.flipped ? 7 : 0) + rsq - 1) % 2 === 0;
        coords += '<text class="qc-coord ' + (rdark ? 'on-dark' : 'on-light') + '" x="6" y="' + (i * 100 + 20) + '">' + rsq + '</text>';
      }
      // targets
      for (const sq of targets) {
        const { x, y } = this.sqXY(sq), occ = map[sq] && map[sq].length;
        tgt += occ ? '<circle class="qc-cap" cx="' + (x + 50) + '" cy="' + (y + 50) + '" r="46"/>'
          : '<circle class="qc-dot" cx="' + (x + 50) + '" cy="' + (y + 50) + '" r="15"/>';
      }
      if (inp.to) { // marker for the first half's destination
        const { x, y } = this.sqXY(inp.to);
        tgt += '<circle class="qc-first" cx="' + (x + 50) + '" cy="' + (y + 50) + '" r="36"/>' +
          '<text class="qc-firstn" x="' + (x + 50) + '" y="' + (y + 64) + '" text-anchor="middle">1</text>';
      }
      if (inp.to2) {
        const { x, y } = this.sqXY(inp.to2);
        tgt += '<circle class="qc-first" cx="' + (x + 50) + '" cy="' + (y + 50) + '" r="36"/>' +
          '<text class="qc-firstn" x="' + (x + 50) + '" y="' + (y + 64) + '" text-anchor="middle">2</text>';
      }
      // pieces
      for (const p of pos.pieces) {
        const al = aliveHalves(p);
        if (al.length === 2) {
          const a = this.sqXY(al[0].square), b = this.sqXY(al[1].square);
          conn += '<line class="qc-link" x1="' + (a.x + 50) + '" y1="' + (a.y + 50) + '" x2="' + (b.x + 50) + '" y2="' + (b.y + 50) + '"/>';
        }
      }
      for (const sq in map) {
        const list = map[sq], { x, y } = this.sqXY(sq), many = list.length > 1;
        list.forEach((e, i) => {
          const q = isQuantum(e.piece);
          const tx = many ? x + (i % 2 ? 44 : 0) : x, ty = many ? y + (i % 2 ? 44 : 0) : y;
          const tr = 'translate(' + tx + ' ' + ty + ')' + (many ? ' scale(.56)' : '');
          const kind = e.half.kind || e.piece.kind;
          const badge = badgeText(e.piece, kind);
          let body;
          if (q) {
            // A quantum half: the left or right half of the icon, over a faint ghost of the whole piece.
            const side = e.piece.halves.indexOf(e.half) === 0 ? 'cl' : 'cr';
            body = '<g class="qc-ghost">' + pieceUse(pfx, e.piece.color, kind) + '</g>' +
              '<g class="qc-body" clip-path="url(#' + pfx + side + ')">' + pieceUse(pfx, e.piece.color, kind) + '</g>' +
              '<line class="qc-cut" x1="50" y1="8" x2="50" y2="90"/>';
          } else {
            body = '<g class="qc-body">' + pieceUse(pfx, e.piece.color, kind) + '</g>';
          }
          if (badge) {
            body += '<g class="qc-badge c-' + e.piece.color + (q ? ' q' : '') + '"><rect x="' + (badge.length > 1 ? 66 : 72) +
              '" y="70" width="' + (badge.length > 1 ? 30 : 24) + '" height="24" rx="12"/>' +
              '<text x="' + (badge.length > 1 ? 81 : 84) + '" y="88" text-anchor="middle">' + esc(badge) + '</text></g>';
          }
          pcs += '<g transform="' + tr + '" class="qc-piece' + (q ? ' q' : '') + '">' + body + '</g>';
        });
      }
      this.svg.innerHTML = defsSvg(pfx) + '<g>' + sqs + '</g><g class="qc-ov">' + hl + '</g><g class="qc-ov">' + conn +
        '</g><g class="qc-ov">' + coords + '</g><g class="qc-ov">' + tgt + '</g><g class="qc-ov">' + pcs +
        '</g><g class="qc-ov qc-hoverg"></g>';
      this.svg.classList.toggle('interactive', interactive);
      this.renderHover();
    }
    renderHover() {
      const g = this.svg.querySelector('.qc-hoverg');
      if (!g || !this.state) return;
      if (!this.hover) { g.innerHTML = ''; return; }
      const pos = this.displayPos(), twins = twinsOf(pos.pieces, this.hover);
      let h = '';
      const a = this.sqXY(this.hover);
      for (const t of twins) {
        const b = this.sqXY(t);
        h += '<line class="qc-hlink" x1="' + (a.x + 50) + '" y1="' + (a.y + 50) + '" x2="' + (b.x + 50) + '" y2="' + (b.y + 50) + '"/>' +
          '<rect class="qc-twin" x="' + b.x + '" y="' + b.y + '" width="100" height="100"/>' +
          '<circle class="qc-twinring" cx="' + (b.x + 50) + '" cy="' + (b.y + 50) + '" r="46"/>';
      }
      g.innerHTML = h;
    }

    /* ---- promotion picker & modal */
    renderPromo() {
      const ph = this.input.phase;
      if (ph !== 'promo1' && ph !== 'promo2') { this.promoEl.hidden = true; this.promoEl.innerHTML = ''; return; }
      const label = ph === 'promo2' ? 'Promote the other half to (the same piece for an ordinary promotion)' : 'Promote this half to';
      let h = '<div class="qc-promo-t">' + label + '</div><div class="qc-promo-row">';
      for (const [p, k] of [['Q', 'queen'], ['R', 'rook'], ['B', 'bishop'], ['N', 'knight']]) {
        h += '<button type="button" data-action="promo" data-p="' + p + '" aria-label="Promote to ' + k + '">' +
          '<svg viewBox="0 0 100 100" aria-hidden="true">' + pieceUse(this.pfx, this.you, k) + '</svg></button>';
      }
      this.promoEl.innerHTML = h + '</div>';
      this.promoEl.hidden = false;
    }
    renderModal() {
      const c = this.collapse;
      if (!c) { this.modalEl.hidden = true; this.modalEl.innerHTML = ''; return; }
      let h = '<div class="qc-dialog" role="dialog" aria-modal="true" aria-label="Choose measurement outcome">' +
        '<h3>Measurement</h3><p>This move forces a measurement. Which outcome do you want to continue with?</p>' +
        '<p class="qc-mono">' + esc(c.notation) + '</p><div class="qc-opts">';
      for (const o of c.options || []) {
        h += '<button type="button" data-action="collapse" data-opt="' + esc(o) + '"><span>' +
          esc(describeOption(o, this.state ? squareMap(this.state.pieces) : null)) + '</span><code>' + esc(o) + '</code></button>';
      }
      this.modalEl.innerHTML = h + '</div><button type="button" class="qc-link-btn" data-action="collapse-cancel">Cancel</button></div>';
      this.modalEl.hidden = false;
      const b = this.modalEl.querySelector('button'); if (b) b.focus();
    }

    /* ---- panels */
    renderBar() {
      if (!this.state) return;
      const inp = this.input, mine = this.canMove;
      let h = '';
      if (mine && onlyThisHalf(inp, this.ctx())) {
        h += '<button type="button" data-action="only" class="qc-only" title="Legal, but it proves the piece was on ' +
          esc(inp.from) + ', so it collapses there">Move only this half (collapses it to ' + esc(inp.from) + ')</button>';
      }
      let hint = '';
      if (!this.isLive) hint = 'Viewing move ' + this.viewIdx + ' of ' + this.lastIdx;
      else if (mine) {
        const castle = inp.phase === 'second' && isCastle(inp);
        hint = ({
          idle: 'Your move: pick a piece',
          first: inp.other ? 'Where does the half on ' + inp.from + ' go?' : 'Where does one half go?',
          second: castle ? 'Same square to castle, or the king\'s square to quantum castle (half the king stays)'
            : inp.other ? 'Where does the other half (' + inp.other + ') go? The same square rejoins them'
            : 'Where does the other half go? The same square is an ordinary move',
          promo1: 'Choose a promotion piece', promo2: 'Choose a promotion piece',
        })[inp.phase] || '';
      } else if (this.you && !this.ended) hint = 'Opponent to move';
      h += '<span class="qc-hint">' + esc(hint) + '</span>';
      h += '<button type="button" data-action="flip" class="qc-flip" aria-label="Flip board">⇅ Flip</button>';
      this.barEl.innerHTML = h;
    }
    renderPlayers() {
      if (!this.state) return;
      const s = this.state;
      const top = this.flipped ? 'white' : 'black', bot = this.flipped ? 'black' : 'white';
      const one = (color) => {
        const p = (s.players && s.players[color]) || {};
        const name = p.username
          ? '<a class="qc-name" href="/@/' + encodeURIComponent(p.username) + '">' + esc(p.username) + '</a>'
          : '<span class="qc-name anon">Anonymous</span>';
        const rating = p.rating != null ? '<span class="qc-rating">' + esc(p.rating) + (p.provisional ? '?' : '') + '</span>' : '';
        const turn = !this.ended && s.to_move === color;
        return '<span class="qc-chip ' + color + '" title="' + color + '"></span>' + name + rating +
          (turn ? '<span class="qc-turn" title="To move"></span>' : '') +
          '<span class="qc-clock" data-clock="' + color + '"></span>';
      };
      this.topEl.innerHTML = one(top); this.botEl.innerHTML = one(bot);
      this.topEl.dataset.color = top; this.botEl.dataset.color = bot;
      this.topEl.classList.toggle('turn', !this.ended && s.to_move === top);
      this.botEl.classList.toggle('turn', !this.ended && s.to_move === bot);
      this.tickClocks();
    }
    tickClocks() {
      const s = this.state;
      if (!s) return;
      this.el.querySelectorAll('[data-clock]').forEach((n) => {
        const c = n.getAttribute('data-clock'), ms = clockMs(s.clock, c, Date.now(), this.offset);
        const t = formatClock(ms);
        if (n.textContent !== t) n.textContent = t;
        n.classList.toggle('low', ms != null && ms < 10000 && !this.ended);
        n.classList.toggle('run', !!s.clock && s.clock.running === c);
      });
    }
    renderConn() {
      this.renderStatus();
    }
    renderStatus() {
      if (!this.state) { this.statusEl.innerHTML = '<div class="qc-line">Connecting…</div>'; return; }
      const s = this.state;
      let h = '';
      const banner = resultBanner(s.status);
      if (banner) h += '<div class="qc-banner">' + esc(banner) + '<small>' + esc(resultText(s.status)) + '</small></div>';
      else {
        const who = s.you && s.to_move === s.you ? 'Your turn' : (s.to_move === 'white' ? 'White' : 'Black') + ' to move';
        h += '<div class="qc-line qc-turnline' + (s.you && s.to_move === s.you ? ' mine' : '') + '">' + who + '</div>';
        if (s.check === 'quantum') h += '<div class="qc-line qc-check q">Check?<small>a king may be attacked in some boards</small></div>';
        else if (s.check === 'full') h += '<div class="qc-line qc-check">Check!</div>';
      }
      if (s.state_count > 1) h += '<div class="qc-line qc-super"><i></i>Superposition: ' + s.state_count + ' boards</div>';
      if (s.draw_offer && !this.ended) h += '<div class="qc-line">Draw offered by ' + s.draw_offer + '</div>';
      if (this.toast) h += '<div class="qc-toast" role="alert">' + esc(this.toast) + '</div>';
      if (!this.replayMode && this.conn && this.conn !== 'open')
        h += '<div class="qc-line qc-offline"><i></i>' + (this.conn === 'connecting' ? 'Connecting…' : 'Reconnecting…') + '</div>';
      this.statusEl.innerHTML = h;
    }
    renderMoves() {
      if (!this.state) return;
      const rows = groupMoves(this.state.moves), cur = this.positions ? this.viewIdx : (this.state.moves || []).length;
      const cell = (m) => {
        if (!m) return '<span class="qc-mv empty"></span>';
        const cl = (m.collapses || []).length
          ? '<small class="qc-coll">' + esc(m.collapses.join(' ')) + '</small>' : '';
        return '<span class="qc-mv' + (m.ply === cur ? ' cur' : '') + '" data-action="ply" data-ply="' + m.ply +
          '" role="button" tabindex="0" title="' + esc(m.notation) + '">' + esc(m.display || m.notation) + cl + '</span>';
      };
      let h = '';
      for (const r of rows) h += '<span class="qc-n">' + r.n + '</span>' + cell(r.w) + cell(r.b);
      this.movesEl.innerHTML = h || '<span class="qc-empty">No moves yet</span>';
      if (this.view == null) this.movesEl.scrollTop = this.movesEl.scrollHeight;
    }
    renderReplay() {
      const show = !!this.positions && this.lastIdx > 0;
      this.replayEl.hidden = !show;
      this.replayEl.classList.toggle('behind', !this.isLive && !this.ended && !this.replayMode);
      if (!show) { this.replayEl.innerHTML = ''; return; }
      const i = this.viewIdx, last = this.lastIdx;
      const b = (to, glyph, label, dis) => '<button type="button" data-action="go" data-to="' + to + '" aria-label="' + label + '"' + (dis ? ' disabled' : '') + '>' + glyph + '</button>';
      this.replayEl.innerHTML = b('first', '⏮', 'First position', i === 0) + b('prev', '◀', 'Previous move', i === 0) +
        '<span class="qc-ply">' + i + ' / ' + last + '</span>' + b('next', '▶', 'Next move', i === last) + b('last', '⏭', 'Last position', i === last);
    }
    renderButtons() {
      const s = this.state;
      if (!s || !this.you || this.ended || this.replayMode) { this.buttonsEl.innerHTML = ''; return; }
      let h = '';
      const nm = (s.moves || []).length;
      if (nm < 2) h += '<button type="button" data-action="abort">Abort</button>';
      else if (this.confirmResign) h += '<span class="qc-confirm">Resign?</span><button type="button" class="danger" data-action="resign-yes">Yes, resign</button><button type="button" data-action="resign-no">Cancel</button>';
      else h += '<button type="button" data-action="resign" class="danger-o">Resign</button>';
      if (s.draw_offer && s.draw_offer !== this.you) {
        h += '<button type="button" class="good" data-action="draw-accept">Accept draw</button><button type="button" data-action="draw-decline">Decline</button>';
      } else if (s.draw_offer === this.you) h += '<button type="button" disabled>Draw offered</button>';
      else if (nm >= 2) h += '<button type="button" data-action="draw-offer">Offer draw</button>';
      this.buttonsEl.innerHTML = h;
    }
    renderChat() {
      this.logEl.innerHTML = this.chat.map((m) => '<div><b>' + esc(m.user) + '</b> ' + esc(m.text) + '</div>').join('');
      this.logEl.scrollTop = this.logEl.scrollHeight;
    }
  }

  /* ------------------------------------------------------------------ mounting */
  function mount(el, opts) {
    opts = opts || {};
    const g = new Game(el, opts);
    return {
      game: g,
      handle: (m) => g.handle(m),
      getState: () => g.state,
      destroy: () => g.destroy(),
    };
  }
  function autoMount(el) {
    const d = el.dataset;
    if (d.replayUrl) {
      const m = mount(el, { replay: true });
      fetch(d.replayUrl).then((r) => r.json()).then((j) => { j.type = 'state'; j.you = j.you || null; m.handle(j); })
        .catch((e) => { el.querySelector('.qc-status').textContent = 'Could not load replay: ' + e.message; });
      return m;
    }
    const transport = new GameClient(d.wsUrl || ('/game/' + d.gameId + '/ws'));
    // If the live game is gone (finished and archived, or the server restarted),
    // stop retrying and reload into replay mode.
    let fails = 0;
    transport.onStatus((st) => {
      if (st === 'open') { fails = 0; return; }
      if (st !== 'reconnecting' || ++fails < 3 || !d.gameId || !root.fetch) return;
      root.fetch('/api/game/' + encodeURIComponent(d.gameId) + '/state')
        .then((r) => (r.ok ? r.json() : null))
        .then((j) => { if (j && j.status && j.status.kind === 'ended') { transport.close(); root.location.reload(); } })
        .catch(() => {});
    });
    const historyUrl = d.gameId ? '/api/game/' + encodeURIComponent(d.gameId) + '/positions' : null;
    return mount(el, { transport, historyUrl });
  }

  return { mount, autoMount, GameClient, buildNotation, reduceInput, newInput, secondTargets, ownHalfAt, twinsOf, squareMap,
    describeOption, badgeText, pieceName, clockMs, formatClock, groupMoves, resultBanner, statusLine, describeSquare, needsPromo,
    kindLetter, KIND_FROM_LETTER };
});
