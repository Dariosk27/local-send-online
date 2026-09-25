//! `lso`: command line front-end for Local Send Online.

use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use lso_core::{
    config::{self, TRANSFER_PROTOCOL},
    identity,
    node::{describe_hint, Node, NodeConfig, NodeEvent, Role},
    rendezvous, ticket,
    transfer::{self, Progress},
    Multiaddr,
};

#[derive(Parser)]
#[command(
    name = "lso",
    version,
    about = "Trasferimento file diretto peer-to-peer via Internet"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    #[command(flatten)]
    net: NetArgs,
}

#[derive(Args, Clone)]
struct NetArgs {
    /// File con la chiave del dispositivo (default: cartella di configurazione utente).
    #[arg(long, global = true)]
    identity: Option<PathBuf>,
    /// Identità deterministica derivata da una stringa: SOLO per test automatici.
    #[arg(long, global = true, hide = true)]
    identity_seed: Option<String>,
    /// Porta locale TCP/UDP (0 = casuale).
    #[arg(long, global = true, default_value_t = 0)]
    port: u16,
    /// Nodo di bootstrap aggiuntivo (multiaddr con /p2p/<id>). Ripetibile.
    #[arg(long, global = true)]
    bootstrap: Vec<Multiaddr>,
    /// Non usare i bootstrap pubblici IPFS (solo quelli passati con --bootstrap).
    #[arg(long, global = true)]
    no_default_bootstrap: bool,
    #[arg(long, global = true)]
    no_mdns: bool,
    #[arg(long, global = true)]
    no_upnp: bool,
    /// Mostra anche gli eventi di rete di basso livello.
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Mostra l'identità (PeerId) di questo dispositivo.
    Id,
    /// Resta in ascolto e riceve file. Stampa un ticket da dare al mittente.
    Receive {
        /// Cartella di destinazione.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Accetta automaticamente ogni offerta (per test/automazione).
        #[arg(long)]
        yes: bool,
        /// Termina dopo il primo trasferimento completato.
        #[arg(long)]
        once: bool,
    },
    /// Invia uno o più file a un ticket o a un PeerId.
    Send {
        /// Ticket (lso1...) oppure PeerId del destinatario.
        target: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Condivide file con un codice breve (es. 7F3K-9H2P) valido 5 minuti.
    Share {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Riceve i file condivisi con `share`, dato il codice breve.
    Get {
        code: String,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
    },
    /// Diagnostica NAT: indirizzo pubblico osservato, NAT simmetrico, UPnP, relay.
    Diag {
        #[arg(long, default_value_t = 25)]
        seconds: u64,
    },
    /// Nodo di infrastruttura opzionale (IP pubblico): DHT server + relay di
    /// sola segnalazione. Non trasporta mai dati dei file.
    Infra {
        /// Indirizzo pubblico da annunciare (es. /ip4/1.2.3.4/udp/4001/quic-v1).
        #[arg(long)]
        external: Vec<Multiaddr>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,lso_core=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match cli.cmd {
        Cmd::Id => {
            let key = load_identity(&cli.net)?;
            println!("{}", key.public().to_peer_id());
            Ok(())
        }
        Cmd::Receive { dir, yes, once } => receive(&cli.net, dir, yes, once).await,
        Cmd::Send { target, files } => send(&cli.net, &target, &files).await,
        Cmd::Share { files } => share(&cli.net, &files).await,
        Cmd::Get { code, dir, yes } => get(&cli.net, &code, dir, yes).await,
        Cmd::Diag { seconds } => diag(&cli.net, seconds).await,
        Cmd::Infra { external } => infra(&cli.net, external).await,
    }
}

fn load_identity(net: &NetArgs) -> Result<lso_core::Keypair> {
    match &net.identity_seed {
        Some(seed) => Ok(identity::from_test_seed(seed)),
        None => identity::load_or_create(&identity_path(net)?),
    }
}

fn identity_path(net: &NetArgs) -> Result<PathBuf> {
    if let Some(p) = &net.identity {
        return Ok(p.clone());
    }
    let dirs = directories::ProjectDirs::from("", "", "local-send-online")
        .context("cannot determine config directory")?;
    Ok(dirs.config_dir().join("identity.key"))
}

async fn start(
    net: &NetArgs,
    role: Role,
    reachable: bool,
    external: Vec<Multiaddr>,
) -> Result<Node> {
    let keypair = load_identity(net)?;
    let mut bootstrap = net.bootstrap.clone();
    if !net.no_default_bootstrap {
        bootstrap.extend(config::default_bootstrap());
    }
    if bootstrap.is_empty() && role == Role::Device {
        eprintln!("attenzione: nessun nodo di bootstrap, funzionerà solo in LAN (mDNS)");
    }
    let node = Node::start(NodeConfig {
        keypair,
        role,
        bootstrap,
        listen_port: net.port,
        reachable,
        enable_mdns: !net.no_mdns && role == Role::Device,
        enable_upnp: !net.no_upnp && role == Role::Device,
        external_addrs: external,
    })
    .await?;
    spawn_event_printer(&node, net.verbose);
    Ok(node)
}

fn spawn_event_printer(node: &Node, verbose: bool) {
    let mut rx = node.events();
    tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let line = match &ev {
                NodeEvent::ObservedAddr { by, addr } => Some(format!(
                    "indirizzo pubblico osservato da {}: {addr}",
                    short(by)
                )),
                NodeEvent::ReservationAccepted { relay } => Some(format!(
                    "prenotazione relay (solo segnalazione) attiva su {}",
                    short(relay)
                )),
                NodeEvent::ReservationLost { relay } => {
                    Some(format!("prenotazione relay persa: {}", short(relay)))
                }
                NodeEvent::Announced => Some("annunciato nella DHT".into()),
                NodeEvent::AnnounceFailed(e) => Some(format!("annuncio DHT fallito: {e}")),
                NodeEvent::NatHint(h) => Some(format!("diagnosi NAT: {}", describe_hint(h))),
                NodeEvent::HolePunch {
                    peer,
                    result: Ok(()),
                } => Some(format!("hole punching riuscito con {}", short(peer))),
                NodeEvent::HolePunch {
                    peer,
                    result: Err(e),
                } => Some(format!("hole punching fallito con {}: {e}", short(peer))),
                NodeEvent::PortMapped(a) => Some(format!("porta aperta sul router via UPnP: {a}")),
                NodeEvent::LanPeer { peer, addr } if verbose => {
                    Some(format!("peer in LAN {} su {addr}", short(peer)))
                }
                NodeEvent::ConnectionOpened {
                    peer,
                    addr,
                    relayed,
                } if verbose => Some(format!(
                    "connessione {} con {} via {addr}",
                    if *relayed { "RELAY" } else { "diretta" },
                    short(peer)
                )),
                NodeEvent::Listening(a) if verbose => Some(format!("in ascolto su {a}")),
                _ => None,
            };
            if let Some(l) = line {
                eprintln!("· {l}");
            }
        }
    });
}

