// Builds a synthetic Screen-mode bundle for the autocrop playground: a fake 1512×982 pt desktop
// (menu bar, Dock, Ghostty, Safari, Notes) rendered by Playwright at Scrawl's Screen sampling size
// (1280 px wide), plus a hand-made scrawl.json with window snapshots, ink, clicks, cursor samples
// and narration over two windows.
//
//   bun scripts/autocrop-fixture.mjs [out.scrawl]
//   cargo run -- autocrop out.scrawl --open
//
// Playwright comes from any node_modules that has it (this repo's site/ does).

import { mkdirSync, writeFileSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

const out = resolve(process.argv[2] || join(tmpdir(), 'scrawl-autocrop-fixture.scrawl'));
const { chromium } = await loadPlaywright();

const SW = 1512, SH = 982, MENU = 33;
const FRAME_W = 1280;
const DOCK = { x: 436, y: SH - 74, w: 640, h: 66 };
const WIN = {
  ghostty: { id: 4101, app: 'Ghostty', bundleId: 'com.mitchellh.ghostty', title: 'opencode — ~/code/acme', x: 36, y: 64, w: 660, h: 540 },
  safari: { id: 4102, app: 'Safari', bundleId: 'com.apple.Safari', title: 'Settings — Acme', x: 540, y: 104, w: 920, h: 660 },
  notes: { id: 4103, app: 'Notes', bundleId: 'com.apple.Notes', title: 'Bugs', x: 120, y: 560, w: 470, h: 330 },
};
// Front to back at each moment (also the DOM stacking order).
const ORDER_START = ['ghostty', 'safari', 'notes'];
const ORDER_SAFARI = ['safari', 'ghostty', 'notes'];
const DURATION = 24_000;

// ---- Activity ---------------------------------------------------------------------------------
const n = (x, y) => [Math.round(x / SW * 10_000), Math.round(y / SH * 10_000)];
function strokeFrom(points, t0, msPerPoint = 16) {
  const pts = [];
  points.forEach(([x, y], i) => pts.push(...n(x, y), i * msPerPoint));
  return { t0, t1: t0 + (points.length - 1) * msPerPoint, fw: SW, fh: SH, pts };
}
const circle = [];
for (let i = 0; i <= 40; i++) {
  const a = i / 40 * Math.PI * 2 - 0.4;
  circle.push([980 + 250 * Math.cos(a) + (i > 36 ? 12 : 0), 330 + 62 * Math.sin(a)]);
}
const arrowShaft = [];
for (let i = 0; i <= 18; i++) {
  const t = i / 18;
  arrowShaft.push([760 - 450 * t, 520 - 120 * t - 40 * Math.sin(t * Math.PI)]);
}
const arrowHead = [[340, 382], [310, 400], [345, 420]];
const strokes = [
  strokeFrom(circle, 5_600),
  strokeFrom(arrowShaft, 9_200, 22),
  strokeFrom(arrowHead, 9_700, 30),
];
const clicks = [
  { kind: 'click', t: 3_000, x: 760 / SW, y: 300 / SH, button: 'left' },
  { kind: 'click', t: 13_400, x: 214 / SW, y: 14 / SH, button: 'left' },
];
const events = [
  ...clicks,
  { kind: 'drag', t: 15_200, t1: 15_900, x0: 640 / SW, y0: 600 / SH, x1: 900 / SW, y1: 640 / SH },
  { kind: 'scroll', t: 16_800, x: 1000 / SW, y: 560 / SH, dx: 0, dy: -120 },
];

// Cursor path: wander in Ghostty, move to Safari and click, circle, arrow, menu bar, drag, then
// rest in Safari with one fling to the far corner (an outlier).
const waypoints = [
  [0, 300, 300], [1_500, 380, 340], [2_400, 600, 320], [3_000, 760, 300], [5_400, 760, 300],
  ...circle.map(([x, y], i) => [5_600 + i * 16, x, y]),
  [8_500, 900, 420], [9_200, 760, 520], ...arrowShaft.map(([x, y], i) => [9_200 + i * 22, x, y]),
  [10_100, 345, 420], [12_600, 300, 200], [13_400, 214, 14], [14_200, 640, 600], [15_200, 640, 600],
  [15_900, 900, 640], [16_800, 1000, 560], [18_000, 1050, 540], [18_400, 1490, 960], [18_700, 1100, 520],
  [21_000, 1080, 500], [24_000, 1080, 500],
];
const cursor = [];
for (let t = 0; t <= DURATION; t += 66) {
  let i = waypoints.findIndex(w => w[0] > t);
  if (i < 0) i = waypoints.length - 1;
  const a = waypoints[Math.max(0, i - 1)], b = waypoints[i];
  const f = b[0] === a[0] ? 1 : Math.min(1, (t - a[0]) / (b[0] - a[0]));
  const x = (a[1] + (b[1] - a[1]) * f) / SW, y = (a[2] + (b[2] - a[2]) * f) / SH;
  const last = cursor[cursor.length - 1];
  if (!last || Math.hypot((x - last[1]) * SW, (y - last[2]) * SH) >= 2) cursor.push([t, +x.toFixed(4), +y.toFixed(4)]);
}

function snapshot(t, order, front) {
  const windows = [
    { id: 6, app: 'Window Server', title: 'Menubar', bounds: [0, 0, SW, MENU], layer: 24, regular: false },
    ...order.map(k => ({ id: WIN[k].id, app: WIN[k].app, bundleId: WIN[k].bundleId, title: WIN[k].title,
      bounds: [WIN[k].x, WIN[k].y, WIN[k].w, WIN[k].h], layer: 0, ...(k === front ? { frontmost: true } : {}) })),
    { id: 38, app: 'Dock', bundleId: 'com.apple.dock', bounds: [0, DOCK.y - 4, SW, SH - DOCK.y + 4], layer: 20, regular: false },
  ];
  return { t, windows };
}
const windows = [snapshot(0, ORDER_START, 'ghostty'), snapshot(3_020, ORDER_SAFARI, 'safari')];

const words = [];
function say(t, text, perWord = 260) {
  for (const w of text.split(' ')) { words.push({ t0: t, t1: t + perWord - 30, text: w }); t += perWord; }
}
say(400, 'So this is the settings page in the dev build,');
say(3_100, 'and when I open the account card');
say(5_500, 'this header is clipped, look at the top edge.');
say(9_100, 'It should match what the terminal prints here.');
say(13_300, 'Even the history menu');
say(15_100, 'and dragging the slider does nothing.');
say(18_900, 'Look, the save button is still disabled.');

const keyframes = [
  { index: 1, t: 400, reason: 'initial', event: 'start', said: 'So this is the settings page in the dev build,' },
  { index: 2, t: 3_000, reason: 'click', event: 'clicked Account', said: 'and when I open the account card', cursor: [760 / SW, 300 / SH] },
  { index: 3, t: 6_300, reason: 'mark', event: 'circled the card header', said: 'this header is clipped, look at the top edge.', markId: 'm1' },
  { index: 4, t: 9_900, reason: 'mark', event: 'arrow to the terminal', said: 'It should match what the terminal prints here.', markId: 'm2' },
  { index: 5, t: 13_400, reason: 'click', event: 'clicked History', said: 'Even the history menu and dragging the slider does nothing.', cursor: [214 / SW, 14 / SH] },
  { index: 6, t: 19_000, reason: 'speech', event: 'said “look”', said: 'Look, the save button is still disabled.' },
].map(k => ({ ...k, file: `frames/frame-${String(k.index).padStart(2, '0')}.png` }));

const box = s => {
  const xs = [], ys = [];
  for (let i = 0; i < s.pts.length; i += 3) { xs.push(s.pts[i] / 10_000); ys.push(s.pts[i + 1] / 10_000); }
  return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
};
const bundle = {
  version: 1, id: 'autocrop-fixture', createdAtMs: Date.now(), durationMs: DURATION,
  target: { mode: 'display', app: 'Ghostty', bundleId: 'com.mitchellh.ghostty', displayId: 1,
    frame: { x: 0, y: 0, width: SW, height: SH, scale: 2 } },
  transcript: { source: 'fixture', text: words.map(w => w.text).join(' '), segments: [], words },
  note: 'Synthetic Screen-mode capture for the autocrop playground',
  strokes,
  marks: [
    { id: 'm1', strokes: [0], t0: strokes[0].t0, t1: strokes[0].t1, boxNorm: box(strokes[0]), shape: 'circle', description: 'circled the card header', frame: keyframes[2].file },
    { id: 'm2', strokes: [1, 2], t0: strokes[1].t0, t1: strokes[2].t1, boxNorm: box(strokes[1]), shape: 'arrow', description: 'arrow to the terminal', frame: keyframes[3].file },
  ],
  events, keyframes, contactSheet: 'contact-sheet.png', windows, cursor,
};

// ---- Frames -----------------------------------------------------------------------------------
const pageHtml = `<!doctype html><html><head><style>
* { box-sizing: border-box; margin: 0; }
body { width: ${SW}px; height: ${SH}px; overflow: hidden; font: 13px -apple-system, "Helvetica Neue", sans-serif;
  background: radial-gradient(circle at 20% 10%, #3b5b8f, transparent 55%), radial-gradient(circle at 85% 80%, #8a3b6e, transparent 55%), linear-gradient(160deg, #1d2b4a, #2a1838); }
.menubar { position: absolute; left: 0; top: 0; width: 100%; height: ${MENU}px; background: rgba(30,30,36,.55); backdrop-filter: blur(20px);
  color: #fff; display: flex; align-items: center; gap: 20px; padding: 0 16px; font-size: 13.5px; }
.menubar b { font-weight: 700; } .menubar .r { margin-left: auto; opacity: .9; }
.menubar .hl { background: rgba(255,255,255,.22); border-radius: 5px; padding: 2px 7px; margin: 0 -7px; }
.dock { position: absolute; left: ${DOCK.x}px; top: ${DOCK.y}px; width: ${DOCK.w}px; height: ${DOCK.h}px; border-radius: 20px;
  background: rgba(255,255,255,.18); border: 1px solid rgba(255,255,255,.25); display: flex; gap: 10px; align-items: center; padding: 0 12px; }
.dock i { width: 48px; height: 48px; border-radius: 12px; display: block; }
.win { position: absolute; border-radius: 11px; overflow: hidden; box-shadow: 0 22px 60px rgba(0,0,0,.55), 0 0 0 1px rgba(255,255,255,.12); }
.bar { height: 30px; display: flex; align-items: center; gap: 8px; padding: 0 12px; font-size: 12px; color: #bbb; }
.dots { display: flex; gap: 7px; } .dots i { width: 12px; height: 12px; border-radius: 50%; display: block; }
.ghostty { background: #16161b; color: #c9d1d9; font: 13px/1.55 ui-monospace, Menlo, monospace; }
.ghostty .bar { background: #1f1f26; }
.ghostty pre { padding: 12px 16px; white-space: pre-wrap; }
.g { color: #3fb950; } .y { color: #d29922; } .m { color: #bc8cff; } .c { color: #39c5cf; } .d { color: #6e7681; }
.safari { background: #f5f5f7; color: #1d1d1f; }
.safari .bar { background: #e8e8ed; height: 52px; color: #333; }
.url { margin: 0 auto; background: #fff; border-radius: 8px; padding: 6px 80px; font-size: 13px; color: #555; }
.page { display: grid; grid-template-columns: 190px 1fr; height: calc(100% - 52px); }
.side { background: #ececf0; padding: 18px 14px; display: grid; gap: 6px; align-content: start; }
.side div { padding: 7px 10px; border-radius: 7px; } .side .on { background: #fff; font-weight: 600; }
.main { padding: 26px 34px; display: grid; gap: 18px; align-content: start; }
.main h1 { font-size: 24px; }
.card { background: #fff; border-radius: 12px; box-shadow: 0 1px 3px rgba(0,0,0,.12); padding: 16px 20px; }
.card h2 { font-size: 16px; margin: -26px 0 10px; background: #fff; display: inline-block; padding: 4px 8px; border-radius: 6px; }
.row { display: flex; justify-content: space-between; padding: 9px 0; border-top: 1px solid #eee; color: #444; }
.slider { height: 6px; background: #ddd; border-radius: 3px; position: relative; margin: 12px 0 4px; }
.slider i { position: absolute; left: 30%; top: -7px; width: 20px; height: 20px; border-radius: 50%; background: #fff; box-shadow: 0 1px 4px rgba(0,0,0,.3); }
.btn { justify-self: end; background: #b8c7e8; color: #fff; padding: 9px 20px; border-radius: 8px; font-weight: 600; }
.notes { background: #fffdf3; color: #333; }
.notes .bar { background: #f3eedb; }
.notes ul { padding: 12px 28px; line-height: 1.9; }
svg { position: absolute; left: 0; top: 0; width: ${SW}px; height: ${SH}px; pointer-events: none; }
.ripple { position: absolute; width: 44px; height: 44px; margin: -22px; border-radius: 50%; border: 3px solid rgba(255,59,48,.9); background: rgba(255,59,48,.18); }
.badge { position: absolute; top: 7px; right: 7px; min-width: 26px; height: 26px; border-radius: 13px; background: rgba(18,18,20,.86);
  border: 1px solid rgba(255,255,255,.17); color: #fff; font-weight: 700; font-size: 14px; display: grid; place-items: center; padding: 0 8px; }
.cursor { position: absolute; width: 0; height: 0; border-left: 9px solid #fff; border-top: 0 solid transparent; }
.cursor::before { content: ''; position: absolute; left: -10px; top: -1px; border-left: 11px solid #000; border-top: 7px solid transparent; border-bottom: 9px solid transparent; transform: rotate(-20deg); transform-origin: 0 0; }
.dropdown { position: absolute; left: 196px; top: ${MENU}px; width: 230px; background: rgba(240,240,244,.96); border-radius: 0 0 8px 8px; box-shadow: 0 10px 30px rgba(0,0,0,.4); padding: 6px; color: #222; }
.dropdown div { padding: 5px 10px; border-radius: 5px; } .dropdown .on { background: #2f6fec; color: #fff; }
</style></head><body>
<div class="menubar"><b id="appname">Ghostty</b><span>File</span><span>Edit</span><span>View</span><span id="history">History</span><span>Window</span><span>Help</span><span class="r">Thu 15 Oct  10:24</span></div>
<div class="win ghostty" id="ghostty"><div class="bar"><span class="dots"><i style="background:#ff5f57"></i><i style="background:#febc2e"></i><i style="background:#28c840"></i></span><span style="margin:auto">${WIN.ghostty.title}</span></div><pre><span class="d">~/code/acme</span> <span class="g">❯</span> bun dev
<span class="c">  VITE v6.1.0</span>  ready in 412 ms

  <span class="g">➜</span>  Local:   http://localhost:5173/
  <span class="g">➜</span>  Network: use --host to expose

<span class="d">10:22:41</span> <span class="y">[hmr]</span> update /src/settings/AccountCard.tsx
<span class="d">10:22:44</span> <span class="m">[layout]</span> AccountCard header offset: -26px
<span class="d">10:22:44</span> <span class="m">[layout]</span> expected header offset: 0px
<span class="d">10:23:02</span> <span class="y">[hmr]</span> update /src/settings/Slider.tsx
<span class="d">10:23:10</span> <span class="y">warn</span> Slider: onChange is not a function

<span class="d">~/code/acme</span> <span class="g">❯</span> <span style="background:#c9d1d9">&nbsp;</span></pre></div>
<div class="win safari" id="safari"><div class="bar"><span class="dots"><i style="background:#ff5f57"></i><i style="background:#febc2e"></i><i style="background:#28c840"></i></span><span class="url">localhost:5173/settings</span></div>
<div class="page"><div class="side"><div>General</div><div class="on">Account</div><div>Billing</div><div>Notifications</div><div>Privacy</div></div>
<div class="main"><h1>Settings</h1>
<div class="card" style="margin-top:10px"><h2>Account</h2><div class="row"><span>Name</span><span>Kit Langton</span></div><div class="row"><span>Email</span><span>kit@example.com</span></div><div class="row"><span>Plan</span><span>Pro</span></div></div>
<div class="card"><div style="font-weight:600">Notification volume</div><div class="slider"><i></i></div><div class="d" style="font-size:12px;color:#888">Drag to adjust</div></div>
<div class="btn">Save changes</div></div></div></div>
<div class="win notes" id="notes"><div class="bar"><span class="dots"><i style="background:#ff5f57"></i><i style="background:#febc2e"></i><i style="background:#28c840"></i></span><span style="margin:auto">Notes — Bugs</span></div><ul><li>Account header clipped</li><li>Slider ignores drags</li><li>Save stays disabled</li><li>History menu empty?</li></ul></div>
<div class="dock"><i style="background:#2d6cdf"></i><i style="background:#1f1f24;border:1px solid #444"></i><i style="background:linear-gradient(#5ac8fa,#007aff)"></i><i style="background:#ffd60a"></i><i style="background:#34c759"></i><i style="background:#ff9f0a"></i><i style="background:#af52de"></i><i style="background:#ff375f"></i><i style="background:#8e8e93"></i><i style="background:#64d2ff"></i></div>
<div id="dropdown" class="dropdown" style="display:none"><div>Show All History</div><div class="on">Recently Closed</div><div>Clear History…</div></div>
<svg id="ink"></svg><div id="fx"></div>
</body></html>`;

function stateFor(k) {
  const t = keyframes[k].t;
  const order = t >= 3_000 ? ORDER_SAFARI : ORDER_START;
  const shown = strokes.map((s, i) => ({ i, s })).filter(({ s }) => s.t0 <= t && t - s.t1 < 1_800);
  const near = (a, b) => Math.abs(a - b) < 600;
  const click = clicks.find(c => near(c.t, t));
  const pos = cursor.reduce((p, c) => (c[0] <= t ? c : p), cursor[0]);
  return { order, front: order[0], ink: shown.map(({ s }) => s.pts), click: click && [click.x * SW, click.y * SH],
    menu: t >= 13_400 && t < 15_000, index: keyframes[k].index, cursor: [pos[1] * SW, pos[2] * SH], win: WIN };
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: SW, height: SH }, deviceScaleFactor: FRAME_W / SW });
await page.setContent(pageHtml);
mkdirSync(join(out, 'frames'), { recursive: true });
for (let k = 0; k < keyframes.length; k++) {
  await page.evaluate(st => {
    const { SW, SH } = { SW: innerWidth, SH: innerHeight };
    st.order.slice().reverse().forEach((key, z) => {
      const el = document.getElementById(key), w = st.win[key];
      Object.assign(el.style, { left: w.x + 'px', top: w.y + 'px', width: w.w + 'px', height: w.h + 'px', zIndex: 10 + z });
    });
    document.getElementById('appname').textContent = st.win[st.front].app;
    document.getElementById('dropdown').style.display = st.menu ? 'block' : 'none';
    document.getElementById('dropdown').style.zIndex = 50;
    document.getElementById('history').className = st.menu ? 'hl' : '';
    const svg = document.getElementById('ink'); svg.style.zIndex = 60;
    svg.innerHTML = st.ink.map(pts => {
      const d = [];
      for (let i = 0; i < pts.length; i += 3) d.push(`${i ? 'L' : 'M'}${pts[i] / 10000 * SW} ${pts[i + 1] / 10000 * SH}`);
      return `<path d="${d.join(' ')}" fill="none" stroke="#ff3b30" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>`;
    }).join('');
    const fx = document.getElementById('fx'); fx.style.cssText = 'position:absolute;inset:0;z-index:70';
    fx.innerHTML = (st.click ? `<div class="ripple" style="left:${st.click[0]}px;top:${st.click[1]}px"></div>` : '')
      + `<div class="cursor" style="left:${st.cursor[0]}px;top:${st.cursor[1]}px"></div>`
      + `<div class="badge">${st.index}</div>`;
  }, stateFor(k));
  await page.screenshot({ path: join(out, keyframes[k].file) });
}
await browser.close();

writeFileSync(join(out, 'scrawl.json'), JSON.stringify(bundle, null, 2));
writeFileSync(join(out, 'summary.md'), '[synthetic autocrop fixture]\n');
console.log(out);

async function loadPlaywright() {
  try { return await import('playwright'); } catch {}
  const here = new URL('..', import.meta.url).pathname;
  for (const dir of [join(here, 'site/node_modules/playwright')]) {
    if (existsSync(dir)) return await import(join(dir, 'index.mjs'));
  }
  throw new Error('Playwright not found: install it (bun add -d playwright) or run from a checkout with site/node_modules');
}
