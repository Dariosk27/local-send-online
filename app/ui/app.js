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

  const $ = (id) => document.getElementById(id);
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  };
  const icon = (paths) => {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    for (const d of paths) {
      const p = document.createElementNS(ns, 'path');
      p.setAttribute('d', d);
      svg.appendChild(p);
    }
    return svg;
  };
  const ICON_UP = ['M12 19V5', 'M6 11l6-6 6 6'];
  const ICON_DOWN = ['M12 5v14', 'M6 13l6 6 6-6'];
  const ICON_OK = ['M5 12l5 5 9-10'];
  const ICON_X = ['M6 6l12 12', 'M18 6L6 18'];
  const ICON_BOLT = ['M13 3L5 13h6l-1 8 8-10h-6z'];

  function human(n) {
    const u = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    while (n >= 1000 && i < u.length - 1) { n /= 1000; i++; }
    return `${n >= 100 || i === 0 ? Math.round(n) : n.toFixed(1)} ${u[i]}`;
  }
  function duration(s) {
    if (!isFinite(s) || s < 0) return '';
    if (s < 60) return `${Math.ceil(s)} s`;
    const m = Math.floor(s / 60);
    if (m < 60) return `${m} min ${Math.round(s % 60)} s`;
    return `${Math.floor(m / 60)} h ${m % 60} min`;
  }
  function initials(name) {
    const w = (name || '?').replace(/[^\p{L}\p{N} ]/gu, ' ').trim().split(/\s+/);
    return ((w[0] || '?')[0] + (w.length > 1 ? w[w.length - 1][0] : '')).toUpperCase();
  }
  function hue(s) {
    let h = 0;
    for (const c of s) h = (h * 31 + c.charCodeAt(0)) % 360;
    return h;
  }
  function paintAvatar(node, name, seed) {
    node.textContent = initials(name);
    const h = hue(seed || name || '');
    node.style.background = `linear-gradient(135deg, hsl(${h} 70% 58%), hsl(${(h + 40) % 360} 70% 52%))`;
  }
  let toastTimer;
  function toast(msg) {
    const t = $('toast');
    t.textContent = msg;
    t.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { t.hidden = true; }, 2200);
  }

  // ------------------------------------------------------------ status
  let status = null;
  let lastTicket = '';
  const STATE_TEXT = {
    connecting: 'Connessione alla rete…',
    online: 'Online',
    limited: 'Rete limitata',
    offline: 'Offline',
  };
  const REACH_TEXT = {
    connecting: 'Connessione alla rete…',
    online: 'Raggiungibile da Internet',
    limited: 'Raggiungibile solo in parte',
    offline: 'Nessuna connessione alla rete',
  };

  async function refreshStatus() {
    try {
      status = await invoke('status');
    } catch (e) {
      return;
    }
    $('statusPill').dataset.state = status.state;
    $('statusText').textContent = STATE_TEXT[status.state];
    $('reachText').textContent = status.symmetric_nat && status.state !== 'offline'
      ? 'Rete restrittiva: alcune connessioni non riusciranno'
      : REACH_TEXT[status.state];
    if (document.activeElement !== $('deviceName')) $('deviceName').value = status.device_name;
    paintAvatar($('myAvatar'), status.device_name, status.peer_id);
    $('folderText').textContent = prettyPath(status.download_dir);

    // The code is worth sharing once we are reachable (relay reservation).
    const ready = status.state === 'online' || status.state === 'limited';
    if (ready && status.ticket !== lastTicket) {
      lastTicket = status.ticket;
      renderQr(status.ticket);
      $('codeText').textContent = status.ticket;
      $('copyCode').disabled = false;
    }
    if (!$('netOverlay').hidden) renderNet();
  }

  function prettyPath(p) {
    return p.replace(/^\/Users\/[^/]+/, '~').replace(/^[A-Z]:\\Users\\[^\\]+/, '~');
  }

  function renderQr(text) {
    try {
      const qr = qrcode(0, 'L');
      qr.addData(text);
      qr.make();
      $('qr').innerHTML = qr.createSvgTag({ cellSize: 4, margin: 0, scalable: true });
    } catch (e) {
      $('qr').textContent = '';
    }
  }

  $('copyCode').addEventListener('click', async () => {
    await navigator.clipboard.writeText(lastTicket);
    toast('Codice copiato: incollalo in un messaggio a chi deve inviarti file');
  });

  $('deviceName').addEventListener('keydown', (e) => { if (e.key === 'Enter') e.target.blur(); });
  $('deviceName').addEventListener('change', async (e) => {
    await invoke('set_device_name', { name: e.target.value });
    toast('Nome aggiornato');
    refreshStatus();
  });

  $('openFolder').addEventListener('click', (e) => {
    if (e.target.id === 'changeFolder') return;
    invoke('open_download_dir');
  });
  $('changeFolder').addEventListener('click', async (e) => {
    e.stopPropagation();
    const dir = await window.__TAURI__.dialog.open({ directory: true, title: 'Cartella per i file ricevuti' });
    if (dir) {
      await invoke('set_download_dir', { dir });
      refreshStatus();
    }
  });

  // ------------------------------------------------------- network sheet
  $('statusPill').addEventListener('click', () => { $('netOverlay').hidden = false; renderNet(); });
  $('netClose').addEventListener('click', () => { $('netOverlay').hidden = true; });
  $('netOverlay').addEventListener('click', (e) => { if (e.target.id === 'netOverlay') e.target.hidden = true; });

  function renderNet() {
    if (!status) return;
    const v = $('netVerdict');
    v.className = `net-verdict ${status.state}`;
    const verdicts = {
      online: ['Tutto a posto', 'Altri dispositivi possono trovarti e proporti file ovunque si trovino.'],
      limited: ['Rete limitata', 'Conosciamo il tuo indirizzo pubblico ma non abbiamo ancora un relay per farti trovare. Riprova tra poco.'],
      connecting: ['Connessione in corso', 'Ricerca dei nodi pubblici della rete…'],
      offline: ['Nessuna rete', 'Nessun nodo pubblico raggiungibile. Controlla la connessione, oppure la rete blocca il traffico non web.'],
    };
    let [title, text] = verdicts[status.state];
    if (status.symmetric_nat) {
      title = 'Rete restrittiva (NAT simmetrico)';
      text = 'Da questa rete la connessione diretta funziona solo verso dispositivi su reti “facili” (casa con UPnP o porta aperta). Tipico di 4G/5G e reti aziendali.';
    }
    v.innerHTML = '';
    v.append(el('b', '', title), el('span', '', text));
    $('netRelays').textContent = status.reservations ? `${status.reservations} attivi` : 'nessuno';
    $('netPeers').textContent = status.dht_peers;
    $('netObserved').textContent = [...new Set(status.observed.map((a) => a.split('/').slice(0, 3).join('/').replace('/ip4/', '').replace('/ip6/', '')))].join(', ') || 'non ancora noto';
    $('netPeerId').textContent = status.peer_id;
    const hints = $('netHints');
    hints.innerHTML = '';
    for (const h of status.hints) hints.append(el('li', '', h));
  }

  // --------------------------------------------------------- contacts
  let contacts = [];
  let selected = null;

  async function refreshContacts() {
    contacts = await invoke('contacts');
    const box = $('contacts');
    box.innerHTML = '';
    for (const c of contacts.slice(0, 8)) {
      const chip = el('button', 'contact' + (selected === c.peer_id ? ' selected' : ''));
      const av = el('div', 'avatar');
      paintAvatar(av, c.name, c.peer_id);
      chip.append(av, el('span', '', c.name));
      chip.title = `${c.name}\n${c.peer_id}`;
      chip.addEventListener('click', () => {
        selected = selected === c.peer_id ? null : c.peer_id;
        $('target').value = selected ? '' : $('target').value;
        $('target').placeholder = selected ? `Invio a ${c.name}` : 'Incolla il codice del destinatario (lso1…)';
        refreshContacts();
        updateSendButton();
      });
      box.append(chip);
    }
    box.hidden = contacts.length === 0;
  }

  $('target').addEventListener('input', () => {
    if (selected) {
      selected = null;
      $('target').placeholder = 'Incolla il codice del destinatario (lso1…)';
      refreshContacts();
    }
    updateSendButton();
  });

  // ------------------------------------------------------------ files
  let files = [];

  async function addPaths(paths) {
    const sizes = await invoke('file_sizes', { paths });
    let skipped = 0;
    paths.forEach((p, i) => {
      if (sizes[i] === null) { skipped++; return; }
      if (files.some((f) => f.path === p)) return;
      files.push({ path: p, name: p.split(/[\\/]/).pop(), size: sizes[i] });
    });
    if (skipped) toast('Le cartelle non sono ancora supportate: seleziona i file');
    renderFiles();
  }

  function renderFiles() {
    const box = $('files');
    box.innerHTML = '';
    $('dropEmpty').hidden = files.length > 0;
    box.hidden = files.length === 0;
    files.forEach((f, i) => {
      const chip = el('div', 'file-chip');
      const rm = el('button', '', '×');
      rm.title = 'Rimuovi';
      rm.addEventListener('click', () => { files.splice(i, 1); renderFiles(); });
      chip.append(el('span', 'name', f.name), el('span', 'size', human(f.size)), rm);
      box.append(chip);
    });
    if (files.length) {
      const total = el('div', 'files-total');
      const more = el('button', 'link', 'Aggiungi altri');
      more.addEventListener('click', pickFiles);
      total.append(el('span', '', `${files.length} file · ${human(files.reduce((a, f) => a + f.size, 0))}`), more);
      box.append(total);
    }
    updateSendButton();
  }

  async function pickFiles() {
    const res = await window.__TAURI__.dialog.open({ multiple: true, directory: false, title: 'Scegli i file da inviare' });
    if (res) addPaths(Array.isArray(res) ? res : [res]);
  }
  $('pickFiles').addEventListener('click', pickFiles);

  if (window.__TAURI__.webview) {
    window.__TAURI__.webview.getCurrentWebview().onDragDropEvent((e) => {
      const t = e.payload.type;
      if (t === 'over' || t === 'enter') $('drop').classList.add('over');
      else $('drop').classList.remove('over');
      if (t === 'drop' && e.payload.paths?.length) addPaths(e.payload.paths);
    });
  }

  function updateSendButton() {
    const hasTarget = selected || $('target').value.trim().length > 20;
    $('sendBtn').disabled = !(files.length && hasTarget);
  }

  $('sendBtn').addEventListener('click', async () => {
    const target = selected || $('target').value.trim();
    try {
      await invoke('send', { target, files: files.map((f) => ({ path: f.path })) });
      files = [];
      renderFiles();
    } catch (e) {
      toast(String(e));
    }
  });

  // -------------------------------------------------------- transfers
  const transfers = new Map();
  const STAGE = {
    searching: 'Ricerca del destinatario…',
    connecting: 'Apertura connessione diretta…',
    direct: 'Collegato',
    waiting: 'In attesa che accetti…',
    transferring: 'In corso',
    done: 'Completato',
    failed: 'Non riuscito',
    rejected: 'Rifiutato',
  };

  function renderTransfer(ev) {
    $('emptyActivity').hidden = true;
    let t = transfers.get(ev.id);
    if (!t) {
      const row = el('div', 'transfer');
      const ic = el('div', 't-icon');
      const title = el('div', 't-title');
      const right = el('div', 't-right');
      const stage = el('span', 'stage');
      right.append(stage);
      const meta = el('div', 't-meta');
      const bar = el('div', 'bar');
      bar.append(el('i'));
      row.append(ic, title, right, meta, bar);
      $('transfers').prepend(row);
      t = { row, ic, title, right, stage, meta, bar, samples: [], ev };
      transfers.set(ev.id, t);
    }
    t.ev = ev;
    const s = ev.stage;
    const active = !['done', 'failed', 'rejected'].includes(s);
    t.row.className = `transfer ${s} ${active ? 'active' : ''}`;

    t.ic.replaceChildren(icon(s === 'done' ? ICON_OK : (s === 'failed' || s === 'rejected') ? ICON_X : ev.direction === 'out' ? ICON_UP : ICON_DOWN));
    t.title.replaceChildren(
      document.createTextNode(ev.title + ' '),
      el('span', 'peer', ev.direction === 'out' ? `→ ${ev.peer}` : `da ${ev.peer}`),
    );
    t.stage.textContent = STAGE[s];

    // Speed from real byte counts, smoothed over the last ~3 s.
    const now = performance.now();
    t.samples.push([now, ev.done]);
    while (t.samples.length > 2 && now - t.samples[0][0] > 3000) t.samples.shift();
    const [t0, d0] = t.samples[0];
    const speed = now - t0 > 400 ? ((ev.done - d0) / (now - t0)) * 1000 : 0;

    const meta = [];
    if (s === 'transferring') {
      meta.push(`${human(ev.done)} di ${human(ev.total)}`);
      if (speed > 0) meta.push(`${human(speed)}/s`, `mancano ${duration((ev.total - ev.done) / speed)}`);
      if (ev.file_count > 1) meta.push(`file ${ev.file_index + 1}/${ev.file_count}`);
    } else if (s === 'done') {
      meta.push(`${human(ev.total)} · integrità verificata`);
    } else {
      meta.push(`${ev.file_count} file · ${human(ev.total)}`);
    }
    t.meta.replaceChildren(...meta.flatMap((m, i) => (i ? [el('span', '', '·'), el('span', '', m)] : [el('span', '', m)])));
    if (ev.via && s !== 'failed') {
      const via = el('span', 'via');
      via.append(icon(ICON_BOLT), document.createTextNode(ev.via));
      t.meta.append(via);
    }

    const pct = ev.total ? (ev.done / ev.total) * 100 : 0;
    const indeterminate = ['searching', 'connecting', 'direct', 'waiting'].includes(s);
    t.bar.classList.toggle('indeterminate', indeterminate);
    t.bar.hidden = s === 'failed' || s === 'rejected';
    t.bar.firstChild.style.width = indeterminate ? '' : `${s === 'done' ? 100 : pct}%`;

    t.right.querySelectorAll('.btn').forEach((b) => b.remove());
    if (s === 'done' && ev.direction === 'in' && ev.detail) {
      const b = el('button', 'btn ghost', 'Mostra');
      b.addEventListener('click', () => invoke('reveal_path', { path: ev.detail }));
      t.right.prepend(b);
    }
    t.row.querySelector('.detail')?.remove();
    if (s === 'failed' && (ev.message || ev.detail)) {
      const box = el('div', 'detail');
      box.append(el('div', 'detail-msg', ev.message || ev.detail));
      if (ev.message && ev.detail) {
        const more = el('details');
        more.append(el('summary', '', 'Dettagli tecnici'), el('pre', '', ev.detail));
        box.append(more);
      }
      t.row.append(box);
    }
    if (s === 'done' || s === 'failed') refreshContacts();
  }

  await listen('transfer', (e) => renderTransfer(e.payload));

  // ------------------------------------------------------------ offers
  const offers = [];
  function showOffer() {
    const o = offers[0];
    if (!o) { $('offerOverlay').hidden = true; return; }
    $('offerFrom').textContent = o.from;
    paintAvatar($('offerAvatar'), o.from, o.peer_id);
    $('offerSummary').textContent = `${o.files.length} file · ${human(o.total)}`;
    $('offerKnown').hidden = !o.known_contact;
    const list = $('offerFiles');
    list.innerHTML = '';
    for (const f of o.files.slice(0, 200)) {
      const li = el('li');
      li.append(el('span', '', f.name), el('span', '', human(f.size)));
      list.append(li);
    }
    $('offerOverlay').hidden = false;
    $('offerAccept').focus();
  }
  async function answer(accept) {
    const o = offers.shift();
    if (o) await invoke('answer_offer', { id: o.id, accept });
    showOffer();
  }
  $('offerAccept').addEventListener('click', () => answer(true));
  $('offerReject').addEventListener('click', () => answer(false));
  await listen('offer', (e) => { offers.push(e.payload); if (offers.length === 1) showOffer(); });
  await listen('offer-closed', (e) => {
    const i = offers.findIndex((o) => o.id === e.payload);
    if (i >= 0) { offers.splice(i, 1); if (i === 0) showOffer(); }
  });

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') $('netOverlay').hidden = true;
  });

  await listen('net', () => refreshStatus());
  refreshStatus();
  refreshContacts();
  setInterval(refreshStatus, 2000);
})();
