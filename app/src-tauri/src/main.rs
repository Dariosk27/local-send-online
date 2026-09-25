//! Desktop shell for Local Send Online.
//!
//! The UI (app/ui) is plain HTML/CSS/JS; everything network-related is the
//! same `lso-core` used by the CLI. This file only adapts it to Tauri:
//! commands the UI calls, and events the UI listens to.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lso_core::{
    config::{self, TRANSFER_PROTOCOL},
    identity,
    node::{describe_hint, NatHint, Node, NodeConfig, NodeEvent, Role},
    ticket::{self, Ticket},
    transfer::{self, Progress},
    Multiaddr, PeerId,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

struct App {
    node: Node,
    data_dir: PathBuf,
    settings: Mutex<Settings>,
    contacts: Mutex<Vec<Contact>>,
    pending_offers: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    next_id: AtomicU64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    device_name: String,
    download_dir: PathBuf,
    auto_accept_contacts: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Contact {
    peer_id: String,
    name: String,
    /// Last ticket seen for this peer: its addresses are hints, the PeerId is
    /// what matters (the DHT is asked for fresh addresses anyway).
    ticket: Option<String>,
    last_seen: u64,
}

#[derive(Serialize)]
struct Status {
    peer_id: String,
    ticket: String,
    device_name: String,
    download_dir: String,
    /// "connecting" | "online" | "limited" | "offline"
    state: &'static str,
    reservations: usize,
    observed: Vec<String>,
    dht_peers: usize,
    hints: Vec<String>,
    symmetric_nat: bool,
}

#[derive(Serialize, Clone)]
struct OfferEvent {
    id: u64,
    peer_id: String,
    from: String,
    files: Vec<FileInfo>,
    total: u64,
    known_contact: bool,
}

#[derive(Serialize, Clone)]
struct FileInfo {
    name: String,
    size: u64,
}

/// Everything the activity list needs to draw one transfer.
#[derive(Serialize, Clone)]
struct TransferEvent {
    id: u64,
    direction: &'static str, // "in" | "out"
    /// "searching" | "connecting" | "direct" | "waiting" | "transferring"
    /// | "done" | "failed" | "rejected"
    stage: &'static str,
    peer: String,
    title: String,
    total: u64,
    done: u64,
    file_index: usize,
    file_count: usize,
    /// Plain-language explanation shown to the user.
    message: Option<String>,
    /// Technical details (collapsed in the UI).
    detail: Option<String>,
    via: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn short(p: &str) -> String {
    format!("…{}", &p[p.len().saturating_sub(6)..])
}

impl App {
    fn save_settings(&self) {
        let s = self.settings.lock().unwrap().clone();
        let _ = std::fs::write(
            self.data_dir.join("settings.json"),
            serde_json::to_vec_pretty(&s).unwrap(),
        );
    }

    fn save_contacts(&self) {
        let c = self.contacts.lock().unwrap().clone();
        let _ = std::fs::write(
            self.data_dir.join("contacts.json"),
            serde_json::to_vec_pretty(&c).unwrap(),
        );
    }

    fn remember(&self, peer: &PeerId, name: Option<&str>, ticket: Option<String>) {
        let id = peer.to_base58();
        {
            let mut contacts = self.contacts.lock().unwrap();
            match contacts.iter_mut().find(|c| c.peer_id == id) {
                Some(c) => {
                    if let Some(n) = name {
                        c.name = n.to_string();
                    }
                    if ticket.is_some() {
                        c.ticket = ticket;
                    }
                    c.last_seen = now();
                }
                None => contacts.push(Contact {
                    name: name
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("Dispositivo {}", short(&id))),
                    peer_id: id,
                    ticket,
                    last_seen: now(),
                }),
            }
            contacts.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        }
        self.save_contacts();
    }

