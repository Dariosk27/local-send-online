#!/usr/bin/env bash
# Network lab: two devices on *different* private networks, each behind its
# own NAT router, plus one public node, all connected through a simulated
# "internet". Built with Linux network namespaces + iptables, so the NAT
# behaviour is the real Linux conntrack NAT (the same one most home routers
# run), not a mock.
#
#            hostA 192.168.1.2                      hostB 192.168.2.2
#                 |                                      |
#            natA 192.168.1.1                       natB 192.168.2.1
#            natA 80.20.0.2 (WAN)                   natB 80.30.0.2 (WAN)
#                  \                                   /
#                   +------------ inet ---------------+
#                                  |
#                      infra 80.10.0.2    infra2 80.40.0.2
#            (public helper nodes: DHT bootstrap + relay, signalling only)
#
# NAT types:
#   cone      MASQUERADE: endpoint-independent mapping, port preserved,
#             address+port-dependent filtering (typical home router).
#   symmetric MASQUERADE --random-fully: a new random public port per
#             destination (endpoint-dependent mapping; like many CGNATs).
#   forward   cone + manual port forwarding of TCP/UDP 5000 to the host
#             (what UPnP/NAT-PMP or a user-configured router rule gives you).
#
# LAB_IPV6=1 also gives every site global IPv6 (2001:db8::/32, routed, no NAT)
# with a stateful firewall on each router that drops unsolicited inbound
# traffic, like home routers and mobile networks with IPv6 do.
#
# Usage (root):  netlab.sh up <natA-type> <natB-type> | down | exec <ns> <cmd...>
set -euo pipefail

NS=(inet infra infra2 natA natB hostA hostB)
DELAY_MS="${LAB_DELAY_MS:-20}"
PIDFILE=/run/lso-netlab-delay.pid

up() {
  local ta="${1:-cone}" tb="${2:-cone}"
  down 2>/dev/null || true
  for n in "${NS[@]}"; do ip netns add "lab-$n"; ip -n "lab-$n" link set lo up; done

  link() { # ns1 if1 ns2 if2
    ip link add "$2" netns "lab-$1" type veth peer name "$4" netns "lab-$3"
    ip -n "lab-$1" link set "$2" up
    ip -n "lab-$3" link set "$4" up
  }
  link inet i-infra infra wan
  link inet i-infra2 infra2 wan
  link inet i-natA natA wan
  link inet i-natB natB wan
  link natA lan hostA eth0
  link natB lan hostB eth0

  ip -n lab-inet addr add 80.10.0.1/24 dev i-infra
  ip -n lab-inet addr add 80.40.0.1/24 dev i-infra2
  ip -n lab-inet addr add 80.20.0.1/24 dev i-natA
  ip -n lab-inet addr add 80.30.0.1/24 dev i-natB
  ip netns exec lab-inet sysctl -qw net.ipv4.ip_forward=1
  # One-way latency across the "internet": netem if the kernel has it,
  # otherwise a user-space NFQUEUE delayer (delay.py).
  if [ "$DELAY_MS" != 0 ]; then
    if tc -n lab-inet qdisc add dev i-infra root netem delay "${DELAY_MS}ms" 2>/dev/null; then
      for i in i-infra2 i-natA i-natB; do tc -n lab-inet qdisc add dev "$i" root netem delay "${DELAY_MS}ms"; done
    elif python3 -c 'import netfilterqueue' 2>/dev/null; then
      ip netns exec lab-inet iptables -I FORWARD -j NFQUEUE --queue-num 0 --queue-bypass
      ip netns exec lab-inet python3 "$(dirname "$0")/delay.py" "$DELAY_MS" >/dev/null 2>&1 &
      echo $! > "$PIDFILE"
      # With a mean delay D per hop (inet is one hop) RTT between sites = 2*D.
    else
      echo "warning: neither netem nor NetfilterQueue available: zero latency lab" >&2
    fi
  fi

  ip -n lab-infra addr add 80.10.0.2/24 dev wan
  ip -n lab-infra route add default via 80.10.0.1
  ip -n lab-infra2 addr add 80.40.0.2/24 dev wan
  ip -n lab-infra2 route add default via 80.40.0.1

  local x
  for x in A B; do
    local wan net typ
    if [ "$x" = A ]; then wan=80.20.0.2; net=1; typ="$ta"; else wan=80.30.0.2; net=2; typ="$tb"; fi
    ip -n "lab-nat$x" addr add "$wan/24" dev wan
    ip -n "lab-nat$x" route add default via "${wan%.2}.1"
    ip -n "lab-nat$x" addr add "192.168.$net.1/24" dev lan
    ip netns exec "lab-nat$x" sysctl -qw net.ipv4.ip_forward=1
    if [ "$typ" = symmetric ]; then
      ip netns exec "lab-nat$x" iptables -t nat -A POSTROUTING -o wan -j MASQUERADE --random-fully
    else
      ip netns exec "lab-nat$x" iptables -t nat -A POSTROUTING -o wan -j MASQUERADE
    fi
    if [ "$typ" = forward ]; then
      for proto in tcp udp; do
        ip netns exec "lab-nat$x" iptables -t nat -A PREROUTING -i wan -p $proto --dport 5000 \
          -j DNAT --to-destination "192.168.$net.2:5000"
        ip netns exec "lab-nat$x" iptables -A FORWARD -i wan -p $proto -d "192.168.$net.2" --dport 5000 -j ACCEPT
      done
    fi
    # Stateful firewall like a home router: nothing unsolicited gets in.
    ip netns exec "lab-nat$x" iptables -A FORWARD -i wan -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    ip netns exec "lab-nat$x" iptables -A FORWARD -i wan -j DROP
    ip -n "lab-host$x" addr add "192.168.$net.2/24" dev eth0
    ip -n "lab-host$x" route add default via "192.168.$net.1"
  done
  [ "${LAB_IPV6:-0}" = 1 ] && up_ipv6
  echo "lab up: natA=$ta natB=$tb ipv6=${LAB_IPV6:-0}"
}

