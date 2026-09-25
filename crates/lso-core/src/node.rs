//! The libp2p node: one task owns the `Swarm`, everything else talks to it
//! through a command channel and a broadcast of `NodeEvent`s.
//!
//! Control plane handled here:
//!   * joining the Kademlia DHT through bootstrap nodes,
//!   * learning our public address(es) from identify (as seen by others),
//!   * keeping relay reservations so that others can reach us while NAT-ed,
//!   * announcing ourselves in the DHT (provider record) and looking up peers,
//!   * DCUtR hole punching, which upgrades relayed connections to direct ones.
//!
//! The data plane is *not* handled here: it is the `/lso/transfer` stream
//! protocol (see `transfer.rs`), which `DirectOnly` confines to direct
//! connections.

use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use futures::StreamExt;
use libp2p::{
    dcutr, identify,
    identity::Keypair,
    kad, mdns,
    multiaddr::Protocol,
    noise, ping, relay,
    swarm::{
        behaviour::toggle::Toggle,
        dial_opts::{DialOpts, PeerCondition},
        ConnectionId, NetworkBehaviour, SwarmEvent,
    },
    tcp, upnp, yamux, Multiaddr, PeerId, Swarm, SwarmBuilder,
};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    config::{self, AGENT_VERSION},
    direct_only::{is_relayed, DirectOnly},
    rendezvous,
    ticket::{strip_peer, Ticket},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A normal device: DHT client, uses relays, never relays for others.
    Device,
    /// Optional publicly reachable helper (DHT server + relay for signalling).
    /// Anybody with a public IP can run one; nothing depends on a specific one.
    Infrastructure,
}

pub struct NodeConfig {
    pub keypair: Keypair,
    pub role: Role,
    pub bootstrap: Vec<Multiaddr>,
    pub listen_port: u16,
    /// Keep relay reservations and announce ourselves in the DHT, i.e. be
    /// reachable as a receiver.
    pub reachable: bool,
    pub enable_mdns: bool,
    pub enable_upnp: bool,
    /// Addresses to announce as confirmed (infrastructure nodes on a public IP).
    pub external_addrs: Vec<Multiaddr>,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay_client: relay::client::Behaviour,
    relay_server: Toggle<relay::Behaviour>,
    dcutr: dcutr::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    mdns: Toggle<mdns::tokio::Behaviour>,
    upnp: Toggle<upnp::tokio::Behaviour>,
    transfer: DirectOnly<libp2p_stream::Behaviour>,
}

