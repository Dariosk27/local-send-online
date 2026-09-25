// Design preview only: a fake backend so the UI can be opened in a normal
// browser (python3 -m http.server in app/ui). Never loaded inside the app,
// where window.__TAURI__ exists. ?s=<scenario> picks the screen to show.
(function () {
  const handlers = {};
  const emit = (name, payload) => (handlers[name] || []).forEach((h) => h({ payload }));
  const scenario = new URLSearchParams(location.search).get('s') || 'home';
  const now = Math.floor(Date.now() / 1000);
  const files = [
    { path: '/Users/dario/Video_Progetto.mp4', size: 2400000000 },
    { path: '/Users/dario/Foto_evento.zip', size: 850000000 },
    { path: '/Users/dario/Documenti.pdf', size: 120000000 },
    { path: '/Users/dario/Copertina.png', size: 4800000 },
  ];
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        switch (cmd) {
          case 'status': return {
            peer_id: '12D3KooWQkYGdSSkzv1e88rwHsxZ1yxLKDMe2e4BQGp3BcWd1Hwe', ticket: 'lso1…',
            state: 'online', reservations: 2, observed: ['/ip4/93.41.12.7/udp/51820/quic-v1'], dht_peers: 41,
            hints: [], symmetric_nat: false,
          };
          case 'contacts': return [
            { peer_id: 'a1', name: 'MacBook di Luca', last_seen: now - 3600 },
            { peer_id: 'b2', name: 'PC Ufficio', last_seen: now - 86400 - 7200 },
            { peer_id: 'c3', name: 'iMac di Sara', last_seen: now - 86400 * 12 },
          ];
          case 'settings': return { device_name: 'MacBook di Dario', download_dir: '/Users/dario/Downloads/LocalSendOnline', notifications: true, auto_accept_contacts: false };
          case 'file_sizes': return args.paths.map((p) => files.find((f) => f.path === p)?.size ?? 1000);
          case 'start_share': return { id: 7, code: '7F3K-9H2P', expires_in: 300 };
          default: return null;
        }
      },
    },
    event: { listen: async (n, h) => { (handlers[n] ||= []).push(h); } },
    dialog: { open: async () => files.map((f) => f.path) },
  };
  const click = (id) => document.getElementById(id)?.click();
  const tr = (over) => Object.assign({
    id: 7, direction: 'out', stage: 'transferring', peer: 'MacBook di Luca',
    files: files.map((f) => ({ name: f.path.split('/').pop(), size: f.size })),
    total: 3374800000, done: 0, file_index: 0, via: 'Internet, QUIC', message: null, detail: null, folder: null,
  }, over);
  setTimeout(async () => {
    const wait = (ms) => new Promise((r) => setTimeout(r, ms));
    if (scenario === 'files' || scenario === 'connect') {
      click('goSend'); await wait(50); click('pickFiles'); await wait(100);
      if (scenario === 'connect') { click('continueBtn'); }
    }
    if (scenario === 'transfer' || scenario === 'done' || scenario === 'failed') {
      click('goSend'); await wait(50); click('pickFiles'); await wait(100); click('continueBtn'); await wait(100);
      emit('share-used', 7);
      emit('transfer', tr({ stage: 'waiting' }));
      if (scenario === 'transfer') {
        emit('transfer', tr({ done: 2200000000 }));
        await wait(700);
        emit('transfer', tr({ done: 2295000000 }));
      }
      if (scenario === 'done') emit('transfer', tr({ stage: 'done', done: 3374800000 }));
      if (scenario === 'failed') emit('transfer', tr({ stage: 'failed', message: "L'altro dispositivo è online, ma tra le vostre due reti non è possibile un collegamento diretto (succede di solito con reti mobili 4G/5G o aziendali). Prova da un'altra rete, per esempio il Wi‑Fi di casa su uno dei due dispositivi.", detail: 'hole punching (DCUtR) fallito: Giving up after 3 dial attempts' }));
    }
    if (scenario === 'receive') {
      click('goReceive'); await wait(50);
      const i = document.getElementById('codeInput'); i.value = '7f3k9h2'; i.dispatchEvent(new Event('input'));
      emit('code', { code: '7F3K-9H2P', stage: 'connecting' });
    }
    if (scenario === 'offer') {
      emit('offer', { id: 9, peer_id: 'a1', from: 'Luca (MacBook)', total: 5200000000, known_contact: true,
        files: [{ name: 'Video_Progetto.mp4', size: 2400000000 }, { name: 'Foto_evento.zip', size: 850000000 }, { name: 'Documenti.pdf', size: 120000000 }, { name: 'Cartella_Design.zip', size: 1830000000 }] });
    }
    if (scenario === 'home-active') {
      emit('transfer', tr({ id: 3, direction: 'in', peer: 'PC Ufficio', done: 1100000000 }));
    }
    if (scenario === 'settings') click('gearBtn');
  }, 250);
})();
