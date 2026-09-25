//! `DirectOnly<B>`: wraps a behaviour so that its protocols are only ever
//! negotiated on *direct* connections.
//!
//! This is the enforcement point of the "data plane stays direct" rule.
//! A relayed connection (`/p2p-circuit`) still exists for the control plane
//! (it carries the DCUtR hole-punch signalling), but on that connection the
//! inner behaviour gets a `dummy` handler: no protocol is advertised or
//! accepted, so file bytes physically cannot travel through a third-party
//! relay. The inner behaviour is also never told that the relayed connection
//! exists, so it cannot pick it when opening an outbound stream.

use std::{
    collections::HashSet,
    task::{Context, Poll},
};

use either::Either;
use libp2p::{
    core::{transport::PortUse, Endpoint, Multiaddr},
    multiaddr::Protocol,
    swarm::{
        behaviour::ConnectionEstablished, dummy, ConnectionClosed, ConnectionDenied, ConnectionId,
        FromSwarm, NetworkBehaviour, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    PeerId,
};

pub struct DirectOnly<B> {
    inner: B,
    relayed: HashSet<ConnectionId>,
}

impl<B> DirectOnly<B> {
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            relayed: HashSet::new(),
        }
    }

    pub fn inner(&self) -> &B {
        &self.inner
    }
}

pub fn is_relayed(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| matches!(p, Protocol::P2pCircuit))
}

impl<B: NetworkBehaviour> NetworkBehaviour for DirectOnly<B> {
    type ConnectionHandler = Either<THandler<B>, dummy::ConnectionHandler>;
    type ToSwarm = B::ToSwarm;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        if is_relayed(local_addr) || is_relayed(remote_addr) {
            return Ok(());
        }
        self.inner
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        if is_relayed(local_addr) || is_relayed(remote_addr) {
            self.relayed.insert(connection_id);
            return Ok(Either::Right(dummy::ConnectionHandler));
        }
        self.inner
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr)
            .map(Either::Left)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        self.inner.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        if is_relayed(addr) {
            self.relayed.insert(connection_id);
            return Ok(Either::Right(dummy::ConnectionHandler));
        }
        self.inner
            .handle_established_outbound_connection(
                connection_id,
                peer,
                addr,
                role_override,
                port_use,
            )
            .map(Either::Left)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished { connection_id, .. })
                if self.relayed.contains(&connection_id) => {}
            FromSwarm::ConnectionClosed(ConnectionClosed { connection_id, .. })
                if self.relayed.contains(&connection_id) =>
            {
                self.relayed.remove(&connection_id);
            }
            other => self.inner.on_swarm_event(other),
        }
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {
            Either::Left(ev) => self
                .inner
                .on_connection_handler_event(peer_id, connection_id, ev),
            Either::Right(never) => match never {},
        }
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        self.inner.poll(cx).map(|ev| ev.map_in(Either::Left))
    }
}