    fn is_contact(&self, peer: &PeerId) -> bool {
        let id = peer.to_base58();
        self.contacts
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.peer_id == id)
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
async fn status(app: State<'_, Arc<App>>) -> Result<Status, String> {
    let s = app.node.snapshot().await.map_err(|e| e.to_string())?;
    let settings = app.settings.lock().unwrap().clone();
    let observed = !s.observed_addrs.is_empty() || !s.external_addrs.is_empty();
    let state = if !s.reservations.is_empty()
        || (observed
            && s.external_addrs
                .iter()
                .any(|a| !a.to_string().contains("p2p-circuit")))
    {
        "online"
    } else if observed {
        "limited"
    } else if s.routing_peers > 0 {
        "connecting"
    } else {
        "offline"
    };
    Ok(Status {
        peer_id: app.node.peer_id().to_base58(),
        ticket: s.ticket().encode(),
        device_name: settings.device_name,
        download_dir: settings.download_dir.display().to_string(),
        state,
        reservations: s.reservations.len(),
        observed: s.observed_addrs.iter().map(|a| a.to_string()).collect(),
        dht_peers: s.routing_peers,
        symmetric_nat: s
            .hints
            .iter()
            .any(|h| matches!(h, NatHint::SymmetricNat { .. })),
        hints: s.hints.iter().map(describe_hint).collect(),
    })
}

#[tauri::command]
fn contacts(app: State<'_, Arc<App>>) -> Vec<Contact> {
    app.contacts.lock().unwrap().clone()
}

#[tauri::command]
fn rename_contact(app: State<'_, Arc<App>>, peer_id: String, name: String) {
    if let Some(c) = app
        .contacts
        .lock()
        .unwrap()
        .iter_mut()
        .find(|c| c.peer_id == peer_id)
    {
        c.name = name.trim().to_string();
    }
    app.save_contacts();
}

#[tauri::command]
fn forget_contact(app: State<'_, Arc<App>>, peer_id: String) {
    app.contacts
        .lock()
        .unwrap()
        .retain(|c| c.peer_id != peer_id);
    app.save_contacts();
}

#[tauri::command]
fn set_device_name(app: State<'_, Arc<App>>, name: String) {
    let name = name.trim();
    if !name.is_empty() {
        app.settings.lock().unwrap().device_name = name.chars().take(64).collect();
        app.save_settings();
    }
}

#[tauri::command]
fn set_download_dir(app: State<'_, Arc<App>>, dir: String) {
    app.settings.lock().unwrap().download_dir = PathBuf::from(dir);
    app.save_settings();
}

#[tauri::command]
fn open_download_dir(app: State<'_, Arc<App>>) -> Result<(), String> {
    let dir = app.settings.lock().unwrap().download_dir.clone();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    reveal(&dir)
}

#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    reveal(&PathBuf::from(path))
}

fn reveal(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let r = if path.is_file() {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
    } else {
        std::process::Command::new("open").arg(path).spawn()
    };
    #[cfg(target_os = "windows")]
    let r = if path.is_file() {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else {
        std::process::Command::new("explorer").arg(path).spawn()
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let r = std::process::Command::new("xdg-open")
        .arg(if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        })
        .spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn answer_offer(app: State<'_, Arc<App>>, id: u64, accept: bool) {
    if let Some(tx) = app.pending_offers.lock().unwrap().remove(&id) {
        let _ = tx.send(accept);
    }
}

#[derive(Deserialize)]
struct FileMeta {
    path: String,
}

#[tauri::command]
fn file_sizes(paths: Vec<String>) -> Vec<Option<u64>> {
    paths
        .iter()
        .map(|p| {
            std::fs::metadata(p)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len())
        })
        .collect()
}

/// Sends files. Progress and outcome are reported through `transfer` events;
/// the returned id identifies them.
#[tauri::command]
async fn send(
    handle: AppHandle,
    app: State<'_, Arc<App>>,
    target: String,
    files: Vec<FileMeta>,
) -> Result<u64, String> {
    let target_str = target.trim().to_string();
    let parsed = resolve_target(&app, &target_str)?;
    let paths: Vec<PathBuf> = files.into_iter().map(|f| PathBuf::from(f.path)).collect();
    if paths.is_empty() {
        return Err("Nessun file selezionato".into());
    }
    let mut total = 0;
    for p in &paths {
        let m = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        if !m.is_file() {
            return Err(format!(
                "{} non è un file (le cartelle non sono ancora supportate)",
                p.display()
            ));
        }
        total += m.len();
    }
    let id = app.next_id.fetch_add(1, Ordering::Relaxed);
    let app = app.inner().clone();
    tauri::async_runtime::spawn(run_send(handle, app, id, parsed, target_str, paths, total));
    Ok(id)
}

fn resolve_target(app: &App, target: &str) -> Result<Ticket, String> {
    // A contact's PeerId: reuse its last ticket as address hints.
    if !target.starts_with("lso1") {
        let hint = app
            .contacts
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.peer_id == target)
            .and_then(|c| c.ticket.clone());
        if let Some(t) = hint.and_then(|t| Ticket::decode(&t).ok()) {
            return Ok(t);
        }
    }
    ticket::parse_target(target).map_err(|_| {
        "Codice non valido: incolla il codice completo (inizia con lso1) oppure scegli un contatto".to_string()
    })
}

async fn run_send(
    handle: AppHandle,
    app: Arc<App>,
    id: u64,
    target: Ticket,
    target_str: String,
    paths: Vec<PathBuf>,
    total: u64,
) {
    let peer = target.peer;
    let contact_name = app
        .contacts
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.peer_id == peer.to_base58())
        .map(|c| c.name.clone());
    let title = if paths.len() == 1 {
        paths[0]
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        format!("{} file", paths.len())
    };
    let mut ev = TransferEvent {
        id,
        direction: "out",
        stage: "searching",
        peer: contact_name.unwrap_or_else(|| format!("Dispositivo {}", short(&peer.to_base58()))),
        title,
        total,
        done: 0,
        file_index: 0,
        file_count: paths.len(),
        message: None,
        detail: None,
        via: None,
    };
    let _ = handle.emit("transfer", ev.clone());

