/* Lobby: quick pairing, friend challenges, games against the computer
 * (which run in this browser) and the list of live games. Updates arrive
 * over one WebSocket; all dynamic text goes through textContent. */
(function () {
  'use strict';
  const { el, store } = window.QC;
  const root = document.getElementById('lobby');
  if (!root) return;

  const saved = (k, d) => store.get('qc_lobby_' + k) || d;
  const remember = (k, v) => store.set('qc_lobby_' + k, v);
  let data = { seeks: [], games: [], tcs: [], online: 0 };
  let connected = false;
  let message = '';
  const BOT_TCS = ['1+0', '3+2', '5+3', '10+5', '15+10', '30+0'];

  const sock = window.QC.socket('/ws/lobby', (m) => {
    if (m.type === 'lobby') {
      data = m; connected = true;
      if (!data.seeks.some((s) => s.mine) && message.startsWith('Waiting')) message = '';
      render();
    } else if (m.type === 'matched') location.href = '/game/' + m.game;
    else if (m.type === 'challenge') location.href = '/challenge/' + m.token;
    else if (m.type === 'error') { message = m.message; render(); }
  });

  // The Play panel is rebuilt only when what it shows changes, never just
  // because a lobby update arrived: replacing a button between mousedown and
  // mouseup swallows the click.
  const createBox = el('div');
  const sideBox = el('div', { class: 'lobby-side' });
  root.append(el('div', { class: 'lobby-grid' }, [createBox, sideBox]));
  let createKey = null;

  function render() {
    const mine = data.seeks.find((s) => s.mine);
    const key = JSON.stringify([message, data.tcs, mine && mine.id]);
    if (key !== createKey) {
      createKey = key;
      createBox.textContent = '';
      createBox.append(createPanel(mine));
    }
    sideBox.textContent = '';
    sideBox.append(seeksPanel(), gamesPanel());
  }

  /* Play card: the time grid pairs you with a person; the two buttons below
   * open a setup dialog for the computer or a friend. */
  function createPanel(mine) {
    const grid = el('div', { class: 'tc-grid' });
    for (const tc of data.tcs) {
      grid.append(el('button', {
        type: 'button', class: 'tc', 'data-tc': tc, text: tc,
        onclick: () => {
          sock.send({ type: 'seek', tc, color: 'random' });
          message = 'Waiting for an opponent (' + tc + ')…'; render();
        },
      }));
    }
    if (!data.tcs.length) grid.append(el('p', { class: 'muted', text: 'Connecting to the server…' }));
    let notice = el('span');
    if (message) {
      notice = el('p', { class: 'notice' }, [document.createTextNode(message + ' ')]);
      if (mine && message.startsWith('Waiting')) {
        notice.append(el('button', { type: 'button', class: 'linklike', text: 'Cancel',
          onclick: () => { sock.send({ type: 'cancel', id: mine.id }); message = ''; render(); } }));
      }
    }
    return el('section', { class: 'card' }, [
      el('h2', { text: 'Play someone' }),
      el('p', { class: 'muted small', text: 'Pick a time control and you are paired with the next person who does.' }),
      grid,
      notice,
      el('div', { class: 'play-alt' }, [
        el('button', { type: 'button', class: 'btn', id: 'play-computer', text: 'Play the computer', onclick: () => openSetup('bot') }),
        el('button', { type: 'button', class: 'btn alt', id: 'play-friend', text: 'Challenge a friend', onclick: () => openSetup('friend') }),
      ]),
    ]);
  }

  /* A row of toggle buttons; returns the element and calls onpick(value). */
  function segmented(label, options, value, onpick, wide) {
    const row = el('div', { class: 'seg-row' + (wide ? ' wide' : ''), role: 'group', 'aria-label': label });
    const btns = options.map(([v, t]) => {
      const b = el('button', { type: 'button', class: 'seg', 'data-value': v, text: t, 'aria-pressed': String(v === value) });
      b.addEventListener('click', () => {
        btns.forEach((x) => x.setAttribute('aria-pressed', String(x === b)));
        onpick(v);
      });
      return b;
    });
    row.append(...btns);
    return el('div', { class: 'seg-field' }, [el('div', { class: 'seg-label', text: label }), row]);
  }

  function openSetup(kind) {
    if (document.querySelector('dialog.setup')) return;
    const bot = kind === 'bot';
    const tcs = bot ? BOT_TCS : (data.tcs.length ? data.tcs : BOT_TCS);
    let tc = saved(kind + '_tc', bot ? '5+3' : '10+0');
    if (!tcs.includes(tc)) tc = tcs[0];
    let color = saved('color', 'random');
    let level = saved('level', 'medium');
    const dlg = el('dialog', { class: 'setup', 'aria-labelledby': 'setup-title' });
    const close = () => { dlg.close(); dlg.remove(); };
    const start = () => {
      remember(kind + '_tc', tc); remember('color', color);
      if (bot) {
        remember('level', level);
        location.href = '/local?' + new URLSearchParams({ new: '1', tc, level, color });
      } else if (!connected) {
        status.textContent = 'Still connecting to the server, try again in a moment.';
      } else {
        sock.send({ type: 'challenge', tc, color });
        status.textContent = 'Creating the link…';
      }
    };
    const status = el('p', { class: 'muted small' });
    dlg.append(
      el('h2', { id: 'setup-title', text: bot ? 'Play the computer' : 'Challenge a friend' }),
      bot ? segmented('Level', [['easy', 'Easy'], ['medium', 'Medium'], ['hard', 'Hard']], level, (v) => { level = v; }) : el('span'),
      segmented('Time', tcs.map((t) => [t, t]), tc, (v) => { tc = v; }, true),
      segmented('You play', [['white', 'White'], ['random', 'Random'], ['black', 'Black']], color, (v) => { color = v; }),
      el('p', { class: 'muted small', text: bot
        ? 'The computer runs in your browser, so this works offline too.'
        : 'You get a link to send to your friend. The game starts when they open it.' }),
      status,
      el('div', { class: 'setup-actions' }, [
        el('button', { type: 'button', class: 'btn alt', text: 'Cancel', onclick: close }),
        el('button', { type: 'button', class: 'btn', id: 'setup-start', text: bot ? 'Start game' : 'Create link', onclick: start }),
      ]),
    );
    dlg.addEventListener('click', (e) => { if (e.target === dlg) close(); }); // click on the backdrop
    dlg.addEventListener('close', () => dlg.remove());
    document.body.append(dlg);
    dlg.showModal();
    dlg.querySelector('#setup-start').focus();
  }

  function seeksPanel() {
    const body = el('tbody');
    for (const s of data.seeks) {
      const act = s.mine
        ? el('button', {
            type: 'button', class: 'btn small danger', text: 'Cancel',
            onclick: (ev) => { ev.stopPropagation(); sock.send({ type: 'cancel', id: s.id }); message = ''; },
          })
        : el('span', { class: 'muted', text: 'Play' });
      const tr = el('tr', { class: s.mine ? 'mine' : 'open', 'data-seek': s.id }, [
        el('td', { text: s.user }), el('td', { text: s.tc }), el('td', { text: s.color }), el('td', {}, [act]),
      ]);
      if (!s.mine) tr.addEventListener('click', () => sock.send({ type: 'accept', id: s.id }));
      body.append(tr);
    }
    const table = el('table', { class: 'games' }, [
      el('thead', {}, [el('tr', {}, ['Player', 'Time', 'Color', ''].map((h) => el('th', { text: h })))]),
      body,
    ]);
    const online = connected ? data.online + (data.online === 1 ? ' person' : ' people') + ' here' : 'connecting…';
    return el('section', { class: 'card' }, [
      el('h2', {}, [document.createTextNode('Open games'), el('span', { class: 'muted small online', text: online })]),
      data.seeks.length ? table : el('p', { class: 'muted', text: 'No one is waiting. Create a game!' }),
    ]);
  }

  function gamesPanel() {
    const list = el('div', { class: 'live-list' });
    for (const g of data.games) {
      list.append(el('a', { class: 'live', href: '/game/' + g.id }, [
        el('span', { text: g.white }), el('span', { class: 'muted', text: ' vs ' }), el('span', { text: g.black }),
        el('small', { class: 'muted', text: ' · ' + g.tc }),
      ]));
    }
    return el('section', { class: 'card' }, [
      el('h2', { text: 'Live games' }),
      data.games.length ? list : el('p', { class: 'muted', text: 'No games in progress.' }),
    ]);
  }

  render();
})();