/// Human-relevant things happening in the control plane.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    Listening(Multiaddr),
    /// A peer told us how it sees us (identify). Our "public" address.
    ObservedAddr {
        by: PeerId,
        addr: Multiaddr,
    },
    ReservationAccepted {
        relay: PeerId,
    },
    ReservationLost {
        relay: PeerId,
    },
    Announced,
    AnnounceFailed(String),
    NatHint(NatHint),
    ConnectionOpened {
        peer: PeerId,
        addr: Multiaddr,
        relayed: bool,
    },
    ConnectionClosed {
        peer: PeerId,
        relayed: bool,
    },
    HolePunch {
        peer: PeerId,
        result: Result<(), String>,
    },
    LanPeer {
        peer: PeerId,
        addr: Multiaddr,
    },
    PortMapped(Multiaddr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NatHint {
    /// Observed address equals one of our interface addresses.
    PublicAddress(IpAddr),
    /// Two peers saw the same local socket on different public ports:
    /// endpoint-dependent mapping ("symmetric NAT"). Hole punching will
    /// almost certainly fail unless the other side is directly reachable.
    SymmetricNat {
        ports: Vec<u16>,
    },
    /// UPnP found a gateway but it has no public address (double NAT/CGNAT).
    DoubleNat,
    NoUpnpGateway,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub peer_id: Option<PeerId>,
    pub listen_addrs: Vec<Multiaddr>,
    pub external_addrs: Vec<Multiaddr>,
    pub observed_addrs: Vec<Multiaddr>,
    pub reservations: Vec<PeerId>,
    pub routing_peers: usize,
    pub hints: Vec<NatHint>,
}

impl Snapshot {
    /// Addresses worth putting in a ticket: confirmed external ones (relay
    /// circuits, UPnP mappings, public IPs) plus LAN addresses.
    pub fn ticket(&self) -> Ticket {
        let me = self.peer_id.expect("snapshot from running node");
        let mut addrs: Vec<Multiaddr> = self.external_addrs.clone();
        for a in &self.listen_addrs {
            if is_lan_or_public(a) && !addrs.contains(a) {
                addrs.push(a.clone());
            }
        }
        let addrs = addrs.into_iter().map(|a| strip_peer(a, &me)).collect();
        Ticket { peer: me, addrs }
    }
}

#[derive(Debug, Clone)]
pub struct DirectConnection {
    pub peer: PeerId,
    pub addr: Multiaddr,
    /// True if we first had to go through a relay and upgraded via DCUtR.
    pub hole_punched: bool,
}

/// Why no direct connection could be established. The fields are facts
/// collected while trying, `explain()` turns them into a diagnosis.
#[derive(Debug, Clone, Default)]
pub struct ConnectFailure {
    pub found_in_dht: bool,
    pub relayed_connection: bool,
    pub holepunch_error: Option<String>,
    pub dial_errors: Vec<String>,
    pub local_hints: Vec<NatHint>,
    pub have_observed_addr: bool,
}

impl ConnectFailure {
    pub fn explain(&self) -> String {
        let mut out = String::new();
        if self.relayed_connection {
            out.push_str(
                "Il peer è online e raggiungibile tramite relay (canale di segnalazione OK),\n\
                 ma il hole punching (DCUtR) non è riuscito",
            );
            if let Some(e) = &self.holepunch_error {
                out.push_str(&format!(": {e}"));
            }
            out.push_str(
                ".\nCon questa combinazione di NAT/firewall una connessione diretta NON è possibile.\n\
                 Cause tipiche: NAT simmetrico (mapping dipendente dalla destinazione, frequente\n\
                 su CGNAT mobile/aziendale) su almeno un lato, oppure UDP e TCP in ingresso\n\
                 filtrati da un firewall. Per scelta i dati non passano dal relay.\n\
                 Rimedi: rete diversa per uno dei due (es. hotspot), IPv6 nativo, UPnP/port\n\
                 forwarding sul router di uno dei due.",
            );
        } else if !self.found_in_dht && self.dial_errors.is_empty() {
            out.push_str(
                "Nessun indirizzo trovato per il peer (né nel ticket né nella DHT).\n\
                 Il ricevente è offline, oppure non ha ancora ottenuto una prenotazione relay\n\
                 e un annuncio DHT (attendere ~1 minuto dopo l'avvio di `lso receive`).",
            );
        } else {
            out.push_str(
                "Il peer non ha risposto su nessun indirizzo noto (offline o ticket scaduto).",
            );
        }
        if !self.have_observed_addr {
            out.push_str(
                "\nATTENZIONE: nessun nodo pubblico ci ha comunicato il nostro indirizzo esterno;\n\
                 il bootstrap è probabilmente bloccato da un firewall.",
            );
        }
        for h in &self.local_hints {
            out.push_str(&format!("\nDiagnosi locale: {}", describe_hint(h)));
        }
        if !self.dial_errors.is_empty() {
            out.push_str("\nErrori di connessione:");
            for e in self.dial_errors.iter().take(6) {
                out.push_str(&format!("\n  - {e}"));
            }
        }
        out
    }
}

pub fn describe_hint(h: &NatHint) -> String {
    match h {
        NatHint::PublicAddress(ip) => format!("IP pubblico diretto ({ip}), nessun NAT"),
        NatHint::SymmetricNat { ports } => format!(
            "NAT simmetrico probabile (stesso socket visto su porte diverse: {ports:?}); \
             hole punching quasi impossibile verso peer anch'essi dietro NAT"
        ),
        NatHint::DoubleNat => "doppio NAT / CGNAT (il router UPnP non ha IP pubblico)".into(),
        NatHint::NoUpnpGateway => "nessun router UPnP/IGD disponibile".into(),
    }
}

enum Command {
    Snapshot(oneshot::Sender<Snapshot>),
    Connect {
        target: Ticket,
        reply: oneshot::Sender<Result<DirectConnection, ConnectFailure>>,
    },
    IsDirect(PeerId, oneshot::Sender<Option<Multiaddr>>),
}

#[derive(Clone)]
pub struct Node {
    peer_id: PeerId,
    cmd: mpsc::Sender<Command>,
    events: broadcast::Sender<NodeEvent>,
    control: libp2p_stream::Control,
}

impl Node {
    pub async fn start(cfg: NodeConfig) -> Result<Node> {
        let mut swarm = build_swarm(&cfg)?;
        let peer_id = *swarm.local_peer_id();
        let control = swarm.behaviour().transfer_control();

        for ip in ["0.0.0.0", "::"] {
            let v = if ip == "::" { "ip6" } else { "ip4" };
            let port = cfg.listen_port;
            for a in [
                format!("/{v}/{ip}/udp/{port}/quic-v1"),
                format!("/{v}/{ip}/tcp/{port}"),
            ] {
                if let Err(e) = swarm.listen_on(a.parse()?) {
                    tracing::debug!("cannot listen on {a}: {e}");
                }
            }
        }
        for a in &cfg.external_addrs {
            swarm.add_external_address(a.clone());
        }

        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (ev_tx, _) = broadcast::channel(256);
        let state = State::new(peer_id, &cfg, ev_tx.clone());
        tokio::spawn(run(swarm, state, cmd_rx, cfg.bootstrap));

        Ok(Node {
            peer_id,
            cmd: cmd_tx,
            events: ev_tx,
            control,
        })
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn events(&self) -> broadcast::Receiver<NodeEvent> {
        self.events.subscribe()
    }

    pub fn control(&self) -> libp2p_stream::Control {
        self.control.clone()
    }

    pub async fn snapshot(&self) -> Result<Snapshot> {
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(Command::Snapshot(tx))
            .await
            .context("node stopped")?;
        Ok(rx.await?)
    }

    /// Returns the direct address we are connected to `peer` on, if any.
    pub async fn direct_addr(&self, peer: PeerId) -> Option<Multiaddr> {
        let (tx, rx) = oneshot::channel();
        self.cmd.send(Command::IsDirect(peer, tx)).await.ok()?;
        rx.await.ok().flatten()
    }

    /// Waits until the node knows its own public address (needed for DCUtR)
    /// and, if `need_reservation`, holds at least one relay reservation.
    pub async fn wait_ready(&self, need_reservation: bool, timeout: Duration) -> Result<Snapshot> {
        let deadline = Instant::now() + timeout;
        loop {
            let s = self.snapshot().await?;
            let observed = !s.observed_addrs.is_empty() || !s.external_addrs.is_empty();
            let reserved = !need_reservation || !s.reservations.is_empty();
            if (observed && reserved) || Instant::now() >= deadline {
                return Ok(s);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// Establishes a *direct* connection to the target: dials the ticket
    /// addresses, looks the peer up in the DHT, lets DCUtR upgrade relayed
    /// connections. Succeeds only once a non-relayed connection exists.
    pub async fn connect_direct(&self, target: Ticket) -> Result<DirectConnection, ConnectFailure> {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd
            .send(Command::Connect { target, reply: tx })
            .await
            .is_err()
        {
            return Err(ConnectFailure {
                dial_errors: vec!["node stopped".into()],
                ..Default::default()
            });
        }
        rx.await.unwrap_or_else(|_| Err(ConnectFailure::default()))
    }
}

impl Behaviour {
    fn transfer_control(&self) -> libp2p_stream::Control {
        self.transfer.inner().new_control()
    }
}

fn build_swarm(cfg: &NodeConfig) -> Result<Swarm<Behaviour>> {
    let role = cfg.role;
    let enable_mdns = cfg.enable_mdns;
    let enable_upnp = cfg.enable_upnp;
    let swarm = SwarmBuilder::with_existing_identity(cfg.keypair.clone())
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_dns()?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_client| {
            let peer_id = key.public().to_peer_id();
            let mut kad = kad::Behaviour::with_config(
                peer_id,
                kad::store::MemoryStore::new(peer_id),
                kad::Config::new(kad::PROTOCOL_NAME),
            );
            // Devices behind NAT must not pretend to be DHT servers: the
            // relay reservation would otherwise flip Kademlia to server mode
            // and pollute other nodes' routing tables with unreachable entries.
            kad.set_mode(Some(match role {
                Role::Device => kad::Mode::Client,
                Role::Infrastructure => kad::Mode::Server,
            }));
            let mdns = if enable_mdns {
                mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id).ok()
            } else {
                None
            };
            Ok(Behaviour {
                relay_client,
                relay_server: Toggle::from(
                    (role == Role::Infrastructure)
                        .then(|| relay::Behaviour::new(peer_id, relay::Config::default())),
                ),
                dcutr: dcutr::Behaviour::new(peer_id),
                identify: identify::Behaviour::new(
                    identify::Config::new("/ipfs/0.1.0".into(), key.public())
                        .with_agent_version(AGENT_VERSION.into())
                        .with_push_listen_addr_updates(true),
                ),
                ping: ping::Behaviour::default(),
                kad,
                mdns: Toggle::from(mdns),
                upnp: Toggle::from(enable_upnp.then(upnp::tokio::Behaviour::default)),
                transfer: DirectOnly::new(libp2p_stream::Behaviour::new()),
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();
    Ok(swarm)
}

struct PendingConnect {
    peer: PeerId,
    deadline: Instant,
    reply: oneshot::Sender<Result<DirectConnection, ConnectFailure>>,
    failure: ConnectFailure,
    lookup: Option<kad::QueryId>,
}

struct State {
    me: PeerId,
    reachable: bool,
    events: broadcast::Sender<NodeEvent>,

    listen_addrs: Vec<Multiaddr>,
    external_addrs: Vec<Multiaddr>,
    /// How others see us, per (observer, transport is QUIC).
    observed: HashMap<(PeerId, bool), Multiaddr>,
    hints: Vec<NatHint>,

    /// All live connections: id -> (peer, remote address, relayed?).
    conns: HashMap<ConnectionId, (PeerId, Multiaddr, bool)>,
    /// Relays we could ask for a reservation, with the address we reached them on.
    relay_candidates: HashMap<PeerId, Multiaddr>,
    reservations: HashMap<libp2p::core::transport::ListenerId, (PeerId, bool)>,
    relay_backoff: HashMap<PeerId, Instant>,

    announce_due: bool,
    last_announce: Option<Instant>,
    announce_query: Option<kad::QueryId>,

    pending: Vec<PendingConnect>,
    holepunched: HashSet<PeerId>,
}

impl State {
    fn new(me: PeerId, cfg: &NodeConfig, events: broadcast::Sender<NodeEvent>) -> Self {
        State {
            me,
            reachable: cfg.reachable && cfg.role == Role::Device,
            events,
            listen_addrs: vec![],
            external_addrs: vec![],
            observed: HashMap::new(),
            hints: vec![],
            conns: HashMap::new(),
            relay_candidates: HashMap::new(),
            reservations: HashMap::new(),
            relay_backoff: HashMap::new(),
            announce_due: false,
            last_announce: None,
            announce_query: None,
            pending: vec![],
            holepunched: HashSet::new(),
        }
    }

    fn emit(&self, e: NodeEvent) {
        let _ = self.events.send(e);
    }

    fn hint(&mut self, h: NatHint) {
        // One hint per kind: the symmetric-NAT port list keeps growing as
        // more peers observe us, which is not news for the user.
        let same_kind = |x: &NatHint| std::mem::discriminant(x) == std::mem::discriminant(&h);
        if let Some(existing) = self.hints.iter_mut().find(|x| same_kind(x)) {
            *existing = h;
            return;
        }
        self.hints.push(h.clone());
        self.emit(NodeEvent::NatHint(h));
    }

    fn direct_addr(&self, peer: &PeerId) -> Option<Multiaddr> {
        self.conns
            .values()
            .find(|(p, _, relayed)| p == peer && !relayed)
            .map(|(_, a, _)| a.clone())
    }

    fn confirmed_reservations(&self) -> Vec<PeerId> {
        self.reservations
            .values()
            .filter(|(_, ok)| *ok)
            .map(|(p, _)| *p)
            .collect()
    }

    fn snapshot(&self, swarm: &mut Swarm<Behaviour>) -> Snapshot {
        let routing_peers = swarm
            .behaviour_mut()
            .kad
            .kbuckets()
            .map(|b| b.num_entries())
            .sum();
        Snapshot {
            peer_id: Some(self.me),
            listen_addrs: self.listen_addrs.clone(),
            external_addrs: self.external_addrs.clone(),
            observed_addrs: {
                let mut v: Vec<_> = self.observed.values().cloned().collect();
                v.sort();
                v.dedup();
                v
            },
            reservations: self.confirmed_reservations(),
            routing_peers,
            hints: self.hints.clone(),
        }
    }

    /// Detects endpoint-dependent mapping: the same local listen socket seen
    /// by different peers on different external ports.
    fn check_symmetric_nat(&mut self) {
        let mut ports: HashMap<(IpAddr, bool), HashSet<u16>> = HashMap::new();
        for a in self.observed.values() {
            if let Some((ip, port, quic)) = ip_port(a) {
                ports.entry((ip, quic)).or_default().insert(port);
            }
        }
        for ((_, quic), set) in ports {
            // QUIC always dials from the listen socket, so different observed
            // ports are unambiguous. (TCP may use ephemeral ports.)
            if quic && set.len() > 1 {
                let mut p: Vec<u16> = set.into_iter().collect();
                p.sort();
                self.hint(NatHint::SymmetricNat { ports: p });
            }
        }
    }
}

async fn run(
    mut swarm: Swarm<Behaviour>,
    mut st: State,
    mut cmds: mpsc::Receiver<Command>,
    bootstrap: Vec<Multiaddr>,
) {
    // Wait until our listeners are bound before dialing anybody: outgoing
    // connections then reuse the listen port (TCP port reuse, QUIC shares the
    // socket), so the public address others observe is the one we can be
    // hole-punched on.
    let listeners_ready = tokio::time::sleep(Duration::from_secs(3));
    tokio::pin!(listeners_ready);
    while !has_tcp_and_udp(&st.listen_addrs) {
        tokio::select! {
            ev = swarm.select_next_some() => on_swarm_event(&mut swarm, &mut st, ev),
            _ = &mut listeners_ready => break,
        }
    }
    for addr in &bootstrap {
        if let Some(Protocol::P2p(peer)) = addr.iter().last() {
            swarm.behaviour_mut().kad.add_address(&peer, addr.clone());
        }
        // Dial each address separately so we get both a TCP and a QUIC
        // connection (and thus an observed address for each transport).
        let _ = swarm.dial(DialOpts::unknown_peer_id().address(addr.clone()).build());
    }
    let mut kad_bootstrapped = false;
    let mut tick = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            ev = swarm.select_next_some() => on_swarm_event(&mut swarm, &mut st, ev),
            cmd = cmds.recv() => match cmd {
                None => return,
                Some(cmd) => on_command(&mut swarm, &mut st, cmd),
            },
            _ = tick.tick() => {
                if !kad_bootstrapped && swarm.behaviour_mut().kad.bootstrap().is_ok() {
                    kad_bootstrapped = true;
                }
                maintain(&mut swarm, &mut st);
            }
        }
    }
}

fn on_command(swarm: &mut Swarm<Behaviour>, st: &mut State, cmd: Command) {
    match cmd {
        Command::Snapshot(reply) => {
            let _ = reply.send(st.snapshot(swarm));
        }
        Command::IsDirect(peer, reply) => {
            let _ = reply.send(st.direct_addr(&peer));
        }
        Command::Connect { target, reply } => {
            let peer = target.peer;
            if let Some(addr) = st.direct_addr(&peer) {
                let _ = reply.send(Ok(DirectConnection {
                    peer,
                    addr,
                    hole_punched: false,
                }));
                return;
            }
            for a in &target.addrs {
                swarm.add_peer_address(peer, a.clone());
            }
            if !target.addrs.is_empty() {
                let opts = DialOpts::peer_id(peer)
                    .addresses(target.addrs.clone())
                    .condition(PeerCondition::Always)
                    .build();
                let _ = swarm.dial(opts);
            }
            // Always look the peer up as well: ticket addresses may be stale
            // (new IP, relay gone), the DHT has the latest announcement.
            let lookup = Some(
                swarm
                    .behaviour_mut()
                    .kad
                    .get_providers(rendezvous::key_for(&peer)),
            );
            st.pending.push(PendingConnect {
                peer,
                deadline: Instant::now() + config::DIRECT_CONNECT_TIMEOUT,
                reply,
                failure: ConnectFailure::default(),
                lookup,
            });
        }
    }
}

fn maintain(swarm: &mut Swarm<Behaviour>, st: &mut State) {
    // Expire connection attempts.
    let now = Instant::now();
    let (expired, keep): (Vec<_>, Vec<_>) = st.pending.drain(..).partition(|p| p.deadline <= now);
    st.pending = keep;
    for mut p in expired {
        p.failure.local_hints = st.hints.clone();
        p.failure.have_observed_addr = !st.observed.is_empty() || !st.external_addrs.is_empty();
        if let Some(mut q) = p
            .lookup
            .and_then(|id| swarm.behaviour_mut().kad.query_mut(&id))
        {
            q.finish();
        }
        let _ = p.reply.send(Err(p.failure));
    }

    if !st.reachable {
        return;
    }

    // Keep enough relay reservations.
    let active = st.reservations.len();
    if active < config::TARGET_RESERVATIONS {
        let busy: HashSet<PeerId> = st.reservations.values().map(|(p, _)| *p).collect();
        let candidate = st
            .relay_candidates
            .iter()
            .filter(|(p, _)| !busy.contains(p))
            .filter(|(p, _)| st.relay_backoff.get(p).is_none_or(|t| *t <= now))
            .map(|(p, a)| (*p, a.clone()))
            .next();
        if let Some((relay, addr)) = candidate {
            let circuit = strip_peer(addr, &relay)
                .with(Protocol::P2p(relay))
                .with(Protocol::P2pCircuit);
            match swarm.listen_on(circuit.clone()) {
                Ok(id) => {
                    tracing::info!("requesting relay reservation via {circuit}");
                    st.reservations.insert(id, (relay, false));
                }
                Err(e) => tracing::debug!("listen on {circuit} failed: {e}"),
            }
            st.relay_backoff
                .insert(relay, now + Duration::from_secs(120));
        }
    }

    // Announce ourselves in the DHT once reachable, periodically, and when
    // our addresses changed.
    let has_reservation = !st.confirmed_reservations().is_empty();
    let periodic = st
        .last_announce
        .is_some_and(|t| t.elapsed() >= config::REPROVIDE_INTERVAL);
    if has_reservation && (st.announce_due || periodic) && st.announce_query.is_none() {
        match swarm
            .behaviour_mut()
            .kad
            .start_providing(rendezvous::key_for(&st.me))
        {
            Ok(q) => {
                st.announce_query = Some(q);
                st.announce_due = false;
                st.last_announce = Some(now);
            }
            Err(e) => tracing::warn!("cannot announce: {e}"),
        }
    }
}

fn on_swarm_event(swarm: &mut Swarm<Behaviour>, st: &mut State, ev: SwarmEvent<BehaviourEvent>) {
    match ev {
        SwarmEvent::NewListenAddr { address, .. } => {
            if !is_relayed(&address) {
                st.listen_addrs.push(address.clone());
                st.emit(NodeEvent::Listening(address));
            }
        }
        SwarmEvent::ExpiredListenAddr { address, .. } => st.listen_addrs.retain(|a| a != &address),
        SwarmEvent::ListenerClosed { listener_id, .. } => {
            if let Some((relay, _)) = st.reservations.remove(&listener_id) {
                st.emit(NodeEvent::ReservationLost { relay });
                st.announce_due = true;
            }
        }
        SwarmEvent::ExternalAddrConfirmed { address } => {
            if !st.external_addrs.contains(&address) {
                st.external_addrs.push(address);
                st.announce_due = true;
            }
        }
        SwarmEvent::ExternalAddrExpired { address } => {
            st.external_addrs.retain(|a| a != &address);
            st.announce_due = true;
        }
        SwarmEvent::ConnectionEstablished {
            peer_id,
            connection_id,
            endpoint,
            ..
        } => {
            let relayed = endpoint.is_relayed();
            let addr = endpoint.get_remote_address().clone();
            st.conns
                .insert(connection_id, (peer_id, addr.clone(), relayed));
            st.emit(NodeEvent::ConnectionOpened {
                peer: peer_id,
                addr: addr.clone(),
                relayed,
            });
            if !relayed {
                let hole_punched = st.holepunched.contains(&peer_id);
                let mut i = 0;
                while i < st.pending.len() {
                    if st.pending[i].peer == peer_id {
                        let p = st.pending.remove(i);
                        if let Some(mut q) = p
                            .lookup
                            .and_then(|id| swarm.behaviour_mut().kad.query_mut(&id))
                        {
                            q.finish();
                        }
                        let _ = p.reply.send(Ok(DirectConnection {
                            peer: peer_id,
                            addr: addr.clone(),
                            hole_punched,
                        }));
                    } else {
                        i += 1;
                    }
                }
            } else {
                for p in st.pending.iter_mut().filter(|p| p.peer == peer_id) {
                    p.failure.relayed_connection = true;
                }
            }
        }
        SwarmEvent::ConnectionClosed {
            peer_id,
            connection_id,
            endpoint,
            ..
        } => {
            st.conns.remove(&connection_id);
            st.emit(NodeEvent::ConnectionClosed {
                peer: peer_id,
                relayed: endpoint.is_relayed(),
            });
        }
        SwarmEvent::OutgoingConnectionError {
            peer_id: Some(peer),
            error,
            ..
        } => {
            for p in st.pending.iter_mut().filter(|p| p.peer == peer) {
                p.failure.dial_errors.push(error.to_string());
            }
        }
        SwarmEvent::Behaviour(ev) => on_behaviour_event(swarm, st, ev),
        _ => {}
    }
}

fn on_behaviour_event(swarm: &mut Swarm<Behaviour>, st: &mut State, ev: BehaviourEvent) {
    match ev {
        BehaviourEvent::Identify(identify::Event::Received {
            peer_id,
            info,
            connection_id,
        }) => {
            let observed = info.observed_addr.clone();
            if let Some((_, _, quic)) = ip_port(&observed).filter(|_| !is_relayed(&observed)) {
                let changed =
                    st.observed.insert((peer_id, quic), observed.clone()) != Some(observed.clone());
                if changed {
                    st.emit(NodeEvent::ObservedAddr {
                        by: peer_id,
                        addr: observed.clone(),
                    });
                    if let Some((ip, _, _)) = ip_port(&observed) {
                        if st
                            .listen_addrs
                            .iter()
                            .any(|a| ip_port(a).is_some_and(|(l, _, _)| l == ip))
                        {
                            st.hint(NatHint::PublicAddress(ip));
                        }
                    }
                    st.check_symmetric_nat();
                }
            }
            let supports_hop = info
                .protocols.contains(&relay::HOP_PROTOCOL_NAME);
            if supports_hop && peer_id != st.me {
                if let Some((_, addr, false)) = st.conns.get(&connection_id) {
                    if is_public(addr) {
                        st.relay_candidates
                            .entry(peer_id)
                            .or_insert_with(|| addr.clone());
                    }
                }
            }
            let is_dht_server = info.protocols.contains(&kad::PROTOCOL_NAME);
            for a in info.listen_addrs.into_iter().filter(|_| is_dht_server) {
                if is_public(&a) {
                    swarm.behaviour_mut().kad.add_address(&peer_id, a);
                }
            }
        }
        BehaviourEvent::RelayClient(relay::client::Event::ReservationReqAccepted {
            relay_peer_id,
            renewal,
            ..
        }) => {
            for (p, ok) in st.reservations.values_mut() {
                if *p == relay_peer_id {
                    *ok = true;
                }
            }
            if !renewal {
                st.emit(NodeEvent::ReservationAccepted {
                    relay: relay_peer_id,
                });
                st.announce_due = true;
            }
        }
        BehaviourEvent::Dcutr(dcutr::Event {
            remote_peer_id,
            result,
        }) => {
            match &result {
                Ok(_) => {
                    st.holepunched.insert(remote_peer_id);
                }
                Err(e) => {
                    for p in st.pending.iter_mut().filter(|p| p.peer == remote_peer_id) {
                        p.failure.holepunch_error = Some(e.to_string());
                    }
                }
            }
            st.emit(NodeEvent::HolePunch {
                peer: remote_peer_id,
                result: result.map(|_| ()).map_err(|e| e.to_string()),
            });
        }
        BehaviourEvent::Kad(kad::Event::OutboundQueryProgressed { id, result, .. }) => match result
        {
            kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                providers,
                ..
            })) => {
                for p in st.pending.iter_mut().filter(|p| p.lookup == Some(id)) {
                    if providers.contains(&p.peer) {
                        p.failure.found_in_dht = true;
                        // Dial while the query is alive: Kademlia hands the
                        // provider's announced (relay) addresses to the dial.
                        let _ = swarm.dial(
                            DialOpts::peer_id(p.peer)
                                .condition(PeerCondition::DisconnectedAndNotDialing)
                                .build(),
                        );
                    }
                }
            }
            kad::QueryResult::StartProviding(res) if Some(id) == st.announce_query => {
                st.announce_query = None;
                match res {
                    Ok(_) => st.emit(NodeEvent::Announced),
                    Err(e) => {
                        st.emit(NodeEvent::AnnounceFailed(e.to_string()));
                        st.announce_due = true;
                    }
                }
            }
            _ => {}
        },
        BehaviourEvent::Mdns(mdns::Event::Discovered(list)) => {
            for (peer, addr) in list {
                swarm.add_peer_address(peer, addr.clone());
                st.emit(NodeEvent::LanPeer { peer, addr });
            }
        }
        BehaviourEvent::Upnp(ev) => match ev {
            upnp::Event::NewExternalAddr { external_addr, .. } => {
                st.emit(NodeEvent::PortMapped(external_addr))
            }
            upnp::Event::NonRoutableGateway => st.hint(NatHint::DoubleNat),
            upnp::Event::GatewayNotFound => st.hint(NatHint::NoUpnpGateway),
            _ => {}
        },
        _ => {}
    }
}

fn has_tcp_and_udp(addrs: &[Multiaddr]) -> bool {
    let tcp = addrs
        .iter()
        .any(|a| ip_port(a).is_some_and(|(_, _, quic)| !quic));
    let udp = addrs
        .iter()
        .any(|a| ip_port(a).is_some_and(|(_, _, quic)| quic));
    tcp && udp
}

fn ip_port(a: &Multiaddr) -> Option<(IpAddr, u16, bool)> {
    let mut it = a.iter();
    let ip = match it.next()? {
        Protocol::Ip4(ip) => IpAddr::V4(ip),
        Protocol::Ip6(ip) => IpAddr::V6(ip),
        _ => return None,
    };
    match it.next()? {
        Protocol::Tcp(p) => Some((ip, p, false)),
        Protocol::Udp(p) => Some((ip, p, true)),
        _ => None,
    }
}

fn first_ip(a: &Multiaddr) -> Option<IpAddr> {
    ip_port(a)
        .map(|(ip, _, _)| ip)
        .or_else(|| match a.iter().next()? {
            Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
            Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
            _ => None,
        })
}

/// Globally routable IP (or a DNS name).
pub fn is_public(a: &Multiaddr) -> bool {
    if matches!(
        a.iter().next(),
        Some(Protocol::Dns(_) | Protocol::Dns4(_) | Protocol::Dns6(_) | Protocol::Dnsaddr(_))
    ) {
        return true;
    }
    match first_ip(a) {
        Some(IpAddr::V4(ip)) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_documentation()
                || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64))
        }
        Some(IpAddr::V6(ip)) => (ip.segments()[0] & 0xe000) == 0x2000,
        None => false,
    }
}

fn is_lan_or_public(a: &Multiaddr) -> bool {
    match first_ip(a) {
        Some(ip) => !ip.is_loopback() && !ip.is_unspecified(),
        None => false,
    }
}
