// Design preview only: a fake backend so the UI can be opened in a normal
// browser (python3 -m http.server in app/ui). Never loaded inside the app,
// where window.__TAURI__ exists.
(function () {
  const handlers = {};
  const emit = (name, payload) => (handlers[name] || []).forEach((h) => h({ payload }));
  const ticket = 'lso1eyJwIjoiMTJEM0tvb1dRa1lHZFNTa3p2MWU4OHJ3SHN4WjF5eExLRE1lMmU0QlFHcDNCY1dkMUh3ZSIsImEiOlsiL2lwNC84MC4xMC4wLjIvdGNwLzQwMDEvcDJwLzEyRDNLb29XS1ZFa0pHZjlYTTZGRWl1NkFUZGRnNUdRMnlSYXFBcjE5dkhhOWdReVpNcWcvcDJwLWNpcmN1aXQiXX0';
  const scenario = new URLSearchParams(location.search).get('s') || 'main';
  window.__TAURI__ = {
    core: {
      invoke: async (cmd) => {
        if (cmd === 'status') return {
          peer_id: '12D3KooWQkYGdSSkzv1e88rwHsxZ1yxLKDMe2e4BQGp3BcWd1Hwe', ticket,
          device_name: 'MacBook di Dario', download_dir: '/Users/dario/Downloads/LocalSendOnline',
          state: 'online', reservations: 2, observed: ['/ip4/93.41.12.7/udp/51820/quic-v1'], dht_peers: 41,
          hints: [], symmetric_nat: false,
        };
        if (cmd === 'contacts') return [
          { peer_id: '12D3KooWAbc1', name: 'PC di Luca', last_seen: 2 },
          { peer_id: '12D3KooWXyz9', name: 'iMac ufficio', last_seen: 1 },
        ];
        if (cmd === 'file_sizes') return [734003200];
        return null;
      },
    },
    event: { listen: async (n, h) => { (handlers[n] ||= []).push(h); } },
    dialog: { open: async () => null },
  };
  setTimeout(() => {
    if (scenario === 'main' || scenario === 'offer') {
      emit('transfer', { id: 1, direction: 'out', stage: 'done', peer: 'PC di Luca', title: 'Foto vacanze.zip', total: 1843000000, done: 1843000000, file_index: 0, file_count: 1, via: 'diretta via Internet, NAT attraversato (QUIC)' });
      emit('transfer', { id: 3, direction: 'out', stage: 'failed', peer: 'Dispositivo …9x2Lk', title: 'Presentazione.key', total: 52000000, done: 0, file_index: 0, file_count: 1,
        message: "Il destinatario è online, ma tra le vostre due reti non è possibile un collegamento diretto (di solito succede con reti mobili 4G/5G o aziendali). Prova da un'altra rete, per esempio il Wi‑Fi di casa su uno dei due dispositivi.",
        detail: 'Il peer è online e raggiungibile tramite relay (canale di segnalazione OK),\nma il hole punching (DCUtR) non è riuscito.\nCon questa combinazione di NAT/firewall una connessione diretta NON è possibile.' });
      emit('transfer', { id: 2, direction: 'in', stage: 'transferring', peer: 'iMac ufficio', title: 'Montaggio_finale.mov', total: 4200000000, done: 1650000000, file_index: 0, file_count: 1, via: 'diretta in rete locale (QUIC)' });
      setTimeout(() => emit('transfer', { id: 2, direction: 'in', stage: 'transferring', peer: 'iMac ufficio', title: 'Montaggio_finale.mov', total: 4200000000, done: 1712000000, file_index: 0, file_count: 1, via: 'diretta in rete locale (QUIC)' }), 600);
    }
    if (scenario === 'offer') {
      emit('offer', { id: 9, peer_id: '12D3KooWAbc1', from: 'PC di Luca', total: 912000000, known_contact: true,
        files: [{ name: 'Video compleanno.mp4', size: 880000000 }, { name: 'Lista invitati.pdf', size: 1200000 }, { name: 'Foto gruppo.heic', size: 30800000 }] });
    }
  }, 300);
})();
