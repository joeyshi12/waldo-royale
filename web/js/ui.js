
export const $ = (sel) => document.querySelector(sel);

export function showScreen(id) {
  for (const s of document.querySelectorAll('.screen')) s.classList.remove('show');
  if (id) $(id).classList.add('show');
}

export function toast(msg, ms = 1800, kind = 'error') {
  const t = $('#toast');
  t.textContent = msg;
  t.classList.toggle('info', kind === 'info');
  t.classList.add('show');
  clearTimeout(t._t);
  t._t = setTimeout(() => t.classList.remove('show'), ms);
}

export function fmtTime(totalSecs) {
  const s = Math.max(0, Math.ceil(totalSecs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

// player colors, index-stable within a game
export const PLAYER_COLORS = [
  '#ff5a5a', '#4fc3f7', '#ffe14a', '#81c784',
  '#ba68c8', '#ff9e40', '#f06292', '#4db6ac',
  '#a1887f', '#90a4ae', '#dce775', '#7986cb',
];