fn short(p: &lso_core::PeerId) -> String {
    let s = p.to_base58();
    format!("…{}", &s[s.len() - 8..])
}

async fn receive(net: &NetArgs, dir: Option<PathBuf>, yes: bool, once: bool) -> Result<()> {
    let dest = match dir {
        Some(d) => d,
        None => directories::UserDirs::new()
            .and_then(|u| u.download_dir().map(|d| d.join("LocalSendOnline")))
            .context("specificare --dir")?,
    };
    let node = start(net, Role::Device, true, vec![]).await?;
    let mut control = node.control();
    let mut incoming = control.accept(TRANSFER_PROTOCOL).expect("registered once");

    eprintln!("PeerId: {}", node.peer_id());
    eprintln!("in attesa di indirizzo pubblico e prenotazione relay...");
    let snap = node.wait_ready(true, Duration::from_secs(45)).await?;
    if snap.reservations.is_empty() {
        eprintln!(
            "ATTENZIONE: nessuna prenotazione relay ottenuta. Si è raggiungibili solo se\n\
             si ha un IP pubblico/IPv6/UPnP o dalla stessa LAN."
        );
    }
    println!(
        "\nTicket (da dare al mittente):\n{}\n",
        snap.ticket().encode()
    );
    eprintln!("In alternativa il mittente può usare solo il PeerId (ricerca nella DHT).");
    eprintln!("Salvataggio in {}", dest.display());

    use futures_lite::StreamExt as _;
    while let Some((peer, stream)) = incoming.next().await {
        let (offer, incoming_transfer) = match transfer::read_offer(stream).await {
            Ok(x) => x,
            Err(e) => {
                eprintln!("offerta non valida da {peer}: {e}");
                continue;
            }
        };
        let via = node
            .direct_addr(peer)
            .await
            .map(|a| a.to_string())
            .unwrap_or_else(|| "?".into());
        eprintln!(
            "\nOfferta da {peer} ({}) via connessione DIRETTA {via}:",
            offer.from
        );
        for f in &offer.files {
            eprintln!("  {}  ({})", f.name, human(f.size));
        }
        let ok = yes || ask("Accettare? [s/N] ").await;
        if !ok {
            incoming_transfer.reject("rifiutato dall'utente").await.ok();
            continue;
        }
        let started = Instant::now();
        let total = offer.total_size();
        let mut bars = Bars::default();
        match incoming_transfer
            .accept_as(peer, &dest, Some(&hostname()), |p| bars.update(p))
            .await
        {
            Ok(paths) => {
                bars.finish();
                let dt = started.elapsed().as_secs_f64();
                eprintln!(
                    "ricevuti {} file, {} in {:.1}s ({}/s), BLAKE3 verificato:",
                    paths.len(),
                    human(total),
                    dt,
                    human((total as f64 / dt) as u64)
                );
                for p in paths {
                    println!("{}", p.display());
                }
                if once {
                    return Ok(());
                }
            }
            Err(e) => {
                bars.finish();
                eprintln!("trasferimento fallito: {e:#}");
            }
        }
    }
    Ok(())
}

