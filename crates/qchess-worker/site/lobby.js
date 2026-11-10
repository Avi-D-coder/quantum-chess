/* Lobby: quick pairing, friend challenges, games against the computer
 * (which run in this browser) and the list of live games. Updates arrive
 * over one WebSocket; all dynamic text goes through textContent. */
(function () {
  'use strict';
  const { el, store } = window.QC;
  const root = document.getElementById('lobby');
  if (!root) return;

  const saved = (k, d) => store.get('qc_lobby_' + k) || d;
  let mode = saved('mode', 'seek');
  let level = saved('level', 'medium');
  let colorChoice = saved('color', 'random');
  let data = { seeks: [], games: [], tcs: [], online: 0 };
  let connected = false;
  let message = '';
  const BOT_TCS = ['1+0', '3+2', '5+3', '10+5', '15+10', '30+0'];

  const sock = window.QC.socket('/ws/lobby', (m) => {
    if (m.type === 'lobby') { data = m; connected = true; if (!data.seeks.some((s) => s.mine) && message.startsWith('Waiting')) message = ''; render(); }
    else if (m.type === 'matched') location.href = '/game/' + m.game;
    else if (m.type === 'challenge') location.href = '/challenge/' + m.token;
    else if (m.type === 'error') { message = m.message; render(); }
  });

  const box = el('div', { class: 'lobby-grid' });
  root.append(box);

  function remember(k, v) { store.set('qc_lobby_' + k, v); }

  function render() {
    box.textContent = '';
    box.append(createPanel(), el('div', { class: 'lobby-side' }, [seeksPanel(), gamesPanel()]));
  }

  function picker(id, label, value, options, onchange) {
    const sel = el('select', { id, onchange: () => onchange(sel.value) }, options.map(([v, t]) => el('option', { value: v, text: t })));
    sel.value = value;
    return el('label', {}, [document.createTextNode(label + ' '), sel]);
  }

  function createPanel() {
    const modeBtn = (m, label) => el('button', {
      type: 'button', class: 'tab' + (mode === m ? ' on' : ''), text: label,
      onclick: () => { mode = m; remember('mode', m); message = ''; render(); },
    });
    const tcs = mode === 'bot' ? BOT_TCS : data.tcs;
    const grid = el('div', { class: 'tc-grid' });
    for (const tc of tcs) {
      grid.append(el('button', {
        type: 'button', class: 'tc', 'data-tc': tc, text: tc,
        onclick: () => {
          if (mode === 'bot') {
            const q = new URLSearchParams({ new: '1', tc, level, color: colorChoice });
            location.href = '/local?' + q;
          } else if (!connected) {
            message = 'Connecting to the server…'; render();
          } else if (mode === 'friend') {
            sock.send({ type: 'challenge', tc, color: colorChoice });
          } else {
            sock.send({ type: 'seek', tc, color: colorChoice });
            message = 'Waiting for an opponent (' + tc + ')…'; render();
          }
        },
      }));
    }
    if (!tcs.length) grid.append(el('p', { class: 'muted', text: 'Connecting…' }));
    const opts = [picker('opt-color', 'Play as', colorChoice, [['random', 'Random'], ['white', 'White'], ['black', 'Black']],
      (v) => { colorChoice = v; remember('color', v); })];
    if (mode === 'bot') {
      opts.unshift(picker('opt-level', 'Level', level, [['easy', 'Easy'], ['medium', 'Medium'], ['hard', 'Hard']],
        (v) => { level = v; remember('level', v); }));
    }
    const hint = {
      seek: 'Anyone in the lobby can take your game.',
      friend: 'You get a link to send to a friend.',
      bot: 'The computer runs in your browser, so this works offline too.',
    }[mode];
    return el('section', { class: 'card' }, [
      el('h2', { text: 'Play' }),
      el('div', { class: 'tabs' }, [modeBtn('seek', 'Quick pairing'), modeBtn('friend', 'Challenge a friend'), modeBtn('bot', 'Play the computer')]),
      el('div', { class: 'opts' }, opts),
      el('p', { class: 'muted small', text: hint }),
      grid,
      message ? el('p', { class: 'notice', text: message }) : el('span'),
    ]);
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
