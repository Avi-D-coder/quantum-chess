/* A friend challenge: the creator waits here with the link; the friend
 * accepts here. Both are sent to the game when it starts. */
(function () {
  'use strict';
  const { el } = window.QC;
  const token = decodeURIComponent(location.pathname.split('/')[2] || '');
  const box = document.getElementById('challenge');

  const sock = window.QC.socket('/ws/lobby', (m) => {
    if (m.type === 'challenge_info') render(m);
    else if (m.type === 'matched') location.href = '/game/' + m.game;
    else if (m.type === 'challenge_cancelled') location.href = '/';
    else if (m.type === 'error') show([el('h1', { text: 'Challenge' }), el('p', { class: 'error', text: m.message }), back()]);
  }, () => sock.send({ type: 'challenge_info', token }));

  function back() { return el('a', { class: 'btn', href: '/', text: 'Back to the lobby' }); }
  function show(children) { box.textContent = ''; box.append(...children); }

  function render(info) {
    if (!info.found) {
      show([el('h1', { text: 'Challenge not found' }), el('p', { text: 'This challenge was cancelled or has expired.' }), back()]);
      return;
    }
    if (info.game) { location.href = '/game/' + info.game; return; }
    const color = info.color === 'random' ? '' : (info.mine ? ' You play ' : ' They play ') + info.color + '.';
    if (info.mine) {
      const link = el('input', { class: 'linkbox', type: 'text', readonly: '', value: location.href });
      link.addEventListener('click', () => link.select());
      const copy = el('button', { type: 'button', class: 'btn alt', text: 'Copy link' });
      copy.addEventListener('click', () => {
        navigator.clipboard.writeText(location.href).then(() => { copy.textContent = 'Copied'; }, () => link.select());
      });
      show([
        el('h1', { text: 'Challenge a friend' }),
        el('p', { text: 'Send this link to your friend. The ' + info.tc + ' game starts when they accept.' + color }),
        link, el('p', {}, [copy]),
        el('p', { class: 'muted', text: 'Waiting for your friend…' }),
        el('button', { type: 'button', class: 'btn danger', text: 'Cancel challenge', onclick: () => sock.send({ type: 'challenge_cancel', token }) }),
      ]);
    } else {
      show([
        el('h1', { text: 'Quantum chess challenge' }),
        el('p', {}, [el('b', { text: info.creator }), document.createTextNode(' invites you to a ' + info.tc + ' game.' + color)]),
        el('button', { type: 'button', class: 'btn', text: 'Accept and play', onclick: () => sock.send({ type: 'challenge_accept', token }) }),
      ]);
    }
  }
})();