async fn send(net: &NetArgs, target: &str, files: &[PathBuf]) -> Result<()> {
    let target = ticket::parse_target(target)?;
    for f in files {
        if !f.is_file() {
            bail!("{} non è un file", f.display());
        }
    }
    let node = start(net, Role::Device, false, vec![]).await?;
    eprintln!("rilevamento del proprio indirizzo pubblico...");
    let snap = node.wait_ready(false, Duration::from_secs(20)).await?;
    if snap.observed_addrs.is_empty() && snap.external_addrs.is_empty() {
        eprintln!(
            "attenzione: nessun indirizzo pubblico noto, il hole punching difficilmente funzionerà"
        );
    }
    eprintln!("connessione diretta a {} ...", target.peer);
    let conn = match node.connect_direct(target.clone()).await {
        Ok(c) => c,
        Err(failure) => {
            eprintln!(
                "\nIMPOSSIBILE stabilire una connessione diretta.\n{}",
                failure.explain()
            );
            std::process::exit(2);
        }
    };
    eprintln!(
        "connessione DIRETTA stabilita via {}{}",
        conn.addr,
        if conn.hole_punched {
            " (hole punching)"
        } else {
            ""
        }
    );

    let total: u64 = files
        .iter()
        .map(|f| f.metadata().map(|m| m.len()).unwrap_or(0))
        .sum();
    let from = hostname();
    let started = Instant::now();
    let mut bars = Bars::default();
    let mut control = node.control();
    let res =
        transfer::send_files(&mut control, target.peer, files, &from, |p| bars.update(p)).await;
    bars.finish();
    res?;
    let dt = started.elapsed().as_secs_f64();
    eprintln!(
        "inviato e verificato dal ricevente: {} in {:.1}s ({}/s)",
        human(total),
        dt,
        human((total as f64 / dt) as u64)
    );
    Ok(())
}