up_ipv6() {
  local n
  for n in inet natA natB; do ip netns exec "lab-$n" sysctl -qw net.ipv6.conf.all.forwarding=1; done
  for n in "${NS[@]}"; do ip netns exec "lab-$n" sysctl -qw net.ipv6.conf.all.accept_dad=0 net.ipv6.conf.default.accept_dad=0; done
  ip -n lab-inet addr add 2001:db8:10::1/64 dev i-infra nodad
  ip -n lab-inet addr add 2001:db8:40::1/64 dev i-infra2 nodad
  ip -n lab-inet addr add 2001:db8:20::1/64 dev i-natA nodad
  ip -n lab-inet addr add 2001:db8:30::1/64 dev i-natB nodad
  ip -n lab-infra addr add 2001:db8:10::2/64 dev wan nodad
  ip -n lab-infra -6 route add default via 2001:db8:10::1
  ip -n lab-infra2 addr add 2001:db8:40::2/64 dev wan nodad
  ip -n lab-infra2 -6 route add default via 2001:db8:40::1
  local x w l
  for x in A B; do
    if [ $x = A ]; then w=20; l=a; else w=30; l=b; fi
    ip -n "lab-nat$x" addr add "2001:db8:$w::2/64" dev wan nodad
    ip -n "lab-nat$x" -6 route add default via "2001:db8:$w::1"
    ip -n "lab-nat$x" addr add "2001:db8:$l::1/64" dev lan nodad
    ip -n lab-inet -6 route add "2001:db8:$l::/64" via "2001:db8:$w::2"
    ip -n "lab-host$x" addr add "2001:db8:$l::2/64" dev eth0 nodad
    ip -n "lab-host$x" -6 route add default via "2001:db8:$l::1"
    ip netns exec "lab-nat$x" ip6tables -A FORWARD -i wan -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    ip netns exec "lab-nat$x" ip6tables -A FORWARD -i wan -j DROP
  done
}

down() {
  [ -f "$PIDFILE" ] && kill "$(cat "$PIDFILE")" 2>/dev/null; rm -f "$PIDFILE"
  for n in "${NS[@]}"; do ip netns del "lab-$n" 2>/dev/null || true; done
}

case "${1:-}" in
  up) shift; up "$@" ;;
  down) down ;;
  exec) shift; n="$1"; shift; exec ip netns exec "lab-$n" "$@" ;;
  *) echo "usage: $0 up <nat-type-A> <nat-type-B> (cone|symmetric|forward) | down | exec <ns> <cmd...>"; exit 1 ;;
esac