    // Our own public address is needed before hole punching can work.
    let _ = app.node.wait_ready(false, Duration::from_secs(20)).await;
    ev.stage = "connecting";
    let _ = handle.emit("transfer", ev.clone());

    let conn = match app.node.connect_direct(target.clone()).await {
        Ok(c) => c,
        Err(failure) => {
            ev.stage = "failed";
            ev.message = Some(friendly_failure(&failure));
            ev.detail = Some(failure.explain());
            let _ = handle.emit("transfer", ev);
            return;
        }
    };
    ev.stage = "direct";
    ev.via = Some(describe_via(&conn.addr, conn.hole_punched));
    let _ = handle.emit("transfer", ev.clone());
    app.remember(
        &peer,
        None,
        target_str.starts_with("lso1").then(|| target_str.clone()),
    );

    ev.stage = "waiting"; // receiver has to accept
    let _ = handle.emit("transfer", ev.clone());

    let from = app.settings.lock().unwrap().device_name.clone();
    let mut control = app.node.control();
    let mut throttle = Throttle::new();
    let mut base = 0u64;
    let sizes: Vec<u64> = paths
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .collect();
    let res = transfer::send_files_named(&mut control, peer, &paths, &from, |p| match p {
        Progress::FileStart { index, offset, .. } => {
            base = sizes[..index].iter().sum();
            ev.stage = "transferring";
            ev.file_index = index;
            ev.done = base + offset;
            let _ = handle.emit("transfer", ev.clone());
        }
        Progress::Bytes { done, .. } => {
            ev.done = base + done;
            if throttle.ready() {
                let _ = handle.emit("transfer", ev.clone());
            }
        }
        Progress::FileVerified { .. } => {}
    })
    .await;
    match res {
        Ok(name) => {
            ev.stage = "done";
            ev.done = total;
            if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
                app.remember(&peer, Some(n.trim()), None);
                ev.peer = n.trim().to_string();
            }
        }
        Err(e) => {
            let msg = format!("{e:#}");
            ev.stage = if msg.contains("rifiutato") {
                "rejected"
            } else {
                "failed"
            };
            if ev.stage == "failed" {
                ev.message = Some(
                    "Il collegamento si è interrotto durante l'invio. Riprova: il trasferimento riprenderà da dove si era fermato."
                        .into(),
                );
            }
            ev.detail = Some(msg);
        }
    }
    let _ = handle.emit("transfer", ev);
}