fn download_dir(dir: Option<PathBuf>) -> Result<PathBuf> {
    match dir {
        Some(d) => Ok(d),
        None => directories::UserDirs::new()
            .and_then(|u| u.download_dir().map(|d| d.join("LocalSendOnline")))
            .context("specificare --dir"),
    }
}

async fn share(net: &NetArgs, files: &[PathBuf]) -> Result<()> {
    for f in files {
        if !f.is_file() {
            bail!("{} non è un file", f.display());
        }
    }
    let node = start(net, Role::Device, true, vec![]).await?;
    let mut control = node.control();
    let mut pulls = control
        .accept(config::PULL_PROTOCOL)
        .expect("registered once");
    eprintln!("connessione alla rete...");
    let snap = node.wait_ready(true, Duration::from_secs(45)).await?;
    if snap.reservations.is_empty() {
        eprintln!(
            "ATTENZIONE: nessun relay ottenuto, il codice potrebbe non funzionare fuori dalla LAN"
        );
    }
    let code = rendezvous::generate_code();
    let key = rendezvous::key_for_code(&code);
    node.provide(key.clone()).await;
    let expires = Instant::now() + config::CODE_TTL;
    println!("{code}");
    eprintln!("Codice: {code}  (valido 5 minuti). Chi riceve: lso get {code}");

    use futures_lite::StreamExt as _;
    loop {
        let next = tokio::time::timeout_at(expires.into(), pulls.next()).await;
        let Ok(Some((peer, stream))) = next else {
            node.stop_providing(key).await;
            bail!("codice scaduto senza che nessuno lo usasse");
        };
        let Ok((req, responder)) = transfer::read_pull(stream).await else {
            continue;
        };
        let valid = rendezvous::normalize_code(&req.code).as_deref() == Some(code.as_str());
        responder.answer(valid).await.ok();
        if !valid {
            continue;
        }
        node.stop_providing(key).await;
        eprintln!("{} ({peer}) ha inserito il codice, invio...", req.name);
        let started = Instant::now();
        let total: u64 = files
            .iter()
            .map(|f| f.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        let mut bars = Bars::default();
        let res =
            transfer::send_files(&mut control, peer, files, &hostname(), |p| bars.update(p)).await;
        bars.finish();
        res?;
        let dt = started.elapsed().as_secs_f64();
        eprintln!(
            "inviato e verificato: {} in {:.1}s ({}/s)",
            human(total),
            dt,
            human((total as f64 / dt) as u64)
        );
        return Ok(());
    }
}

async fn get(net: &NetArgs, code: &str, dir: Option<PathBuf>, yes: bool) -> Result<()> {
    let code = rendezvous::normalize_code(code).context("codice non valido (formato XXXX-XXXX)")?;
    let dest = download_dir(dir)?;
    let node = start(net, Role::Device, true, vec![]).await?;
    let mut control = node.control();
    let mut incoming = control.accept(TRANSFER_PROTOCOL).expect("registered once");
    eprintln!("connessione alla rete...");
    node.wait_ready(false, Duration::from_secs(20)).await?;
    eprintln!("ricerca del dispositivo con il codice {code}...");
    let peer = node
        .find_provider(rendezvous::key_for_code(&code), Duration::from_secs(90))
        .await
        .context("nessun dispositivo trovato con questo codice (scaduto o sbagliato?)")?;
    let conn = match node
        .connect_direct(ticket::Ticket {
            peer,
            addrs: vec![],
        })
        .await
    {
        Ok(c) => c,
        Err(f) => {
            eprintln!(
                "\nIMPOSSIBILE stabilire una connessione diretta.\n{}",
                f.explain()
            );
            std::process::exit(2);
        }
    };
    eprintln!("connessione DIRETTA via {}", conn.addr);
    transfer::request_pull(&mut control, peer, &code, &hostname()).await?;

    use futures_lite::StreamExt as _;
    let (from, stream) = tokio::time::timeout(Duration::from_secs(60), incoming.next())
        .await
        .context("il mittente non ha avviato l'invio")?
        .context("chiuso")?;
    anyhow::ensure!(from == peer, "offerta da un dispositivo inatteso");
    let (offer, t) = transfer::read_offer(stream).await?;
    eprintln!("{} vuole inviarti:", offer.from);
    for f in &offer.files {
        eprintln!("  {}  ({})", f.name, human(f.size));
    }
    if !(yes || ask("Accettare? [s/N] ").await) {
        t.reject("rifiutato dall'utente").await.ok();
        return Ok(());
    }
    let mut bars = Bars::default();
    let paths = t
        .accept_as(peer, &dest, Some(&hostname()), |p| bars.update(p))
        .await;
    bars.finish();
    for p in paths? {
        println!("{}", p.display());
    }
    eprintln!("ricevuto e verificato");
    Ok(())
}

async fn diag(net: &NetArgs, seconds: u64) -> Result<()> {
    let node = start(net, Role::Device, true, vec![]).await?;
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    let s = node.snapshot().await?;
    println!("PeerId:              {}", node.peer_id());
    println!("peer DHT noti:       {}", s.routing_peers);
    println!(
        "indirizzi locali:    {:?}",
        s.listen_addrs
            .iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
    );
    println!(
        "osservati da altri:  {:?}",
        s.observed_addrs
            .iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
    );
    println!(
        "esterni confermati:  {:?}",
        s.external_addrs
            .iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
    );
    println!("prenotazioni relay:  {}", s.reservations.len());
    for h in &s.hints {
        println!("diagnosi:            {}", describe_hint(h));
    }
    if s.observed_addrs.is_empty() {
        println!("esito: nessun nodo pubblico raggiunto (UDP e TCP in uscita bloccati?) → impossibile senza relay/proxy");
    }
    Ok(())
}

async fn infra(net: &NetArgs, external: Vec<Multiaddr>) -> Result<()> {
    let node = start(net, Role::Infrastructure, false, external).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = node.snapshot().await?;
    println!("nodo infrastruttura {}", node.peer_id());
    for a in s.listen_addrs.iter().chain(s.external_addrs.iter()) {
        println!("  {a}/p2p/{}", node.peer_id());
    }
    std::io::stdout().flush().ok();
    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn ask(prompt: &str) -> bool {
    eprint!("{prompt}");
    tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).ok();
        matches!(
            s.trim().to_lowercase().as_str(),
            "s" | "si" | "sì" | "y" | "yes"
        )
    })
    .await
    .unwrap_or(false)
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "dispositivo".into())
}

fn human(n: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

/// Progress bars driven only by real byte counts reported by the transfer.
#[derive(Default)]
struct Bars {
    bar: Option<ProgressBar>,
}

impl Bars {
    fn update(&mut self, p: Progress) {
        match p {
            Progress::FileStart {
                name, size, offset, ..
            } => {
                self.finish();
                let bar = ProgressBar::new(size);
                bar.set_style(
                    ProgressStyle::with_template(
                        "{msg} [{bar:30}] {bytes}/{total_bytes} {binary_bytes_per_sec} eta {eta}",
                    )
                    .unwrap()
                    .progress_chars("=> "),
                );
                if offset > 0 {
                    eprintln!(
                        "ripresa di {name} da {} (parziale già presente dal tentativo precedente)",
                        human(offset)
                    );
                }
                bar.set_message(name);
                bar.set_position(offset);
                bar.reset_eta();
                self.bar = Some(bar);
            }
            Progress::Bytes { done, .. } => {
                if let Some(b) = &self.bar {
                    b.set_position(done);
                }
            }
            Progress::FileVerified { .. } => self.finish(),
        }
    }

    fn finish(&mut self) {
        if let Some(b) = self.bar.take() {
            b.finish();
        }
    }
}
