# Architettura

## 1. Schema

```
 ┌──────────────── dispositivo (Windows / macOS / Linux) ────────────────┐
 │  UI: oggi CLI `lso`; in seguito Flutter desktop (flutter_rust_bridge)  │
 │ ───────────────────────────────────────────────────────────────────── │
 │  lso-core (Rust, libreria)                                            │
 │                                                                       │
 │  CONTROL PLANE (node.rs)                 DATA PLANE (transfer.rs)     │
 │  ├ identità Ed25519 → PeerId             ├ /lso/transfer/1.0.0        │
 │  ├ Kademlia (client) → DHT IPFS          ├ offerta → accetta          │
 │  ├ identify → "come mi vedono"           ├ stream con ripresa         │
 │  ├ relay v2 client → prenotazioni        ├ BLAKE3 su tutto il file    │
 │  ├ DCUtR → hole punching                 └ progresso = byte reali     │
 │  ├ UPnP, mDNS                                     ▲                   │
 │  └ diagnostica NAT                                │ solo connessioni  │
 │                                                   │ DIRETTE           │
 │                       direct_only.rs ─────────────┘ (vincolo nel codice)│
 │  Trasporti: QUIC (UDP) + TCP/Noise/Yamux, stessa porta per dial/listen │
 └───────────────────────────────────────────────────────────────────────┘
          │  segnalazione (KB)                     │ file (GB)
          ▼                                        ▼
  nodi pubblici di terzi                  l'altro dispositivo, direttamente
  (DHT IPFS / relay v2 / `lso infra`)     attraverso i due NAT
```

## 2. Flussi

**Ricezione** (`lso receive`)
1. Avvio, bind di QUIC e TCP sulla stessa porta. Solo dopo il bind si
   contattano i bootstrap, così le connessioni in uscita riusano la porta di
   ascolto e l'indirizzo osservato dagli altri è quello "bucabile".
2. identify: i nodi pubblici comunicano l'IP:porta osservato.
3. Tra i peer che supportano `/libp2p/circuit/relay/0.2.0/hop` se ne scelgono
   2 con indirizzo pubblico, e si prende una prenotazione su ciascuno.
4. Annuncio nella DHT (provider record sotto `H(PeerId)`) con gli indirizzi
   `…/p2p-circuit`. Viene ripetuto ogni 10 minuti e a ogni cambio di
   indirizzo.
5. Stampa del ticket.

**Invio** (`lso send <ticket|PeerId> file…`)
1. Avvio e attesa dell'indirizzo osservato: senza, DCUtR non ha nulla da
   proporre.
2. Dial degli indirizzi del ticket e, in parallelo, ricerca del PeerId nella
   DHT. Il dial parte appena la DHT risponde, perché Kademlia espone gli
   indirizzi del provider solo mentre la query è attiva.
3. Si stabilisce la connessione relayed. Su di essa passa **solo** DCUtR: il
   protocollo di trasferimento lì non esiste.
4. DCUtR: scambio degli indirizzi osservati e misura dell'RTT, poi dial
   simultaneo (QUIC e TCP).
5. Appena esiste una connessione non relayed, si apre lo stream dati.
   Scaduti 60 s senza, il comando termina con codice 2 e una diagnosi
   (`ConnectFailure::explain`).

**Stessa LAN**: mDNS scopre il peer e la connessione è diretta subito.

## 3. Struttura del progetto

```
Cargo.toml                   workspace
crates/lso-core/             libreria (tutta la logica, API async)
  src/config.rs              bootstrap, protocolli, timeout
  src/identity.rs            chiave del dispositivo
  src/node.rs                event loop libp2p, control plane, diagnostica
  src/direct_only.rs         vincolo: protocollo dati solo su conn. dirette
  src/rendezvous.rs          chiave DHT derivata dal PeerId
  src/ticket.rs              formato ticket lso1…
  src/transfer.rs            protocollo di trasferimento, ripresa, BLAKE3
crates/lso-cli/              binario `lso` (receive, send, diag, infra, id)
tests/netlab/                laboratorio NAT (namespace + iptables) e scenari
tests/gen_file.py            file deterministici per i test tra macchine
.github/workflows/ci.yml     build Win/Mac/Linux, laboratorio, test Internet reale
docs/                        analisi, architettura, test
```

Perché una libreria con una CLI sopra: la UI futura (Flutter) chiamerà le
stesse funzioni (`Node::start`, `connect_direct`, `send_files`,
`read_offer`/`accept`) tramite flutter_rust_bridge. La CLI è anche lo
strumento di test e diagnosi sul campo.

