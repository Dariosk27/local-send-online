# Analisi tecnica: trasferimento file diretto via Internet, senza server propri

Questo documento viene prima del codice. Dice cosa è possibile, cosa non lo è
e perché. Le affermazioni sul comportamento dei NAT sono verificate nel
laboratorio di rete del repository (`tests/netlab`, NAT Linux reali in
network namespace; risultati in [TEST.md](TEST.md)).

## 0. In breve

| Domanda | Risposta |
|---|---|
| Si può trasferire un file direttamente tra due dispositivi su reti diverse senza un server mio? | **Sì, nella maggioranza dei casi.** Hole punching tramite NAT, coordinato da nodi pubblici di terzi. |
| Si può fare **sempre**? | **No.** Con certe combinazioni di NAT o firewall una connessione diretta è fisicamente impossibile. Resta solo il relay dei dati, che il progetto rifiuta per scelta. |
| Si può fare con **zero** infrastruttura di terzi? | **No, se entrambi i dispositivi sono dietro NAT.** Serve almeno un nodo raggiungibile pubblicamente che dica a ciascuno il proprio indirizzo pubblico e passi i messaggi di coordinamento. Può essere di chiunque (la rete IPFS pubblica, un amico, un tuo VPS). Non serve che sia tuo, ma deve esistere. |
| Quanto spesso fallisce? | La campagna di misura di libp2p/ProbeLab su DCUtR (Trautwein et al., 2024) ha trovato circa il **70 % ± 7 %** di successo, con TCP e QUIC simili. Tailscale dichiara percentuali più alte di connessioni dirette, ma usa più tecniche (port mapping, predizione delle porte, IPv6) e soprattutto **ha il relay DERP come ripiego**. Aspettati quindi che circa un tentativo su 4-5 tra reti "difficili" non possa diventare diretto. |

## 1. Control plane e data plane