fn friendly_failure(f: &lso_core::node::ConnectFailure) -> String {
    let mut msg = if f.relayed_connection {
        "Il destinatario è online, ma tra le vostre due reti non è possibile un collegamento diretto \
         (di solito succede con reti mobili 4G/5G o aziendali). Prova da un'altra rete, per esempio \
         il Wi‑Fi di casa su uno dei due dispositivi."
            .to_string()
    } else if !f.found_in_dht && f.dial_errors.is_empty() {
        "Destinatario non trovato. L'app è aperta sull'altro dispositivo? Se è appena stata aperta, \
         aspetta un minuto e riprova."
            .to_string()
    } else {
        "Il destinatario non risponde: potrebbe essere offline, oppure il codice non è più valido. \
         Chiedigli di aprire l'app e di mandarti il codice aggiornato."
            .to_string()
    };
    if !f.have_observed_addr {
        msg.push_str(" Nota: la tua rete sembra bloccare le connessioni necessarie.");
    }
    msg
}

fn describe_via(addr: &Multiaddr, hole_punched: bool) -> String {
    let s = addr.to_string();
    let transport = if s.contains("quic") { "QUIC" } else { "TCP" };
    let lan =
        s.starts_with("/ip4/192.168.") || s.starts_with("/ip4/10.") || s.starts_with("/ip4/172.");
    if lan {
        format!("diretta in rete locale ({transport})")
    } else if hole_punched {
        format!("diretta via Internet, NAT attraversato ({transport})")
    } else {
        format!("diretta via Internet ({transport})")
    }
}

struct Throttle(Instant);

impl Throttle {
    fn new() -> Self {
        Throttle(Instant::now() - Duration::from_secs(1))
    }
    fn ready(&mut self) -> bool {
        if self.0.elapsed() >= Duration::from_millis(120) {
            self.0 = Instant::now();
            true
        } else {
            false
        }
    }
}

// ------------------------------------------------------------- receiving

async fn receive_loop(handle: AppHandle, app: Arc<App>) {
    use futures::StreamExt;
    let mut control = app.node.control();
    let Ok(mut incoming) = control.accept(TRANSFER_PROTOCOL) else {
        return;
    };
    while let Some((peer, stream)) = incoming.next().await {
        let handle = handle.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let Ok((offer, incoming)) = transfer::read_offer(stream).await else {
                return;
            };
            let id = app.next_id.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            app.pending_offers.lock().unwrap().insert(id, tx);
            let total = offer.total_size();
            let _ = handle.emit(
                "offer",
                OfferEvent {
                    id,
                    peer_id: peer.to_base58(),
                    from: offer.from.clone(),
                    files: offer
                        .files
                        .iter()
                        .map(|f| FileInfo {
                            name: f.name.clone(),
                            size: f.size,
                        })
                        .collect(),
                    total,
                    known_contact: app.is_contact(&peer),
                },
            );
            let accepted = matches!(
                tokio::time::timeout(Duration::from_secs(300), rx).await,
                Ok(Ok(true))
            );
            app.pending_offers.lock().unwrap().remove(&id);
            let _ = handle.emit("offer-closed", id);
            let title = if offer.files.len() == 1 {
                offer.files[0].name.clone()
            } else {
                format!("{} file", offer.files.len())
            };
            let mut ev = TransferEvent {
                id,
                direction: "in",
                stage: "transferring",
                peer: offer.from.clone(),
                title,
                total,
                done: 0,
                file_index: 0,
                file_count: offer.files.len(),
                message: None,
                detail: None,
                via: app
                    .node
                    .direct_addr(peer)
                    .await
                    .map(|a| describe_via(&a, false)),
            };
            if !accepted {
                let _ = incoming.reject("rifiutato dal destinatario").await;
                ev.stage = "rejected";
                let _ = handle.emit("transfer", ev);
                return;
            }
            app.remember(&peer, Some(&offer.from), None);
            let dest = app.settings.lock().unwrap().download_dir.clone();
            let _ = handle.emit("transfer", ev.clone());
            let sizes: Vec<u64> = offer.files.iter().map(|f| f.size).collect();
            let mut base = 0;
            let mut throttle = Throttle::new();
            let my_name = app.settings.lock().unwrap().device_name.clone();
            let res = incoming
                .accept_as(peer, &dest, Some(&my_name), |p| match p {
                    Progress::FileStart { index, offset, .. } => {
                        base = sizes[..index].iter().sum();
                        ev.file_index = index;
                        ev.done = base + offset;
                        let _ = handle.emit("transfer", ev.clone());
                    }
                    Progress::Bytes { done, .. } => {
                        ev.done = base + done;
                        if throttle.ready() {
                            let _ = handle.emit("transfer", ev.clone());
                        }
                    }
                    Progress::FileVerified { .. } => {}
                })
                .await;
            match res {
                Ok(paths) => {
                    ev.stage = "done";
                    ev.done = total;
                    ev.detail = paths.first().map(|p| p.display().to_string());
                }
                Err(e) => {
                    ev.stage = "failed";
                    ev.message = Some(
                        "Il collegamento si è interrotto. La parte già ricevuta è conservata: se l'altro dispositivo reinvia, si riparte da lì."
                            .into(),
                    );
                    ev.detail = Some(format!("{e:#}"));
                }
            }
            let _ = handle.emit("transfer", ev);
        });
    }
}