## 4. Valutazione critica di Flutter + Rust + libp2p

### Rust per il core: sì

- rust-libp2p ha tutto ciò che serve, già interoperabile con la rete IPFS:
  QUIC, TCP+Noise+Yamux, Kademlia, relay v2, DCUtR, identify, UPnP, mDNS.
- Un unico binario nativo per Windows, macOS e Linux; cross-compilabile
  anche per iOS e Android.
- Memory safety in un componente esposto alla rete.

Contro reali, emersi scrivendo questo codice:

- **API instabile**: breaking change a ogni minor release (0.5x). Va
  aggiornato attivamente.
- **Spigoli nel caso "dispositivo dietro NAT"**, che ho dovuto aggirare:
  - la prenotazione relay veniva confermata come indirizzo esterno e faceva
    passare Kademlia in modalità *server* anche dietro NAT (inquinando la
    DHT): modalità client forzata;
  - gli indirizzi dei provider sono disponibili solo durante la query:
    bisogna fare il dial dentro l'evento;
  - `libp2p-stream` non permette di scegliere su quale connessione aprire lo
    stream: da qui il wrapper `DirectOnly`;
  - subito dopo il hole punching possono esistere più connessioni dirette, e
    quelle ridondanti vengono chiuse: serve un retry sull'apertura dello
    stream.
- **NAT traversal meno sofisticato di Tailscale/iroh**: niente predizione
  delle porte per i "hard NAT", niente migrazione delle connessioni QUIC,
  nessun uso di più percorsi in parallelo.

### libp2p vs alternative

- **libp2p (scelto)**: l'unica pila che offre *insieme* una DHT pubblica
  enorme e relay di segnalazione già esistenti e gestiti da terzi. È ciò che
  rende possibile "nessun server mio".
- **iroh**: NAT traversal migliore, API più semplice. Ma il modello prevede
  relay (default: quelli di n0) che fanno anche da ripiego per i dati, e
  discovery via server DNS/pkarr di n0. Per il vincolo "zero server miei e
  dati mai via relay" significherebbe gestire server propri o dipendere da un
  unico fornitore. Da rivalutare se il vincolo si ammorbidisce.
- **WebRTC**: ICE è lo standard più maturo, ma la segnalazione resta a
  carico tuo (server) e il ripiego naturale è TURN, cioè un relay dei dati.

### Flutter per la UI: sì sul desktop, con riserve sul mobile

- Pro: un solo codebase per Windows, macOS, Linux, Android e iOS; bridge
  maturo verso Rust (flutter_rust_bridge v2 con stream di eventi, adatto a
  `NodeEvent` e `Progress`).
- Contro:
  - **iOS non può ricevere in background**: dopo pochi secondi il sistema
    sospende l'app e chiude i socket. "Ricevere quando arriva" come AirDrop
    richiederebbe notifiche push, cioè APNs più un server che le invii: un
    server centrale, contro il requisito. Su iOS si può ricevere solo con
    l'app in primo piano.
  - Android permette un foreground service, con notifica persistente.
  - Le reti mobili sono spesso dietro CGNAT simmetrico: dal telefono in 4G/5G
    la percentuale di successo cala sensibilmente.
  - Due toolchain (Dart e Rust) da mantenere e da firmare/notarizzare
    (macOS).
- Alternativa: **Tauri** (Rust + webview) se il target resta solo desktop.
  Richiede meno lavoro di integrazione, perché è tutto Rust.

**Raccomandazione**: core Rust (fatto), CLI per validare la rete sul campo
(fatto), poi shell Flutter desktop. Il mobile va trattato come progetto a
parte, con limiti dichiarati.

## 5. Roadmap tecnica (in ordine di impatto sulla connettività)

1. **Recupero peer noti**: salvare su disco i nodi DHT/relay già visti,
   per ridurre la dipendenza dai bootstrap di Protocol Labs.
2. **AutoNAT v2**: confermare l'eventuale raggiungibilità pubblica. I nodi
   pubblici potrebbero allora offrire relay di segnalazione agli altri
   utenti (reciprocità).
3. **Seconda via di scoperta**: Mainline DHT + pkarr (record firmati).
4. **Codici brevi con PAKE** (SPAKE2, stile magic-wormhole) al posto del
   ticket lungo.
5. Priorità a IPv6 quando entrambi lo hanno; NAT-PMP/PCP oltre a UPnP.
6. Cartelle, più file in parallelo, migrazione delle connessioni.
7. Predizione delle porte per NAT simmetrici (sperimentale, probabilistica).
