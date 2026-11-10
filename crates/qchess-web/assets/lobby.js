/* Lobby (seeks, live games, friend challenges) and the challenge waiting page.
 * Plain ES2020; all dynamic text goes through textContent. */
(function () {
  'use strict';

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

  async function post(url, body) {
    const r = await fetch(url, {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body || {}),
    });
    if (!r.ok) {
      let msg = await r.text();
      if (r.status === 401) msg = 'Sign in first.';
      else if (r.status === 403) msg = 'Choose a username first.';
      throw new Error(msg.replace(/<[^>]*>/g, '').trim() || ('Error ' + r.status));
    }
    return r.json();
  }

  async function poll() {
    const r = await fetch('/api/lobby', { headers: { accept: 'application/json' } });
    if (!r.ok) throw new Error('lobby unavailable');
    return r.json();
  }

  /* ---------------------------------------------------------------- lobby page */
  function mountLobby(root) {
    const user = root.dataset.user || '';
    const canPlay = root.dataset.canPlay === 'true';
    let mode = 'seek';
    let level = 'medium';
    // Kept outside the form: the panel is rebuilt on every lobby poll.
    let colorChoice = 'random';
    let ratedChoice = canPlay;
    let data = { seeks: [], games: [], tcs: [] };
    let message = '';

    const box = el('div', { class: 'lobby-grid' });
    root.append(box);

    function render() {
      box.textContent = '';
      box.append(createPanel(), el('div', { class: 'lobby-side' }, [seeksPanel(), gamesPanel()]));
    }

    function createPanel() {
      const rated = el('input', { type: 'checkbox', id: 'opt-rated', onchange: () => { ratedChoice = rated.checked; } });
      rated.checked = ratedChoice;
      rated.disabled = !canPlay;
      const color = el('select', { id: 'opt-color', onchange: () => { colorChoice = color.value; } }, [
        el('option', { value: 'random', text: 'Random' }),
        el('option', { value: 'white', text: 'White' }),
        el('option', { value: 'black', text: 'Black' }),
      ]);
      color.value = colorChoice;
      const modeBtn = (m, label) => el('button', {
        type: 'button', class: 'tab' + (mode === m ? ' on' : ''), text: label,
        onclick: () => { mode = m; render(); },
      });
      const grid = el('div', { class: 'tc-grid' });
      for (const tc of data.tcs) {
        grid.append(el('button', {
          type: 'button', class: 'tc', 'data-tc': tc, text: tc,
          onclick: async () => {
            if (!canPlay) { message = user ? 'Choose a username first.' : 'Sign in to play.'; render(); return; }
            try {
              if (mode === 'bot') {
                const r = await post('/api/bot', { tc, level, color: color.value });
                location.href = '/game/' + r.game;
              } else if (mode === 'friend') {
                const r = await post('/api/challenge', { tc, rated: rated.checked, color: color.value });
                location.href = r.url;
              } else {
                await post('/api/seek', { tc, rated: rated.checked, color: color.value });
                message = 'Waiting for an opponent (' + tc + ')...';
                refresh();
              }
            } catch (e) { message = e.message; render(); }
          },
        }, [el('small', { text: '' })]));
      }
      return el('section', { class: 'card' }, [
        el('h2', { text: 'Play' }),
        el('div', { class: 'tabs' }, [
          modeBtn('seek', 'Quick pairing'), modeBtn('friend', 'Challenge a friend'), modeBtn('bot', 'Play the computer'),
        ]),
        el('div', { class: 'opts' }, [
          mode === 'bot' ? levelPicker() : el('label', {}, [rated, document.createTextNode(' Rated')]),
          el('label', {}, [document.createTextNode('Play as '), color]),
        ]),
        mode === 'bot' ? el('p', { class: 'muted small', text: 'Games against the computer are casual.' }) : el('span'),
        grid,
        message ? el('p', { class: 'notice', text: message }) : el('span'),
        canPlay ? el('span') : el('p', { class: 'muted' }, [
          user ? el('a', { href: '/welcome', text: 'Choose a username to play.' })
               : el('a', { href: '/login', text: 'Sign in to play.' }),
        ]),
      ]);
    }

    function levelPicker() {
      const sel = el('select', { id: 'opt-level', onchange: () => { level = sel.value; } }, [
        el('option', { value: 'easy', text: 'Easy' }),
        el('option', { value: 'medium', text: 'Medium' }),
        el('option', { value: 'hard', text: 'Hard' }),
      ]);
      sel.value = level;
      return el('label', {}, [document.createTextNode('Level '), sel]);
    }

    function seeksPanel() {
      const body = el('tbody');
      for (const s of data.seeks) {
        const act = s.mine
          ? el('button', {
              type: 'button', class: 'btn small danger', text: 'Cancel',
              onclick: async (ev) => { ev.stopPropagation(); await post('/api/seek/cancel', { id: s.id }).catch(() => {}); message = ''; refresh(); },
            })
          : el('span', { class: 'muted', text: 'Play' });
        const tr = el('tr', { class: s.mine ? 'mine' : 'open', 'data-seek': s.id }, [
          el('td', { text: s.user }), el('td', { text: s.rating == null ? '?' : String(s.rating) }),
          el('td', { text: s.tc }), el('td', { text: s.rated ? 'Rated' : 'Casual' }),
          el('td', { text: s.color }), el('td', {}, [act]),
        ]);
        if (!s.mine) {
          tr.addEventListener('click', async () => {
            if (!canPlay) { message = user ? 'Choose a username first.' : 'Sign in to play.'; render(); return; }
            try { const r = await post('/api/seek/accept', { id: s.id }); location.href = '/game/' + r.game; }
            catch (e) { message = e.message; refresh(); }
          });
        }
        body.append(tr);
      }
      const table = el('table', { class: 'games' }, [
        el('thead', {}, [el('tr', {}, ['Player', 'Rating', 'Time', 'Mode', 'Color', ''].map((h) => el('th', { text: h })))]),
        body,
      ]);
      return el('section', { class: 'card' }, [
        el('h2', { text: 'Open games' }),
        data.seeks.length ? table : el('p', { class: 'muted', text: 'No one is waiting. Create a game!' }),
      ]);
    }

    function gamesPanel() {
      const list = el('div', { class: 'live-list' });
      for (const g of data.games) {
        list.append(el('a', { class: 'live', href: '/game/' + g.id }, [
          el('span', { text: g.white + ' (' + g.white_rating + ')' }),
          el('span', { class: 'muted', text: ' vs ' }),
          el('span', { text: g.black + ' (' + g.black_rating + ')' }),
          el('small', { text: ' ' + g.tc + (g.rated ? ' rated' : ' casual') }),
        ]));
      }
      return el('section', { class: 'card' }, [
        el('h2', {}, [document.createTextNode('Live games '), el('a', { class: 'muted small', href: '/tv', text: 'TV' })]),
        data.games.length ? list : el('p', { class: 'muted', text: 'No games in progress.' }),
      ]);
    }

    async function refresh() {
      try {
        data = await poll();
        if (data.matched) { location.href = '/game/' + data.matched; return; }
        if (!data.seeks.some((s) => s.mine) && message.startsWith('Waiting')) message = '';
        render();
      } catch (e) { /* offline: keep the last view */ }
    }
    render();
    refresh();
    setInterval(refresh, 2000);
  }

  /* ---------------------------------------------------------------- challenge wait */
  function mountChallengeWait() {
    async function tick() {
      try {
        const d = await poll();
        if (d.matched) location.href = '/game/' + d.matched;
      } catch (e) { /* retry */ }
    }
    tick();
    setInterval(tick, 2000);
  }

  const lobby = document.getElementById('lobby');
  if (lobby) mountLobby(lobby);
  if (document.getElementById('challenge-wait')) mountChallengeWait();
})();
