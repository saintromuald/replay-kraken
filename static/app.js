// Replay Kraken browser — plain fetch, no build step.

const tokenInput = document.getElementById('token');
const followed = document.getElementById('followed');

function authHeaders() {
  const t = tokenInput.value.trim();
  return t ? { Authorization: 'Bearer ' + t } : {};
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function playerLink(name) {
  return `<a href="#" data-player="${esc(name)}">${esc(name)}</a>`;
}

function raceRow(label, stats) {
  if (!stats || !stats.length) return '';
  const rows = stats
    .map((r) => {
      const total = r.games || 1;
      const wr = Math.round((r.wins / total) * 100);
      return `<tr><td>${esc(label)} ${esc(r.race)}</td><td>${r.wins}–${r.losses}</td><td>${wr}%</td><td>${r.games}</td></tr>`;
    })
    .join('');
  return rows;
}

async function showStats(name) {
  const res = await fetch('/api/stats/' + encodeURIComponent(name));
  if (!res.ok) return;
  const s = await res.json();
  const panel = document.getElementById('stats');
  const total = s.games || 1;
  const winrate = Math.round((s.wins / total) * 100);
  const weeks = s.weekly
    .map((w) => `<span title="${esc(w.week)}">${w.wins}W/${w.games - w.wins}L</span>`)
    .join(' → ');
  panel.innerHTML = `
    <h2>${playerLink(s.id)} ${s.discord_id ? `<span class="dim">(discord ${esc(s.discord_id)})</span>` : ''}</h2>
    <p><b>${s.wins}</b> wins · <b>${s.losses}</b> losses · <b>${winrate}%</b> over ${s.games} games</p>
    <h3>vs opponent race</h3>
    <table>${raceRow('vs', s.vs_race) || '<tr><td colspan="4">no data</td></tr>'}</table>
    <h3>while playing</h3>
    <table>${raceRow('as', s.own_race) || '<tr><td colspan="4">no data</td></tr>'}</table>
    <h3>progress (wins/games per week)</h3>
    <p class="small">${weeks || 'no data'}</p>`;
  panel.style.display = 'block';
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
        return `<span class="${cls}">${playerLink(p.name)} (${esc(p.race)})${p.result === 'won' ? ' ✓' : p.result === 'lost' ? ' ✗' : ''}</span>`;
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
  document.querySelectorAll('a[data-player]').forEach((a) => {
    a.addEventListener('click', (ev) => {
      ev.preventDefault();
      showStats(a.dataset.player);
    });
  });
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

