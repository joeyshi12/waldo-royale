// Client orchestration: menu, lobby, rounds, results. World generation
// runs in Rust (WASM); a 32-bit seed is all that crosses the wire per round.

import init, { generate_world } from '../pkg/waldo_wasm.js';
import { GameRenderer } from './render.js?v=0.3.1';
import { Net } from './net.js?v=0.3.1';
import { $, showScreen, toast, fmtTime, PLAYER_COLORS } from './ui.js?v=0.3.1';

await init();

const canvas = $('#scene');
const R = new GameRenderer(canvas);
const net = new Net();

// ---------- state ----------
let myId = null;
let isHost = false;
let players = [];
let playing = false;
let found = false;
let lastPick = null;
let currentMeta = null;
let roundEndsAt = 0;
let roundMs = 1;
let nextRoundAt = 0;
let isLastRound = false;
let world = null;

const colorForId = (id) => {
  const idx = players.findIndex((p) => p.id === id);
  return PLAYER_COLORS[(idx >= 0 ? idx : 0) % PLAYER_COLORS.length];
};

// ---------- menu ----------
$('#createBtn').onclick = () =>
  connectThen(() => net.send({ type: 'create', name: $('#nameInput').value || 'Player' }));
$('#joinBtn').onclick = () => {
  const code = $('#codeInput').value.trim().toUpperCase();
  if (code.length !== 4) return toast('lobby codes are 4 letters');
  connectThen(() => net.send({ type: 'join', code, name: $('#nameInput').value || 'Player' }));
};

let connected = false;
function connectThen(fn) {
  if (connected) return fn();
  net.connect().then(() => { connected = true; fn(); })
    .catch((e) => toast(e.message));
}

// ---------- lobby ----------
function renderLobby(msg) {
  players = msg.players;
  myId = msg.you;
  isHost = players.find((p) => p.id === myId)?.is_host ?? false;
  $('#lobbyCode').textContent = msg.code;
  $('#playerList').innerHTML = players.map((p, i) => `
    <li><span><span class="swatch" style="background:${PLAYER_COLORS[i % PLAYER_COLORS.length]}"></span>
    ${escapeHtml(p.name)}${p.id === myId ? ' (you)' : ''}</span>
    ${p.is_host ? '<span class="host-tag">Host</span>' : ''}</li>`).join('');
  $('#hostControls').style.display = isHost ? 'block' : 'none';
  $('#waitingMsg').style.display = isHost ? 'none' : 'block';
  $('#roundsSel').value = String(msg.config.rounds);
  $('#secsSel').value = String(msg.config.round_secs);
}

const sendConfig = () => net.send({
  type: 'configure',
  rounds: parseInt($('#roundsSel').value, 10),
  round_secs: parseInt($('#secsSel').value, 10),
});
$('#roundsSel').onchange = sendConfig;
$('#secsSel').onchange = sendConfig;
$('#startBtn').onclick = () => net.send({ type: 'start' });
$('#backBtn').onclick = () => showScreen('#lobby');

