'use strict';

(async function main() {
  if (!window.__TAURI__) {
    // Opened in a plain browser (design preview): load the fake backend.
    await new Promise((ok) => {
      const s = document.createElement('script');
      s.src = 'dev/preview.js';
      s.onload = ok;
      document.head.appendChild(s);
    });
  }
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const dialog = window.__TAURI__.dialog;

  // ------------------------------------------------------------ helpers
  const $ = (id) => document.getElementById(id);
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  };
  const NS = 'http://www.w3.org/2000/svg';
  const icon = (...paths) => {
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    for (const d of paths) {
      const p = document.createElementNS(NS, 'path');
      p.setAttribute('d', d);
      svg.appendChild(p);
    }
    return svg;
  };
  const I = {
    check: ['M5 12l5 5 9-10'],
    x: ['M6 6l12 12', 'M18 6L6 18'],
    chev: ['M9 6l6 6-6 6'],
    up: ['M12 19V5', 'M6 11l6-6 6 6'],
    down: ['M12 5v14', 'M6 13l6 6 6-6'],
    laptop: ['M4 6a1 1 0 0 1 1-1h14a1 1 0 0 1 1 1v9H4z', 'M2 19h20'],
    video: ['M3 6h12v12H3z', 'M15 10l6-3v10l-6-3'],
    image: ['M3 5h18v14H3z', 'M3 16l5-5 4 4 3-3 6 6', 'M15 9h.01'],
    doc: ['M6 3h8l5 5v13H6z', 'M14 3v5h5', 'M9 13h6M9 17h6'],
    zip: ['M6 3h12v18H6z', 'M12 3v2M12 7v2M12 11v2', 'M10 15h4v3h-4z'],
    audio: ['M9 18V6l10-2v12', 'M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0z', 'M19 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z'],
    folder: ['M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z'],
    file: ['M6 3h8l5 5v13H6z', 'M14 3v5h5'],
  };
  const KINDS = [
    [/\.(mp4|mov|mkv|avi|webm|m4v)$/i, 'video', '#7c3aed'],
    [/\.(jpe?g|png|gif|heic|webp|tiff?|raw|dng|svg)$/i, 'image', '#059669'],
    [/\.pdf$/i, 'doc', '#dc2626'],
    [/\.(zip|rar|7z|tar|gz|dmg|iso)$/i, 'zip', '#d97706'],
    [/\.(mp3|wav|flac|m4a|aac|ogg)$/i, 'audio', '#db2777'],
    [/\.(docx?|pages|txt|rtf|md|xlsx?|numbers|pptx?|key|csv)$/i, 'doc', '#2563eb'],
  ];
  function tile(name, big, multiple) {
    let kind = 'file';
    let color = '#64748b';
    if (multiple) { kind = 'folder'; color = '#f59e0b'; } else {
      for (const [re, k, c] of KINDS) if (re.test(name)) { kind = k; color = c; break; }
    }
    const t = el('div', 'file-tile' + (big ? ' big' : ''));
    t.style.background = color;
    t.append(icon(...I[kind]));
    return t;
  }
  function human(n) {
    const u = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    while (n >= 1000 && i < u.length - 1) { n /= 1000; i++; }
    return `${n >= 100 || i === 0 ? Math.round(n) : n.toFixed(1)} ${u[i]}`;
  }
  function duration(s) {
    if (!isFinite(s) || s < 0) return '—';
    if (s < 60) { const n = Math.max(1, Math.round(s)); return n === 1 ? '1 secondo' : `${n} secondi`; }
    const m = Math.floor(s / 60);
    if (m < 60) return `${m} min ${Math.round(s % 60)} s`;
    return `${Math.floor(m / 60)} h ${m % 60} min`;
  }
  function when(ts) {
    const d = new Date(ts * 1000);
    const today = new Date();
    const y = new Date(); y.setDate(today.getDate() - 1);
    const hm = d.toLocaleTimeString('it-IT', { hour: '2-digit', minute: '2-digit' });
    if (d.toDateString() === today.toDateString()) return `oggi, ${hm}`;
    if (d.toDateString() === y.toDateString()) return `ieri, ${hm}`;
    return d.toLocaleDateString('it-IT');
  }
  const files_label = (n) => (n === 1 ? '1 file' : `${n} file`);
  function hue(s) { let h = 0; for (const c of s || '') h = (h * 31 + c.charCodeAt(0)) % 360; return h; }
  function initials(name) {
    const w = (name || '?').replace(/[^\p{L}\p{N} ]/gu, ' ').trim().split(/\s+/);
    return ((w[0] || '?')[0] + (w.length > 1 ? w[w.length - 1][0] : '')).toUpperCase();
  }
  function avatar(node, name, seed) {
    node.textContent = initials(name);
    const h = hue(seed || name);
    node.style.background = `hsl(${h} 65% 52%)`;
    return node;
  }
  function deviceIcon(name, seed) {
    const h = hue(seed || name);
    const d = el('div', 'dev-icon');
    d.style.background = `hsl(${h} 80% 94%)`;
    d.style.color = `hsl(${h} 60% 42%)`;
    d.append(icon(...I.laptop));
    return d;
  }
  let toastTimer;
  function toast(msg) {
    const t = $('toast');
    t.textContent = msg;
    t.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { t.hidden = true; }, 2600);
  }

  // ------------------------------------------------------------ navigation
  let screen = 'home';
  const history = [];
  function show(name, { push = true } = {}) {
    if (push && name !== screen) history.push(screen);
    screen = name;
    document.querySelectorAll('.screen').forEach((s) => { s.hidden = s.dataset.screen !== name; });
    const sec = document.querySelector(`[data-screen="${name}"]`);
    $('topTitle').textContent = sec.dataset.title || '';
    $('backBtn').hidden = name === 'home';
    $('gearBtn').hidden = name !== 'home';
    if (name === 'home') { history.length = 0; renderHome(); }
    if (name === 'settings') renderSettings();
  }
  function back() {
    if (screen === 'connect') stopShare();
    const prev = history.pop() || 'home';
    show(prev, { push: false });
  }
  $('backBtn').addEventListener('click', back);
  $('gearBtn').addEventListener('click', () => show('settings'));
  $('infoBtn').addEventListener('click', () => show('settings'));
  $('statusBtn').addEventListener('click', () => show('settings'));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && screen !== 'home' && $('offer').hidden) back(); });

  // ------------------------------------------------------------ status
  let status = null;
  const STATE = {
    online: 'Pronto',
    limited: 'Rete limitata',
    connecting: 'Connessione alla rete…',
    offline: 'Nessuna rete',
  };
  async function refreshStatus() {
    try { status = await invoke('status'); } catch { return; }
    $('statusDot').className = `dot ${status.state}`;
    $('statusText').textContent = status.symmetric_nat && status.state === 'online'
      ? 'Pronto · rete restrittiva' : STATE[status.state];
    if (screen === 'settings') renderNet();
  }

  // ------------------------------------------------------------ contacts
  let contacts = [];
  async function loadContacts() { contacts = await invoke('contacts'); }
  function deviceRow(c, onClick) {
    const b = el('button', 'item');
    const g = el('div', 'grow');
    g.append(el('div', 'title', c.name), el('div', 'sub', `Ultimo trasferimento: ${when(c.last_seen)}`));
    b.append(deviceIcon(c.name, c.peer_id), g, Object.assign(icon(...I.chev), { classList: 'chev' }));
    b.addEventListener('click', () => onClick(c));
    return b;
  }

  // ------------------------------------------------------------ home
  const transfers = new Map(); // id -> last TransferEvent
  function renderHome() {
    loadContacts().then(() => {
      const list = $('recentList');
      list.replaceChildren(...contacts.slice(0, 5).map((c) => deviceRow(c, (c) => startSendTo(c))));
      $('recentBlock').hidden = contacts.length === 0;
    });
    const active = [...transfers.values()].filter((t) => !isFinal(t.stage));
    $('activeBlock').hidden = active.length === 0;
    $('activeList').replaceChildren(...active.map((t) => {
      const b = el('button', 'item');
      const g = el('div', 'grow');
      g.append(el('div', 'title', `${t.direction === 'out' ? 'Invio a' : 'Ricezione da'} ${t.peer}`),
        el('div', 'sub', `${titleOf(t)} · ${STAGE_LABEL[t.stage]}`));
      const mb = el('div', 'mini-bar'); const i = el('i'); i.style.width = `${pct(t)}%`; mb.append(i); g.append(mb);
      b.append(tile(t.files[0]?.name || '', false, t.files.length > 1), g, Object.assign(icon(...I.chev), { classList: 'chev' }));
      b.addEventListener('click', () => openTransfer(t.id));
      return b;
    }));
  }
  $('goSend').addEventListener('click', () => { sendTarget = null; renderFiles(); show('files'); });
  $('goReceive').addEventListener('click', () => { resetReceive(); show('receive'); setTimeout(() => $('codeInput').focus(), 50); });

  // ------------------------------------------------------------ files
  let files = [];
  let sendTarget = null; // contact chosen from the recent list
  function startSendTo(c) { sendTarget = c; renderFiles(); show('files'); }

  async function addPaths(paths) {
    const sizes = await invoke('file_sizes', { paths });
    let skipped = 0;
    paths.forEach((p, i) => {
      if (sizes[i] === null) { skipped++; return; }
      if (!files.some((f) => f.path === p)) files.push({ path: p, name: p.split(/[\\/]/).pop(), size: sizes[i] });
    });
    if (skipped) toast('Le cartelle non sono ancora supportate: seleziona i file al loro interno');
    renderFiles();
  }
  async function pickFiles() {
    const res = await dialog.open({ multiple: true, directory: false, title: 'Scegli i file da inviare' });
    if (res) addPaths(Array.isArray(res) ? res : [res]);
  }
  $('pickFiles').addEventListener('click', pickFiles);
  $('addMore').addEventListener('click', pickFiles);

  function renderFiles() {
    const has = files.length > 0;
    $('dropEmpty').hidden = has;
    $('fileList').hidden = !has;
    $('drop').classList.toggle('has-files', has);
    $('addMore').hidden = !has;
    $('fileList').replaceChildren(...files.map((f, i) => {
      const r = el('div', 'file-row');
      const g = el('div', 'grow');
      g.append(el('div', 'name', f.name), el('div', 'size', human(f.size)));
      const rm = el('button', 'rm', '×');
      rm.title = 'Rimuovi';
      rm.addEventListener('click', () => { files.splice(i, 1); renderFiles(); });
      r.append(tile(f.name), g, rm);
      return r;
    }));
    const total = files.reduce((a, f) => a + f.size, 0);
    const td = $('toDevice');
    td.hidden = !sendTarget;
    if (sendTarget) {
      const g = el('span', 'grow', `A: ${sendTarget.name}`);
      const x = el('button', 'icon-btn small'); x.append(icon(...I.x));
      x.addEventListener('click', () => { sendTarget = null; renderFiles(); });
      td.replaceChildren(deviceIcon(sendTarget.name, sendTarget.peer_id), g, x);
    }
    $('continueBtn').disabled = !has;
    $('continueBtn').textContent = !has ? 'Continua'
      : sendTarget ? `Invia a ${sendTarget.name} (${human(total)})` : `Continua (${human(total)})`;
  }

  if (window.__TAURI__.webview) {
    window.__TAURI__.webview.getCurrentWebview().onDragDropEvent((e) => {
      const t = e.payload.type;
      $('drop').classList.toggle('over', t === 'over' || t === 'enter');
      if (t === 'drop' && e.payload.paths?.length) {
        if (screen === 'home') { sendTarget = null; show('files'); }
        if (screen === 'files') addPaths(e.payload.paths);
      }
    });
  }

  $('continueBtn').addEventListener('click', async () => {
    if (sendTarget) return sendDirect(sendTarget);
    await startShare();
    show('connect');
  });

  async function sendDirect(c) {
    try {
      const id = await invoke('send_to', { target: c.peer_id, paths: files.map((f) => f.path) });
      files = [];
      openTransfer(id, true);
    } catch (e) { toast(String(e)); }
  }

  // ------------------------------------------------------------ connect (code)
  let share = null; // { id, code, expiresAt }
  let expiryTimer;
  async function startShare() {
    stopShare();
    try {
      const s = await invoke('start_share', { paths: files.map((f) => f.path) });
      share = { id: s.id, code: s.code, expiresAt: Date.now() + s.expires_in * 1000 };
    } catch (e) { toast(String(e)); return; }
    $('codeText').textContent = share.code.replace('-', ' - ');
    try {
      const qr = qrcode(0, 'M');
      qr.addData(share.code);
      qr.make();
      $('qr').innerHTML = qr.createSvgTag({ cellSize: 4, margin: 0, scalable: true });
    } catch { $('qr').textContent = ''; }
    $('newCode').hidden = true;
    $('codeWaiting').hidden = false;
    $('expiry').classList.remove('expired');
    clearInterval(expiryTimer);
    const tick = () => {
      const left = Math.max(0, Math.round((share.expiresAt - Date.now()) / 1000));
      $('expiryText').textContent = left
        ? `Il codice scade tra ${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`
        : 'Codice scaduto';
      if (!left) {
        clearInterval(expiryTimer);
        $('expiry').classList.add('expired');
        $('codeWaiting').hidden = true;
        $('newCode').hidden = false;
      }
    };
    tick();
    expiryTimer = setInterval(tick, 1000);
    loadContacts().then(renderConnectRecent);
  }
  function stopShare() {
    clearInterval(expiryTimer);
    if (share) invoke('stop_share', { code: share.code });
    share = null;
  }
  $('newCode').addEventListener('click', startShare);
  $('copyCode').addEventListener('click', async () => {
    if (!share) return;
    await navigator.clipboard.writeText(share.code);
    toast('Codice copiato');
  });
  document.querySelectorAll('.seg').forEach((b) => b.addEventListener('click', () => {
    document.querySelectorAll('.seg').forEach((x) => x.classList.toggle('active', x === b));
    document.querySelectorAll('[data-tab-panel]').forEach((p) => { p.hidden = p.dataset.tabPanel !== b.dataset.tab; });
  }));
  function renderConnectRecent() {
    $('connectRecent').replaceChildren(...contacts.map((c) => deviceRow(c, (c) => { stopShare(); sendDirect(c); })));
    $('connectRecent').hidden = contacts.length === 0;
    $('noRecent').hidden = contacts.length > 0;
  }
  await listen('share-used', (e) => {
    if (share && share.id === e.payload) {
      clearInterval(expiryTimer);
      share = null;
      files = [];
      openTransfer(e.payload, true);
    }
  });

  // ------------------------------------------------------------ transfer
  let current = null; // id shown on the transfer screen
  const samples = new Map();
  const STAGE_LABEL = {
    searching: 'ricerca', connecting: 'connessione', waiting: 'in attesa di conferma',
    transferring: 'in corso', verifying: 'verifica', done: 'completato',
    failed: 'non riuscito', rejected: 'rifiutato', cancelled: 'annullato',
  };
  const isFinal = (s) => ['done', 'failed', 'rejected', 'cancelled'].includes(s);
  const pct = (t) => (t.total ? Math.min(100, Math.floor((t.done / t.total) * 100)) : 0);
  const titleOf = (t) => (t.files.length === 1 ? t.files[0].name : files_label(t.files.length));

  function openTransfer(id, replace) {
    current = id;
    if (replace) history.length = 0;
    show('transfer', { push: !replace });
    if (replace) history.push('home');
    renderTransfer();
  }

  function stepsFor(t) {
    const order = t.direction === 'out'
      ? [['searching', 'Ricerca del dispositivo'], ['connecting', 'Connessione diretta'], ['waiting', 'Conferma del destinatario'], ['transferring', 'Trasferimento'], ['verifying', 'Verifica integrità file']]
      : [['connecting', 'Connessione diretta'], ['transferring', 'Trasferimento'], ['verifying', 'Verifica integrità file']];
    const rank = { searching: 0, connecting: 1, waiting: 2, transferring: 3, verifying: 4, done: 5 };
    const cur = rank[t.stage] ?? 0;
    return order.map(([s, label]) => {
      const r = rank[s];
      const state = cur > r || (t.direction === 'in' && s === 'connecting') ? 'done' : cur === r ? 'active' : '';
      const li = el('li', state);
      const ic = el('span', 'ic');
      if (state === 'done') ic.append(icon(...I.check));
      li.append(ic, el('span', '', label));
      if (s === 'connecting' && state === 'done' && t.via) li.append(el('span', 'note-inline', t.via));
      return li;
    });
  }

  function renderTransfer() {
    const t = transfers.get(current);
    if (!t || screen !== 'transfer') return;
    const final = isFinal(t.stage);
    $('tProgress').hidden = final;
    $('tResult').hidden = !final;
    $('topTitle').textContent = final ? '' : t.direction === 'out' ? 'Invio in corso…' : 'Ricezione in corso…';
    if (!final) {
      $('tTile').replaceWith(Object.assign(tile(t.files[0]?.name || '', true, t.files.length > 1), { id: 'tTile' }));
      $('tName').textContent = titleOf(t);
      $('tSub').textContent = `${human(t.total)} · ${t.direction === 'out' ? 'a' : 'da'} ${t.peer}`;
      const p = pct(t);
      const moving = t.stage === 'transferring' || t.stage === 'verifying';
      $('tBar').parentElement.classList.toggle('indeterminate', !moving);
      $('tBar').style.width = moving ? `${p}%` : '';
      $('tPct').textContent = moving ? `${p}%` : '';
      // Speed from real byte counts over the last ~3 s.
      const now = performance.now();
      const s = samples.get(t.id) || [];
      s.push([now, t.done]);
      while (s.length > 2 && now - s[0][0] > 3000) s.shift();
      samples.set(t.id, s);
      const speed = s.length > 1 && now - s[0][0] > 300 ? ((t.done - s[0][1]) / (now - s[0][0])) * 1000 : 0;
      $('tDone').textContent = `${human(t.done)} di ${human(t.total)}`;
      $('tSpeed').textContent = moving && speed > 0 ? `${human(speed)}/s` : '—';
      $('tEta').textContent = moving && speed > 0 ? duration((t.total - t.done) / speed) : '—';
      $('tSteps').replaceChildren(...stepsFor(t));
      return;
    }
    const ok = t.stage === 'done';
    const ri = $('rIcon');
    ri.className = `result-icon ${ok ? 'ok' : t.stage === 'failed' ? 'bad' : 'neutral'}`;
    ri.replaceChildren(icon(...(ok ? I.check : I.x)));
    $('rTitle').textContent = ok ? 'Trasferimento completato!'
      : t.stage === 'rejected' ? 'Invio rifiutato'
        : t.stage === 'cancelled' ? 'Trasferimento annullato' : 'Trasferimento non riuscito';
    $('rSub').textContent = `${human(t.total)} · ${files_label(t.files.length)} · ${t.direction === 'out' ? 'a' : 'da'} ${t.peer}`;
    $('rMsg').hidden = !t.message || ok;
    $('rMsg').textContent = t.message || '';
    $('rDetails').hidden = !t.detail || ok;
    $('rDetailText').textContent = t.detail || '';
    $('openFolder').hidden = !(ok && t.direction === 'in');
  }

  $('cancelBtn').addEventListener('click', () => { if (current) invoke('cancel', { id: current }); });
  $('closeResult').addEventListener('click', () => show('home'));
  $('openFolder').addEventListener('click', () => {
    const t = transfers.get(current);
    if (t?.folder) invoke('reveal_path', { path: t.folder });
  });

  await listen('transfer', (e) => {
    const t = e.payload;
    transfers.set(t.id, t);
    if (screen === 'transfer' && current === t.id) renderTransfer();
    else if (screen === 'home') renderHome();
    if (t.direction === 'in' && t.stage === 'transferring' && t.done === 0 && pendingAccept === t.id) {
      pendingAccept = null;
      openTransfer(t.id, true);
    }
  });
  await listen('cancelled', (e) => {
    const t = transfers.get(e.payload);
    if (t) {
      t.stage = 'cancelled';
      t.message = t.direction === 'in' ? 'La parte già ricevuta è conservata: se ti reinviano il file, si riparte da lì.' : null;
      if (screen === 'transfer' && current === t.id) renderTransfer();
    }
  });

  // ------------------------------------------------------------ receive with code
  const codeSteps = [['searching', 'Ricerca del dispositivo'], ['connecting', 'Connessione diretta'], ['requesting', 'Richiesta dei file']];
  function resetReceive() {
    $('codeInput').value = '';
    $('receiveBtn').disabled = true;
    $('codeSteps').hidden = true;
    $('codeError').hidden = true;
    $('codeDetails').hidden = true;
  }
  $('codeInput').addEventListener('input', (e) => {
    const raw = e.target.value.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 8);
    e.target.value = raw.length > 4 ? `${raw.slice(0, 4)}-${raw.slice(4)}` : raw;
    $('receiveBtn').disabled = raw.length !== 8;
  });
  $('codeInput').addEventListener('keydown', (e) => { if (e.key === 'Enter' && !$('receiveBtn').disabled) $('receiveBtn').click(); });
  $('receiveBtn').addEventListener('click', async () => {
    $('codeError').hidden = true;
    $('codeDetails').hidden = true;
    try {
      await invoke('receive_code', { code: $('codeInput').value });
      $('receiveBtn').disabled = true;
    } catch (e) {
      $('codeError').textContent = String(e);
      $('codeError').hidden = false;
    }
  });
  await listen('code', (e) => {
    const c = e.payload;
    if (c.stage === 'failed') {
      $('codeSteps').hidden = true;
      $('codeError').textContent = c.message;
      $('codeError').hidden = false;
      $('codeDetails').hidden = !c.detail;
      $('codeDetailText').textContent = c.detail || '';
      $('receiveBtn').disabled = false;
      return;
    }
    const idx = codeSteps.findIndex(([s]) => s === c.stage);
    $('codeSteps').hidden = false;
    $('codeSteps').replaceChildren(...codeSteps.map(([, label], i) => {
      const li = el('li', i < idx ? 'done' : i === idx ? 'active' : '');
      const ic = el('span', 'ic');
      if (i < idx) ic.append(icon(...I.check));
      li.append(ic, el('span', '', label));
      return li;
    }));
  });

  // ------------------------------------------------------------ incoming offers
  const offers = [];
  let pendingAccept = null;
  function showOffer() {
    const o = offers[0];
    if (!o) { $('offer').hidden = true; return; }
    $('offerFrom').textContent = o.from;
    avatar($('offerAvatar'), o.from, o.peer_id);
    $('offerSize').textContent = `${human(o.total)} · ${files_label(o.files.length)}`;
    $('offerFiles').replaceChildren(...o.files.slice(0, 100).map((f) => {
      const li = el('li');
      li.append(el('span', '', f.name), el('span', '', human(f.size)));
      return li;
    }));
    $('offer').hidden = false;
    $('offerAccept').focus();
  }
  async function answer(accept) {
    const o = offers.shift();
    if (o) {
      if (accept) pendingAccept = o.id;
      await invoke('answer_offer', { id: o.id, accept });
      if (!accept && screen === 'receive') resetReceive();
    }
    showOffer();
  }
  $('offerAccept').addEventListener('click', () => answer(true));
  $('offerReject').addEventListener('click', () => answer(false));
  await listen('offer', (e) => { offers.push(e.payload); if (offers.length === 1) showOffer(); });
  await listen('offer-closed', (e) => {
    const i = offers.findIndex((o) => o.id === e.payload);
    if (i >= 0) { offers.splice(i, 1); if (i === 0) showOffer(); }
  });

  // ------------------------------------------------------------ settings
  let settings = null;
  async function renderSettings() {
    settings = await invoke('settings');
    $('setName').value = settings.device_name;
    $('setFolder').textContent = settings.download_dir.replace(/^\/Users\/[^/]+/, '~');
    $('setNotify').checked = settings.notifications;
    $('setAuto').checked = settings.auto_accept_contacts;
    renderNet();
  }
  async function saveSettings() { await invoke('save_settings', { settings }); }
  $('setName').addEventListener('change', (e) => { settings.device_name = e.target.value; saveSettings(); toast('Nome aggiornato'); });
  $('setNotify').addEventListener('change', (e) => { settings.notifications = e.target.checked; saveSettings(); });
  $('setAuto').addEventListener('change', (e) => { settings.auto_accept_contacts = e.target.checked; saveSettings(); });
  $('setFolderBtn').addEventListener('click', async () => {
    const dir = await dialog.open({ directory: true, title: 'Cartella per i file ricevuti' });
    if (dir) { settings.download_dir = dir; await saveSettings(); renderSettings(); }
  });
  $('copyTicket').addEventListener('click', async () => {
    if (!status) return;
    await navigator.clipboard.writeText(status.ticket);
    toast('Indirizzo permanente copiato');
  });
  function renderNet() {
    if (!status) return;
    const v = $('netVerdict');
    v.className = `verdict ${status.state}`;
    const verdicts = {
      online: ['Tutto a posto', 'Gli altri dispositivi possono trovarti ovunque si trovino.'],
      limited: ['Rete limitata', 'In attesa di un relay per farti trovare. Di solito basta qualche secondo.'],
      connecting: ['Connessione in corso', 'Ricerca dei nodi pubblici della rete…'],
      offline: ['Nessuna rete', 'Nessun nodo raggiungibile: controlla la connessione, oppure la rete blocca il traffico non web.'],
    };
    let [title, text] = verdicts[status.state];
    if (status.symmetric_nat) {
      title = 'Rete restrittiva (NAT simmetrico)';
      text = 'Da questa rete il collegamento diretto riesce solo verso reti “facili”. Tipico di 4G/5G e reti aziendali.';
    }
    v.replaceChildren(el('b', '', title), el('span', '', text));
    $('netRelays').textContent = status.reservations ? `${status.reservations} attivi` : 'nessuno';
    $('netPeers').textContent = status.dht_peers;
    $('netObserved').textContent = [...new Set(status.observed.map((a) => a.split('/')[2]))].join(', ') || 'non ancora noto';
    $('netHints').replaceChildren(...status.hints.map((h) => el('li', '', h)));
  }

  await listen('net', refreshStatus);
  show('home', { push: false });
  refreshStatus();
  setInterval(refreshStatus, 2500);
})();