- **Control plane**: tutto ciò che serve perché due dispositivi si trovino e
  si coordinino. Identità, scoperta (dov'è il peer adesso?), scoperta del
  proprio indirizzo pubblico, segnalazione per il hole punching. Pochi
  kilobyte, e qui la dipendenza da terzi è inevitabile (vedi §3).
- **Data plane**: i byte del file. **Restano diretti, sempre.** Nel codice
  non è solo una convenzione: il protocollo di trasferimento viene registrato
  solo sulle connessioni dirette (`crates/lso-core/src/direct_only.rs`). Su
  una connessione relayed il protocollo non esiste proprio: non viene
  annunciato né accettato. Se il hole punching fallisce, il trasferimento
  fallisce con una diagnosi, e non si ripiega sul relay.

Nel laboratorio, per un file di 100 MB trasferito tra due NAT, i nodi pubblici
vedono passare circa 25-75 KB in totale: identify, Kademlia, prenotazione relay
e messaggi DCUtR. Zero byte del file.

## 2. I quattro problemi, uno per uno

### 2.1 NAT traversal

Un dispositivo dietro NAT non ha un indirizzo raggiungibile dall'esterno. Il
router crea un *mapping* (IP privato:porta → IP pubblico:porta) solo quando il
dispositivo **invia** qualcosa, e fa entrare pacchetti solo se passano il suo
*filtro*. La RFC 4787 classifica i due comportamenti in modo indipendente:

- **Mapping**: *endpoint-independent* (EIM: stesso IP:porta pubblico verso
  qualsiasi destinazione) oppure *address/port-dependent* (APDM, detto
  "NAT simmetrico": porta pubblica diversa per ogni destinazione).
- **Filtering**: *endpoint-independent* (EIF, "full cone"),
  *address-dependent* (ADF) o *address+port-dependent* (APDF, "port
  restricted", il caso tipico dei router Linux).

Come funziona il hole punching: entrambi scoprono il proprio IP:porta pubblico
chiedendolo a un terzo (qui: il protocollo identify dei nodi DHT, l'equivalente
di STUN). Poi, coordinati da un canale di segnalazione (qui: un circuito relay
v2 + il protocollo DCUtR), si inviano pacchetti **nello stesso momento**.
Ognuno apre il proprio NAT verso l'altro, e i pacchetti si incrociano.

**Matrice di fattibilità** (A ↔ B, entrambi dietro NAT):

| A \ B | EIM + qualsiasi filtro | APDM (simmetrico) |
|---|---|---|
| **EIM + EIF/ADF** | ✅ diretto | ⚠️ possibile: B arriva da una porta imprevista, ma il filtro di A guarda solo l'IP |
| **EIM + APDF** | ✅ diretto | ❌ **impossibile**: B usa verso A una porta che nessuno conosce, e il filtro di A la scarta |
| **APDM (simmetrico)** | vedi sopra | ❌ **impossibile** senza predizione delle porte |

Casi che rendono la connessione diretta **tecnicamente impossibile** senza
relay dei dati:

1. **Entrambi dietro NAT simmetrico** (APDM, molto comune su CGNAT mobile e
   su reti aziendali). La porta pubblica verso il peer non è conoscibile in
   anticipo. Esiste la "predizione delle porte" (birthday attack su centinaia
   di porte, come fa Tailscale sui "hard NAT"): è probabilistica, rumorosa e
   non implementata in libp2p.
2. **Simmetrico da un lato, port-restricted (APDF) dall'altro.** Verificato
   in laboratorio (scenario `cone-sym`).
3. **UDP bloccato e TCP in ingresso filtrato in modo stretto** (reti
   aziendali, alcuni Wi-Fi pubblici). Il TCP simultaneous open funziona solo
   con NAT/firewall che lo tollerano.
4. **Solo HTTP(S) in uscita tramite proxy** (reti aziendali restrittive,
   alcuni hotel). Non si fa hole punching attraverso un proxy HTTP: l'unica
   via è un relay, e il progetto lo esclude.
5. **Firewall host** (Windows Defender Firewall, firewall applicativo macOS)
   che blocca il traffico in ingresso dell'app. Rimedio: autorizzare l'app
   (il sistema lo chiede al primo avvio).
6. **Collisioni di conntrack** (NAT Linux): se il pacchetto del peer arriva
   al NAT *prima* che il nostro esca, il NAT crea una voce per il flusso in
   ingresso. Il nostro pacchetto in uscita riceve allora una porta pubblica
   diversa e il hole punching fallisce per circa 30 s. Il protocollo DCUtR
   limita il problema sincronizzando gli invii sull'RTT misurato. Per lo
   stesso motivo il ticket **non** contiene gli indirizzi pubblici osservati
   ma non confermati: comporli "alla cieca" creerebbe proprio queste voci.

Mitigazioni che aumentano la percentuale di successo (non la portano al 100 %):

- **IPv6 nativo** su entrambi i lati: niente NAT, restano solo firewall
  stateful, attraversati dal simultaneous open quasi sempre.
- **UPnP-IGD / NAT-PMP / PCP**: il router apre una porta su richiesta. È
  implementato UPnP (`libp2p-upnp`). Non funziona dietro CGNAT: il router di
  casa non ha un IP pubblico, e l'app lo segnala come "doppio NAT".
- **Port forwarding manuale** sul router di uno dei due: rende quel lato
  raggiungibile direttamente. Verificato in laboratorio (scenario
  `sym-forward`: mittente dietro NAT simmetrico, ricevente con porta
  inoltrata → diretto).

### 2.2 Peer discovery distribuita

Il problema: il ricevente è dietro NAT e ha un IP che cambia. Come fa il
mittente a sapere *dove* sia adesso?

La soluzione adottata separa l'**identità** dalla **posizione**:

- **Identità** = chiave Ed25519 generata sul dispositivo; il `PeerId` ne è
  l'hash. Nessun account. Noise autentica la chiave a ogni connessione: chi
  conosce il PeerId giusto non può essere ingannato da un impostore (può al
  massimo non raggiungere il peer).
- **Posizione** = indirizzi relay correnti, pubblicati nella **DHT Kademlia
  pubblica di IPFS** come *provider record* sotto una chiave derivata dal
  PeerId (`rendezvous.rs`). Un dispositivo dietro NAT è un client DHT: non
  sta nelle tabelle di routing di nessuno, quindi una normale ricerca
  FIND_NODE(PeerId) non lo troverebbe. Il provider record risolve questo.
- **Ticket** (`lso1…`): PeerId più indirizzi attuali, da passare fuori banda
  (chat, QR). Gli indirizzi sono solo suggerimenti: se sono scaduti, il
  mittente cerca comunque il PeerId nella DHT.
- **mDNS** per la stessa LAN: funziona anche senza Internet.

Critica onesta della DHT pubblica:

- **Bootstrap**: per entrare nella DHT serve conoscere almeno un nodo. La
  lista predefinita è quella di kubo (`bootstrap.libp2p.io`, gestita da
  Protocol Labs / IPShipyard). È un punto di centralizzazione "morbido":
  serve solo al primo ingresso, si può sostituire (`--bootstrap`), e in
  futuro si possono ricordare i peer già visti. Resta comunque una
  dipendenza.
- **Latenza**: una ricerca nella DHT richiede da alcuni secondi a decine di
  secondi. Col ticket la si evita.
- **Record non autenticati**: chiunque può pubblicare un provider record per
  la tua chiave. Non può impersonarti (Noise), ma può inquinare i risultati
  (DoS). La mitigazione naturale sono i record firmati: vedi la roadmap su
  Mainline DHT/pkarr.
- **Privacy**: i nodi DHT e i relay vedono PeerId, IP e orari di attività.
  Non vedono il contenuto (Noise end-to-end, anche dentro il circuito relay).
- **Buona cittadinanza**: usiamo la rete IPFS come client, con traffico
  minimo (un annuncio ogni 10 minuti mentre si è in ricezione).

Alternative valutate:

| Opzione | Pro | Contro |
|---|---|---|
| **DHT IPFS (scelta)** | migliaia di nodi, relay v2 già attivi sui nodi pubblici, stessa pila di DCUtR | bootstrap PL, lookup lenti, record non firmati |
| BitTorrent Mainline DHT + pkarr (BEP44) | ~10 M nodi, record firmati Ed25519, molto resiliente | nessun relay per la segnalazione, serve un secondo meccanismo; ottimo come **seconda** via di scoperta |
| WebRTC (ICE/STUN/TURN) | NAT traversal più maturo in assoluto, funziona nel browser | serve comunque un server di segnalazione; TURN è un relay dei dati |
| iroh (n0) | hole punching derivato da Tailscale, QUIC moderno | relay (derivati da DERP) e discovery DNS di n0, oppure self-hosted: di default torni ad avere un server, e il relay trasporta anche i dati come ripiego |
| Nostr / Matrix come segnalazione | semplice | server di terzi con account/chiavi, e resta il problema STUN |

### 2.3 IP dinamici

- L'identità non dipende dall'IP (vedi §2.2). I contatti si salvano come
  PeerId.
- Se l'IP del ricevente cambia, cade la connessione verso il relay. Il nodo
  chiede allora una nuova prenotazione e ripubblica l'annuncio DHT
  (`announce_due` in `node.rs`). Mantiene 2 prenotazioni su relay diversi,
  per sopravvivere alla perdita di uno.
- Se l'IP cambia **durante** un trasferimento, la connessione cade: libp2p non
  espone la migrazione delle connessioni QUIC. Per questo il protocollo
  supporta la **ripresa**: il ricevente conserva un file `.lso-part`, al
  tentativo successivo comunica l'offset e il mittente riparte da lì. L'hash
  BLAKE3 finale copre l'intero file, compresa la parte già ricevuta.
  Verificato in laboratorio (scenario `resume`).

### 2.4 Firewall

- In uscita: servono UDP (QUIC) e/o TCP verso porte arbitrarie. Se passano
  solo 80/443 tramite proxy, è impossibile (§2.1 punto 4).
- In ingresso sull'host: Windows e macOS chiedono il permesso al primo
  avvio. Se lo si nega, il hole punching fallisce anche con NAT favorevoli.
- `lso diag` mostra cosa si vede dall'esterno: indirizzi osservati da più
  peer, porte diverse (NAT simmetrico), presenza di UPnP, doppio NAT,
  prenotazioni relay.

## 3. Perché "zero server" in senso assoluto è impossibile

Consideriamo due dispositivi dietro NAT, senza terze parti:

1. Nessuno dei due conosce il proprio IP:porta pubblico: il NAT non lo
   comunica. Serve un osservatore esterno (STUN, identify).
2. Anche conoscendolo, devono scambiarselo e **sincronizzarsi** entro pochi
   secondi, perché i mapping scadono (UDP spesso in 30-120 s). Serve un canale
   già aperto verso entrambi: un rendezvous raggiungibile da tutti e due.
3. Un canale fuori banda (chat, telefono) potrebbe fare 2, ma non 1, e non ha
   la tempistica necessaria.

Quindi il minimo indispensabile è **un nodo pubblico qualsiasi, non tuo**.
Questo progetto usa i nodi della rete IPFS: migliaia, gestiti da migliaia di
soggetti diversi, sostituibili. Il comando `lso infra` permette a chiunque
abbia un IP pubblico di offrire lo stesso servizio (bootstrap + relay di sola
segnalazione) senza mai vedere i dati.

Eccezioni che funzionano senza terzi:

- stessa LAN (mDNS);
- uno dei due raggiungibile direttamente (IP pubblico, IPv6 con firewall
  aperto, port forwarding) e l'altro che ne conosce l'indirizzo (ticket).

## 4. Sicurezza (sintesi)

- Transport: Noise XX (Ed25519) su TCP, TLS 1.3 su QUIC. Autenticazione
  reciproca del PeerId.
- Il ticket contiene il PeerId: se arriva su un canale fidato, niente MITM.
  Il solo PeerId tramite DHT è ugualmente sicuro, perché è la chiave stessa.
- Il ricevente deve accettare ogni offerta (o usare esplicitamente `--yes`).
- Nomi dei file sanificati (niente path traversal), integrità BLAKE3,
  scrittura su `.lso-part` e rinomina solo dopo la verifica.
- Metadati visibili a terzi: relay e nodi DHT vedono chi parla con chi e
  quando, non il contenuto.

## 5. Conclusione progettuale

L'obiettivo, con il vincolo "dati sempre diretti", è raggiungibile per la
maggior parte delle coppie di reti, ma non per tutte. Il prodotto deve quindi:

1. dire chiaramente quando e perché la connessione diretta è impossibile
   (fatto: `ConnectFailure::explain`, `lso diag`);
2. massimizzare i casi riusciti (QUIC + TCP, UPnP, IPv6, mDNS, ripresa);
3. se un giorno si vorrà un ripiego, renderlo esplicito e opt-in, per esempio
   un relay **tuo** (un tuo dispositivo con porta aperta), mai silenzioso.
