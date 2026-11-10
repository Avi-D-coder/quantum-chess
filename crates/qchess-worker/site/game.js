/* A live game (or one that ended within the last hour). */
(function () {
  'use strict';
  const id = decodeURIComponent(location.pathname.split('/')[2] || '');
  const el = document.getElementById('qgame');
  const positions = '/api/game/' + encodeURIComponent(id) + '/positions';
  fetch(positions).then((r) => {
    if (r.status === 404) {
      el.hidden = true;
      const gone = document.getElementById('gone');
      gone.hidden = false;
      gone.textContent = 'This game does not exist. Finished games are kept for an hour.';
      return;
    }
    const transport = new window.QChess.GameClient(window.QC.wsUrl('/ws/game/' + encodeURIComponent(id)));
    window.QChess.mount(el, { transport, historyUrl: positions });
  }).catch(() => { el.textContent = 'Could not reach the server.'; });
})();
