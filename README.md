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

Flusso:
1. **Invia file** → scegli o trascina i file → **Continua**.
2. L'app mostra un **codice breve** (es. `7F3K-9H2P`, anche come QR), valido
   5 minuti e usabile una volta sola. Lo detti o lo mandi all'altra persona.
3. L'altra persona apre **Ricevi file**, scrive il codice, conferma la
   richiesta: i file arrivano con avanzamento reale, velocità, tempo
   rimanente e verifica finale. **Annulla** interrompe (il parziale resta per
   riprendere).
4. Chi ha già scambiato file con te compare in **Dispositivi recenti**: la
   volta dopo basta un clic, niente codice.

Il codice serve solo a *trovarsi*: il mittente lo pubblica per pochi minuti
nella DHT come chiave derivata dal codice; chi lo digita trova il PeerId del
mittente, si collega direttamente e chiede i file. Il trasferimento resta
diretto, cifrato e con conferma esplicita.

Impostazioni: nome del dispositivo, cartella di ricezione, notifiche di
sistema, accettazione automatica dai dispositivi recenti, stato della rete.

Compilare in locale: `cd app/src-tauri && npx @tauri-apps/cli@2 build`
(su Linux servono `libwebkit2gtk-4.1-dev` e `libgtk-3-dev`).
Anteprima grafica nel browser: `cd app/ui && python3 -m http.server`,
poi `index.html?s=home|files|connect|transfer|done|receive|offer|settings`
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

lso share foto.zip           # stampa un codice breve (5 minuti)
lso get 7F3K-9H2P            # riceve con il codice

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
