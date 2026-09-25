# local-send-online

Trasferimento di file **diretto** tra due dispositivi su reti diverse, via
Internet, come AirDrop ma senza passare da WeTransfer/SwissTransfer, senza
account e senza server del progetto. I byte del file viaggiano solo sulla
connessione diretta tra i due dispositivi (hole punching). Se questa non si
può stabilire, il programma lo dice e spiega perché. Non ripiega mai su un
relay per i dati.

> **Limite fondamentale, prima di tutto**: la connessione diretta non è
> garantita. Con NAT simmetrici (tipici del 4G/5G e delle reti aziendali) o
> con firewall che bloccano UDP/TCP in ingresso è fisicamente impossibile
> senza un relay dei dati. Nelle misure pubbliche su libp2p riesce in circa il
> 70 % dei casi. Dettagli in [docs/ANALISI.md](docs/ANALISI.md).

## Documenti

1. [docs/ANALISI.md](docs/ANALISI.md): NAT traversal, discovery, IP
   dinamici, firewall, casi in cui è impossibile, perché "zero server" in
   senso assoluto non esiste.
2. [docs/ARCHITETTURA.md](docs/ARCHITETTURA.md): control plane e data plane,
   flussi, struttura del progetto, valutazione critica di Rust + libp2p +
   Flutter.
3. [docs/TEST.md](docs/TEST.md): laboratorio NAT, risultati, procedura
   Windows ↔ macOS.

## App desktop (macOS, Windows)

`app/` contiene l'app con interfaccia grafica (Tauri 2: finestra nativa,
UI in HTML/CSS, stesso core Rust della CLI). La CI produce
`LocalSendOnline.dmg` (Mac Intel + Apple Silicon) e
`LocalSendOnline-Setup.exe` (Windows).

- **Ricevi**: il tuo codice (testo + QR) da mandare a chi ti invia file;
  ogni invio chiede conferma.
- **Invia**: trascina i file, scegli un contatto o incolla un codice.
- **Attività**: avanzamento reale, velocità, verifica finale; se il
  collegamento diretto è impossibile, spiega perché in parole semplici.
- **Contatti**: i dispositivi con cui hai già scambiato file restano
  in elenco (ritrovati per PeerId anche se cambiano IP).

Compilare in locale: `cd app/src-tauri && npx @tauri-apps/cli@2 build`
(su Linux servono `libwebkit2gtk-4.1-dev` e `libgtk-3-dev`).
Anteprima grafica nel browser: `cd app/ui && python3 -m http.server`
(usa `dev/preview.js`, un backend finto solo per il design).

## Uso da riga di comando (CLI)

```
cargo build --release          # binario: target/release/lso(.exe)

# chi riceve
lso receive --dir ~/Downloads/lso
#  → stampa il PeerId e un ticket lso1…

# chi invia (altra rete, altro sistema operativo)
lso send lso1…  foto.zip video.mp4    # con il ticket
lso send 12D3KooW…  foto.zip          # oppure solo con il PeerId (ricerca DHT)

lso diag     # cosa vede Internet di questa rete: NAT simmetrico? UPnP? relay?
lso id       # PeerId di questo dispositivo
```

Esiti possibili dell'invio:
- `connessione DIRETTA stabilita via /ip4/…/udp/…/quic-v1 (hole punching)`,
  poi una barra di avanzamento sui byte reali e la conferma finale solo dopo
  la verifica BLAKE3 del ricevente;
- `IMPOSSIBILE stabilire una connessione diretta` con la diagnosi (exit code 2).

I trasferimenti interrotti riprendono dal punto in cui si erano fermati.

## Cosa usa di terzi (e cosa no)

- **Usa** i nodi pubblici della rete IPFS/libp2p per il *control plane*: entrare
  nella DHT, scoprire il proprio indirizzo pubblico, prenotare un relay che
  trasporta solo i messaggi di coordinamento (pochi KB). Nessuno di questi è
  del progetto; `lso infra` permette a chiunque abbia un IP pubblico di
  offrirne uno.
- **Non usa** server per i dati, né account, né Firebase/Supabase, né storage
  cloud.

## Stato

- ✅ core Rust + CLI, Windows/macOS/Linux (build e test in CI)
- ✅ laboratorio NAT con 6 scenari reali (cone, simmetrico, port forwarding,
  ripresa): tutti con l'esito atteso
- ⏳ test Internet reale Windows ↔ macOS: job CI predisposto; esito da
  osservare, dipende dai NAT dei runner
- ⏳ UI Flutter desktop (vedi roadmap in ARCHITETTURA)
