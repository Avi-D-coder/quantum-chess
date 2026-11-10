/* Shared by every page: who you are (no accounts), the name editor in the
 * header, and a reconnecting JSON WebSocket. Plain ES2020. */
(function () {
  'use strict';

  const store = {
    get(k) { try { return localStorage.getItem(k); } catch (e) { return null; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch (e) { /* private mode: this page only */ } },
    del(k) { try { localStorage.removeItem(k); } catch (e) { /* ignore */ } },
  };

  function randomToken(n) {
    const abc = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
    const a = new Uint8Array(n);
    crypto.getRandomValues(a);
    return Array.from(a, (b) => abc[b & 63]).join('');
  }

  // A secret that identifies this browser to the server. Whoever holds it can
  // move your pieces, so it never leaves your browser except to the server.
  let pid = store.get('qc_pid');
  if (!pid || pid.length < 16) { pid = randomToken(24); store.set('qc_pid', pid); }

  function cleanName(s) {
    return String(s || '').replace(/[^\p{L}\p{N} _-]/gu, '').replace(/\s+/g, ' ').trim().slice(0, 20);
  }
  let name = cleanName(store.get('qc_name'));
  if (!name) { name = 'Player' + (100 + Math.floor(Math.random() * 900)); store.set('qc_name', name); }

  function wsUrl(path) {
    return path + (path.includes('?') ? '&' : '?') + 'pid=' + encodeURIComponent(pid) + '&name=' + encodeURIComponent(name);
  }

  /* Header: "Playing as NAME (change)". A new name applies after a reload. */
  function renderMe() {
    const me = document.getElementById('me');
    if (!me) return;
    me.textContent = 'Playing as ';
    const b = document.createElement('b'); b.textContent = name;
    const edit = document.createElement('button');
    edit.type = 'button'; edit.className = 'linklike'; edit.textContent = 'change';
    edit.addEventListener('click', () => {
      me.textContent = '';
      const inp = document.createElement('input');
      inp.type = 'text'; inp.value = name; inp.maxLength = 20; inp.setAttribute('aria-label', 'Your name');
      const done = () => {
        const n = cleanName(inp.value);
        if (n && n !== name) { store.set('qc_name', n); location.reload(); } else renderMe();
      };
      inp.addEventListener('keydown', (e) => { if (e.key === 'Enter') done(); else if (e.key === 'Escape') renderMe(); });
      inp.addEventListener('blur', done);
      me.append(inp); inp.focus(); inp.select();
    });
    me.append(b, ' ', edit);
  }

  /* A JSON WebSocket that reconnects with backoff. onOpen runs on every
   * (re)connect, so callers can resend whatever they need. */
  function socket(path, onMessage, onOpen) {
    let ws = null, attempt = 0;
    function connect() {
      const proto = location.protocol === 'https:' ? 'wss://' : 'ws://';
      ws = new WebSocket(proto + location.host + wsUrl(path));
      ws.onopen = () => { attempt = 0; if (onOpen) onOpen(); };
      ws.onmessage = (ev) => { let m; try { m = JSON.parse(ev.data); } catch (e) { return; } onMessage(m); };
      ws.onclose = () => setTimeout(connect, Math.min(10000, 500 * 2 ** attempt++) + Math.random() * 250);
    }
    connect();
    return {
      send(obj) { if (ws && ws.readyState === 1) { ws.send(JSON.stringify(obj)); return true; } return false; },
    };
  }

  function el(tag, props, children) {
    const e = document.createElement(tag);
    for (const k in props || {}) {
      if (k === 'text') e.textContent = props[k];
      else if (k === 'class') e.className = props[k];
      else if (k.startsWith('on')) e.addEventListener(k.slice(2), props[k]);
      else e.setAttribute(k, props[k]);
    }
    for (const c of children || []) e.append(c);
    return e;
  }

  // Show script errors on the page instead of failing silently.
  function showError(text) {
    let bar = document.getElementById('qc-error');
    if (!bar) {
      bar = el('div', { id: 'qc-error', class: 'qc-error-bar', role: 'alert' });
      document.body.prepend(bar);
    }
    bar.textContent = 'Something went wrong: ' + text;
  }
  window.addEventListener('error', (e) => showError((e.message || 'script error') + (e.filename ? ' (' + e.filename.split('/').pop() + ':' + e.lineno + ')' : '')));
  window.addEventListener('unhandledrejection', (e) => showError(String((e.reason && e.reason.message) || e.reason)));

  window.QC = { pid, get name() { return name; }, wsUrl, socket, el, store };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', renderMe); else renderMe();
})();
