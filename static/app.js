// Replay Kraken browser — plain fetch, no build step.

const tokenInput = document.getElementById('token');
const followed = document.getElementById('followed');

function authHeaders() {
  const t = tokenInput.value.trim();
  return t ? { Authorization: 'Bearer ' + t } : {};
}

async function load() {
  const q = new URLSearchParams();
  const take = (id) => {
    const el = document.getElementById(id);
    if (el && el.value) q.set(id, el.value);
  };
  take('player');
  take('race');
  take('result');
  take('matchup');
  take('from');
  take('to');
  if (followed.value === '1') {
    // Following is per-token: the server resolves the account from the token.
    q.set('followed_by', '@me');
  }
  q.set('limit', '100');

  const res = await fetch('/api/matches?' + q.toString());
  const data = await res.json();
  const rows = document.getElementById('rows');
  document.getElementById('total').textContent = data.total + ' matches';
  rows.innerHTML = '';
  for (const m of data.matches) {
    const tr = document.createElement('tr');
    const players = m.players
      .map((p) => {
        const cls = p.result === 'won' ? 'won' : p.result === 'lost' ? 'lost' : '';
        return `<span class="${cls}">${esc(p.name)} (${esc(p.race)})${p.result === 'won' ? ' ✓' : p.result === 'lost' ? ' ✗' : ''}</span>`;
      })
      .join(' &nbsp; ');
    tr.innerHTML = `
      <td>${esc(m.uploaded_at.replace('T', ' ').slice(0, 16))}</td>
      <td>${esc(m.map)}</td>
      <td>${esc(m.matchup)}</td>
      <td>${players}</td>
      <td><button data-follow="${esc(m.players[0]?.name || '')}">follow first player</button></td>
      <td><a href="/api/matches/${esc(m.uuid)}/replay">rep</a></td>`;
    rows.appendChild(tr);
  }
  document.querySelectorAll('button[data-follow]').forEach((b) => {
    b.addEventListener('click', async () => {
      const name = b.dataset.follow;
      if (!name) return;
      const res = await fetch('/api/players/' + encodeURIComponent(name) + '/follow', {
        method: 'PUT',
        headers: authHeaders(),
      });
      b.textContent = res.ok ? 'following ✓' : 'need token';
    });
  });
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

document.getElementById('go').addEventListener('click', load);
document.getElementById('reset').addEventListener('click', () => {
  for (const id of ['player', 'race', 'result', 'matchup', 'from', 'to']) {
    document.getElementById(id).value = '';
  }
  followed.value = '';
  load();
});
load();
