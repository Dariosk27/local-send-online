# Test

Tre livelli, dal più controllato al più reale. Nessun mock: sono sempre i
binari veri su socket veri.

## 1. Unit test

`cargo test`: sanificazione dei nomi dei file (path traversal).

## 2. Laboratorio NAT (`tests/netlab`)

Due LAN private diverse (192.168.1.0/24 e 192.168.2.0/24), ognuna dietro il
proprio router NAT Linux (iptables MASQUERADE + firewall stateful, come un
router domestico), un "internet" con 20 ms di latenza per tratta (RTT 41 ms
tra le LAN) e due nodi pubblici `lso infra` (bootstrap DHT + relay di sola
segnalazione). Il ricevente e il mittente non si vedono mai direttamente se
non attraversando entrambi i NAT.

```
sudo tests/netlab/scenarios.sh            # tutti gli scenari
sudo tests/netlab/scenarios.sh cone-dht   # uno solo
```

Richiede root, `iproute2` e `iptables`. Per la latenza serve `sch_netem`
oppure, in sua assenza, `pip install NetfilterQueue` (usato da `delay.py`).

Risultati (40 MiB per scenario, commit di questo documento):

| Scenario | Atteso | Esito | Byte passati dai nodi pubblici | Percorso dati |
|---|---|---|---|---|
| cone ↔ cone, ticket | diretto | ✅ | ~79 KB | QUIC diretto `80.30.0.2:35800` via hole punching |
| cone ↔ cone, solo PeerId (DHT) | diretto | ✅ | ~76 KB | QUIC diretto via hole punching |
| cone ↔ simmetrico | impossibile | ✅ fallisce con diagnosi, exit 2 | ~121 KB | nessuno (i dati non passano dal relay) |
| simmetrico ↔ simmetrico | impossibile | ✅ fallisce con diagnosi; NAT simmetrico rilevato (porte 25556 ≠ 48935) | ~119 KB | nessuno |
| simmetrico → ricevente con port forwarding | diretto | ✅ | ~74 KB | QUIC diretto sulla porta inoltrata 5000 |
| interruzione a metà + nuovo invio | ripresa | ✅ riparte da 17,6 MiB, BLAKE3 ok | – | QUIC diretto |

I byte conteggiati sono il traffico totale (rx+tx) sulle interfacce dei due
nodi pubblici durante l'invio: identify, Kademlia, prenotazioni relay e
messaggi DCUtR. Per un file di 40 MiB sono circa lo 0,2 %, e non aumentano
con la dimensione del file. Con 200 MB il valore resta nello stesso ordine
di grandezza (misurato: 26 KB).

### Cosa ho imparato dal laboratorio (e corretto)

- **Riuso della porta**: i bootstrap venivano contattati prima che il
  listener TCP fosse pronto. La porta osservata era quindi effimera e il hole
  punching TCP impossibile. Ora il nodo attende il bind prima di fare dial.
- **Latenza zero ≠ Internet**: senza latenza il caso cone ↔ cone con DHT
  falliva circa 1 volta su 5. Il pacchetto del peer arrivava al NAT prima
  che uscisse il nostro, e conntrack assegnava un'altra porta (vedi
  ANALISI §2.1 punto 6). Con 20 ms per tratta: 5 su 5. Su Internet questo
  rischio non sparisce, si riduce: è una delle ragioni per cui la percentuale
  reale di successo non è il 100 %.
- **Connessioni dirette multiple** dopo il hole punching: lo stream poteva
  finire su una connessione in chiusura. Ora viene riaperto (prima dell'invio
  di qualunque byte).

### Limiti del laboratorio

- I NAT sono Linux conntrack. Router di altri produttori (e i CGNAT degli
  operatori) hanno timeout, allocazione delle porte e filtri diversi.
- Nessuna perdita di pacchetti, nessun IPv6, nessun firewall host.
- I nodi pubblici sono `lso infra`, non la rete IPFS reale: il codice è lo
  stesso (Kademlia `/ipfs/kad/1.0.0`, relay v2), ma nella rete reale la
  ricerca DHT è più lenta e la scelta del relay dipende da nodi di terzi.

## 3. Internet reale: Windows ↔ macOS

### In CI (automatico)

Il workflow `.github/workflows/ci.yml` compila ed esegue i test su Windows,
macOS e Linux. Poi fa girare **in parallelo** un ricevente su un runner macOS
e un mittente su un runner Windows. Il mittente conosce solo il PeerId del
ricevente (derivato da un seme di test) e lo trova tramite la **DHT IPFS
pubblica**. Si usano i bootstrap e i relay pubblici IPFS; nessuna
infrastruttura del progetto.

L'esito dipende dai NAT di GitHub/Azure, che non controlliamo (le uscite SNAT
di Azure si comportano spesso come NAT con mapping dipendente dalla
destinazione). Per questo quei job sono `continue-on-error`: un fallimento con
la diagnosi "hole punching non riuscito" è un risultato legittimo. Il job
`diag` stampa il tipo di NAT osservato su ciascun runner.

### A mano, tra due reti vere (procedura consigliata)

Compila su ciascuna macchina (`cargo build --release`) oppure scarica gli
artifact della CI (`lso-windows-latest`, `lso-macos-latest`).

1. Sul Mac (ricevente):
   ```
   ./lso diag              # annota: indirizzi osservati, NAT simmetrico?, UPnP?
   ./lso receive --dir ~/Downloads/lso
   ```
   Al primo avvio macOS può chiedere di consentire le connessioni in entrata:
   consentile. Copia il ticket `lso1…` (oppure il PeerId).
2. Su Windows (mittente):
   ```
   lso.exe diag
   lso.exe send lso1… C:\percorso\file.iso
   ```
   Windows Defender Firewall chiederà l'accesso: autorizza **anche le reti
   pubbliche**. Se lo neghi, i pacchetti del hole punching vengono scartati
   dall'host.
3. Matrice da provare (ogni riga su reti fisicamente diverse):
   | Mac | Windows | Attesa |
   |---|---|---|
   | Wi-Fi di casa A | Wi-Fi di casa B | di solito diretto |
   | Wi-Fi di casa | hotspot 4G/5G del telefono | spesso impossibile (CGNAT simmetrico) |
   | hotspot 4G | hotspot 4G di un altro operatore | quasi sempre impossibile |
   | casa con port forwarding UDP della porta scelta con `--port` | qualsiasi | diretto |
   | rete aziendale con solo proxy HTTP | qualsiasi | impossibile |
4. Per ogni prova annota l'output di `diag` di entrambi e la riga finale
   (`connessione DIRETTA stabilita via …` oppure `IMPOSSIBILE …`). È il dato
   che serve per capire la percentuale reale sul tuo pubblico.

**Stato**: il codice compila per Windows e macOS nella CI, ma non ho potuto
eseguire di persona un trasferimento tra un Mac e un PC reali su due reti
domestiche diverse. Il container di sviluppo non ha rete in uscita oltre a un
proxy HTTPS. Il laboratorio NAT e il job CI Internet sono le prove che ho
potuto eseguire o predisporre.