// ---------- protocol ----------
net
  .on('error', (m) => toast(m.message))
  .on('lobby', (m) => {
    renderLobby(m);
    // The server re-broadcasts lobby state right after game_over;
    // don't let that replace the win screen or round results.
    const inFlow = playing
      || $('#final').classList.contains('show')
      || $('#results').classList.contains('show');
    if (!inFlow) showScreen('#lobby');
  })
  .on('round_start', async (m) => {
    playing = true;
    found = false;
    lastPick = null;
    roundMs = m.round_secs * 1000;
    roundEndsAt = Math.min(m.ends_at_ms, Date.now() + roundMs);

    await R.assetsReady; // models load once, at startup; no-op afterwards
    world = generate_world(m.seed, m.mutator);
    currentMeta = JSON.parse(world.metaJson());
    R.buildWorld(world, currentMeta);

    $('#roundNum').textContent = m.round;
    $('#roundTotal').textContent = m.total_rounds;
    $('#shapeName').textContent = `the ${currentMeta.shape_name} Planet`;
    $('#foundBanner').style.display = 'none';
    $('#foundCount').style.display = 'none';
    const labels = {
      night: 'Night round · your cursor is a searchlight',
      lightning: 'Lightning round · a third of the time',
      crowded: 'Crowded round · extra everything',
      tiny: 'Tiny planet · everyone is very close together',
    };
    if (labels[m.mutator]) toast(labels[m.mutator], 4000, 'info');
    R.setMutator(m.mutator);
    showScreen(null);
    $('#gameHud').classList.add('show');
  })
  .on('click_result', (m) => {
    const castPos = (role) => currentMeta.cast.find((c) => c.role === role)?.pos;
    switch (m.target) {
      case 'waldo':
        found = true;
        $('#foundBanner').style.display = 'block';
        $('#foundScore').textContent =
          `+${m.points} · bonus hunt: Wenda, Woof's tail, the Wizard`;
        R.markFound(currentMeta.waldo.pos);
        break;
      case 'wenda':
      case 'woof':
      case 'wizard': {
        const names = { wenda: 'Wenda', woof: "Woof's tail", wizard: 'Wizard Whitebeard' };
        toast(`${names[m.target]}! +${m.points}`, 1800, 'info');
        R.showPing(castPos(m.target), 0xffe14a);
        break;
      }
      case 'odlaw':
        toast(`That's Odlaw! ${m.points} points`, 2600);
        R.showPing(castPos('odlaw'), 0xe8a020);
        break;
      default:
        toast(`Not him! ${m.points} points (${m.misses} ${m.misses === 1 ? 'miss' : 'misses'})`);
        if (lastPick) R.showMiss(lastPick);
    }
  })
  .on('player_found', (m) => {
    const el = $('#foundCount');
    el.style.display = 'block';
    el.textContent = `${m.found}/${m.total} found him`;
  })
  .on('round_result', (m) => {
    playing = false;
    $('#gameHud').classList.remove('show');
    R.revealWaldo(m.waldo);

    $('#resRound').textContent = m.round;
    $('#resRows').innerHTML = m.results.map((r) => `
      <tr>
        <td><span class="swatch" style="background:${colorForId(r.id)}"></span>${escapeHtml(r.name)}</td>
        <td>${r.found ? `#${r.rank} · ${(r.time_ms / 1000).toFixed(1)} s` : 'not found'}${
          r.misses ? ` <span style="opacity:.6">· ${r.misses}✗</span>` : ''}</td>
        <td style="text-align:right">${r.bonus > 0 ? '+' : ''}${r.bonus || ''}</td>
        <td class="score">${r.score}</td>
        <td>${r.total}</td>
      </tr>`).join('');
    nextRoundAt = Date.now() + m.next_in_ms;
    isLastRound = m.round >= parseInt($('#roundTotal').textContent, 10);
    showScreen('#results');
  })
  .on('game_over', (m) => {
    const lb = m.leaderboard;
    const myRank = lb.findIndex((s) => s.id === myId);
    const iWon = myRank === 0;
    $('#trophy').textContent = iWon ? '🏆' : ['', '🥈', '🥉'][myRank] ?? '😔';
    $('#winnerName').textContent = iWon ? 'You win!' : (lb[0]?.name ?? 'Nobody') + ' wins';
    $('#winnerName').style.color = iWon ? '#ffe14a' : '#b9c0d8';
    $('#finalSub').textContent = iWon
      ? 'champion of the galaxy'
      : `you finished ${['1st', '2nd', '3rd'][myRank] ?? `${myRank + 1}th`} of ${lb.length}`;
    $('#podium').innerHTML = lb.map((s, i) => `
      <li><span>${['🥇', '🥈', '🥉'][i] ?? `${i + 1}.`} ${escapeHtml(s.name)}${
        s.id === myId ? ' (you)' : ''}</span>
      <b>${s.total}</b></li>`).join('');
    showScreen('#final');
  })
  .on('close', () => {
    toast('disconnected from server', 4000);
    playing = false;
    $('#gameHud').classList.remove('show');
    showScreen('#menu');
    connected = false;
  });

// ---------- game input: drag spins, click guesses, wheel zooms to cursor ----------
let dragging = false, dragDist = 0, px = 0, py = 0;
const pointers = new Map();
let pinchDist = 0, lastPinch = -1000;
const pinchGap = () => {
  const [a, b] = [...pointers.values()];
  return Math.hypot(a.x - b.x, a.y - b.y);
};

canvas.addEventListener('pointerdown', (e) => {
  pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
  canvas.setPointerCapture(e.pointerId);
  if (pointers.size === 2) {
    dragging = false;
    canvas.classList.remove('dragging');
    pinchDist = pinchGap();
  } else if (playing) {
    dragging = true;
    dragDist = 0;
    px = e.clientX; py = e.clientY;
  }
});

canvas.addEventListener('pointermove', (e) => {
  const p = pointers.get(e.pointerId);
  if (p) { p.x = e.clientX; p.y = e.clientY; }
  R.updateSearchlight(e.clientX, e.clientY);
  if (pointers.size === 2) {
    const d = pinchGap();
    if (pinchDist > 0) R.setZoom(R.zoom * d / pinchDist);
    pinchDist = d;
    lastPinch = performance.now();
    return;
  }
  if (!dragging) return;
  const dx = e.clientX - px, dy = e.clientY - py;
  dragDist += Math.abs(dx) + Math.abs(dy);
  if (dragDist > 4) {
    canvas.classList.add('dragging');
    R.rotatePlanet(dx, dy);
  }
  px = e.clientX; py = e.clientY;
});

const endDrag = (e) => {
  pointers.delete(e.pointerId);
  if (pointers.size < 2) pinchDist = 0;
  if (pointers.size === 0) { dragging = false; canvas.classList.remove('dragging'); }
};
addEventListener('pointerup', endDrag);
addEventListener('pointercancel', endDrag);

canvas.addEventListener('wheel', (e) => {
  e.preventDefault();
  R.zoomAt(e.clientX, e.clientY, Math.exp(-e.deltaY * 0.0012));
}, { passive: false });
$('#zoomIn').onclick = () => R.setZoom(R.zoom * 1.3);
$('#zoomOut').onclick = () => R.setZoom(R.zoom / 1.3);

canvas.addEventListener('click', (e) => {
  if (!playing || found) return;
  if (dragDist > 4) return; // that was a spin, not a guess
  if (performance.now() - lastPinch < 400) return;
  const pick = R.pickSurface(e.clientX, e.clientY);
  if (!pick) return;
  lastPick = pick;
  net.send({ type: 'click', pos: [pick.pos.x, pick.pos.y, pick.pos.z] });
});

// ---------- per-frame HUD updates ----------
R.onTick = () => {
  if (playing) {
    const left = Math.max(0, roundEndsAt - Date.now());
    $('#timerText').textContent = fmtTime(left / 1000);
    const frac = left / roundMs;
    const fill = $('#timerFill');
    fill.style.width = `${frac * 100}%`;
    fill.style.background = frac < 0.15 ? '#ff5a5a' : frac < 0.4 ? '#ffe14a' : '#4caf50';
  }
  if ($('#results').classList.contains('show') && nextRoundAt) {
    const s = Math.max(0, nextRoundAt - Date.now()) / 1000;
    const label = isLastRound ? 'final standings' : 'next round';
    $('#nextIn').textContent = s > 0.2 ? `${label} in ${Math.ceil(s)}…` : 'get ready…';
  }
};

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
