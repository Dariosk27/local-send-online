"""Adds a fixed one-way delay to every packet the "internet" namespace forwards.

The container kernel used for development has no sch_netem, so latency is
added in user space through NFQUEUE. Without latency the lab is unrealistic
for hole punching: both peers' packets reach the opposite NAT within
microseconds and the ordering race (see docs/ANALISI.md §2.1, conntrack
collisions) is decided by scheduling noise instead of by the network.

Usage (inside lab-inet):  delay.py <milliseconds>
Needs: pip install NetfilterQueue, and
       iptables -I FORWARD -j NFQUEUE --queue-num 0
"""
import heapq
import itertools
import select
import sys
import time

from netfilterqueue import NetfilterQueue

delay = float(sys.argv[1]) / 1000.0
pending = []
seq = itertools.count()


def on_packet(pkt):
    pkt.retain()
    heapq.heappush(pending, (time.monotonic() + delay, next(seq), pkt))


nfq = NetfilterQueue()
nfq.bind(0, on_packet, max_len=65536)
fd = nfq.get_fd()
try:
    while True:
        timeout = max(0.0, pending[0][0] - time.monotonic()) if pending else 1.0
        ready, _, _ = select.select([fd], [], [], timeout)
        if ready:
            nfq.run(block=False)
        now = time.monotonic()
        while pending and pending[0][0] <= now:
            heapq.heappop(pending)[2].accept()
except KeyboardInterrupt:
    pass
finally:
    nfq.unbind()