// ------------------------------------------------------------------ setup

fn default_settings() -> Settings {
    let download_dir = directories::UserDirs::new()
        .map(|u| {
            u.download_dir()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| u.home_dir().join("Downloads"))
        })
        .unwrap_or_else(std::env::temp_dir)
        .join("LocalSendOnline");
    Settings {
        device_name: whoami::devicename(),
        download_dir,
        auto_accept_contacts: false,
    }
}

fn load<T: for<'de> Deserialize<'de>>(path: PathBuf) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn bootstrap_list() -> Vec<Multiaddr> {
    // LSO_BOOTSTRAP / LSO_NO_DEFAULT_BOOTSTRAP: for the network lab only.
    let mut list: Vec<Multiaddr> = std::env::var("LSO_BOOTSTRAP")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if std::env::var("LSO_NO_DEFAULT_BOOTSTRAP").is_err() {
        list.extend(config::default_bootstrap());
    }
    list
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|tauri_app| {
            let data_dir = match std::env::var("LSO_DATA_DIR") {
                Ok(d) => PathBuf::from(d),
                Err(_) => tauri_app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            let keypair = identity::load_or_create(&data_dir.join("identity.key"))?;
            let settings = load(data_dir.join("settings.json")).unwrap_or_else(default_settings);
            let contacts: Vec<Contact> = load(data_dir.join("contacts.json")).unwrap_or_default();

            let node = tauri::async_runtime::block_on(Node::start(NodeConfig {
                keypair,
                role: Role::Device,
                bootstrap: bootstrap_list(),
                listen_port: std::env::var("LSO_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0),
                reachable: true,
                enable_mdns: true,
                enable_upnp: true,
                external_addrs: vec![],
            }))?;

            let app = Arc::new(App {
                node,
                data_dir,
                settings: Mutex::new(settings),
                contacts: Mutex::new(contacts),
                pending_offers: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
            });
            app.save_settings();
            tauri_app.manage(app.clone());

            let handle = tauri_app.handle().clone();
            tauri::async_runtime::spawn(receive_loop(handle.clone(), app.clone()));

            // Forward the few network events the UI shows live.
            let mut events = app.node.events();
            tauri::async_runtime::spawn(async move {
                while let Ok(ev) = events.recv().await {
                    let msg = match ev {
                        NodeEvent::ReservationAccepted { .. } => Some("Raggiungibile via Internet"),
                        NodeEvent::ReservationLost { .. } => {
                            Some("Relay perso, ricerca di un altro")
                        }
                        NodeEvent::HolePunch { result: Ok(()), .. } => {
                            Some("Connessione diretta aperta")
                        }
                        NodeEvent::NatHint(_)
                        | NodeEvent::ObservedAddr { .. }
                        | NodeEvent::Announced => Some(""),
                        _ => None,
                    };
                    if let Some(m) = msg {
                        let _ = handle.emit("net", m);
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            contacts,
            rename_contact,
            forget_contact,
            set_device_name,
            set_download_dir,
            open_download_dir,
            reveal_path,
            answer_offer,
            file_sizes,
            send,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Local Send Online");
}
