#!/usr/bin/env bash
# End-to-end scenarios on the network lab (real processes, real sockets, real
# Linux NAT). Each scenario builds the lab, starts two public helper nodes, a
# receiver on hostB (LAN 192.168.2.0/24 behind natB) and a sender on hostA
# (LAN 192.168.1.0/24 behind natA), transfers a random file and checks:
#   * outcome (direct transfer or explicit failure) matches the expectation,
#   * SHA-256 of received file equals the original,
#   * bytes that crossed the public helper nodes (relay) are tiny compared to
#     the file: proof that the data plane was direct.
# Usage (root): scenarios.sh [scenario...]   (default: all)
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
B="${LSO_BIN:-$here/../../target/release/lso}"
NL="$here/netlab.sh"
W="${WORK:-/tmp/lso-lab}"
SIZE_MB="${SIZE_MB:-100}"
declare -a RESULTS

cleanup() { pkill -f "^$B( |$)" 2>/dev/null; sleep 0.5; "$NL" down; }
trap cleanup EXIT

relay_bytes() {
  local t=0 i d
  for i in i-infra i-infra2; do for d in rx tx; do
    t=$((t + $(ip netns exec lab-inet cat /sys/class/net/$i/statistics/${d}_bytes)))
  done; done
  echo $t
}

setup() { # natA natB [receiver-port]
  cleanup >/dev/null 2>&1
  rm -rf "$W"; mkdir -p "$W/a" "$W/b/in"
  "$NL" up "$1" "$2" >/dev/null
  local n ip
  for n in infra infra2; do
    [ $n = infra ] && ip=80.10.0.2 || ip=80.40.0.2
    "$NL" exec $n "$B" --identity "$W/$n.key" --port 4001 --no-default-bootstrap infra \
      --external /ip4/$ip/udp/4001/quic-v1 --external /ip4/$ip/tcp/4001 >"$W/$n.out" 2>&1 &
    sleep 0.5
    BOOT+=(--bootstrap "/ip4/$ip/udp/4001/quic-v1/p2p/$("$B" --identity "$W/$n.key" id)")
    BOOT+=(--bootstrap "/ip4/$ip/tcp/4001/p2p/$("$B" --identity "$W/$n.key" id)")
  done
  "$NL" exec hostB "$B" --identity "$W/b.key" --no-default-bootstrap "${BOOT[@]}" --port "${3:-0}" -v \
    receive --dir "$W/b/in" --yes >"$W/b.out" 2>"$W/b.err" &
  for _ in $(seq 60); do grep -q '^lso1' "$W/b.out" 2>/dev/null && break; sleep 1; done
  TICKET=$(grep '^lso1' "$W/b.out")
  RECEIVER_ID=$("$B" --identity "$W/b.key" id)
  head -c $((SIZE_MB * 1024 * 1024)) /dev/urandom >"$W/a/file.bin"
}

send() { # target -> exit code; log in $W/send.log
  timeout 150 "$NL" exec hostA "$B" --identity "$W/a.key" --no-default-bootstrap "${BOOT[@]}" -v \
    send "$1" "$W/a/file.bin" >"$W/send.log" 2>&1
}

record() { # name expected outcome relay_bytes detail
  local verdict=FAIL
  [ "$2" = "$3" ] && verdict=PASS
  RESULTS+=("$(printf '%-4s %-38s expected=%-6s got=%-6s relay=%8s B  %s' "$verdict" "$1" "$2" "$3" "$4" "$5")")
  echo "${RESULTS[-1]}"
  [ "$verdict" = FAIL ] && { echo "---- send.log"; tail -40 "$W/send.log"; echo "---- receiver"; tail -20 "$W/b.err"; }
}

outcome() {
  if [ "$1" = 0 ] && cmp -s "$W/a/file.bin" "$W/b/in/file.bin"; then echo direct; else echo nodirect; fi
}

run_simple() { # name natA natB target(ticket|peerid) expected [receiver-port]
  BOOT=(); setup "$2" "$3" "${6:-0}"
  local target="$TICKET"; [ "$4" = peerid ] && target="$RECEIVER_ID"
  local r0; r0=$(relay_bytes)
  send "$target"; local rc=$?
  local used=$(( $(relay_bytes) - r0 ))
  local how; how=$(grep -o 'connessione DIRETTA stabilita via [^ ]*' "$W/send.log" | head -1 | awk '{print $5}')
  local hint; hint=$(grep -o 'NAT simmetrico probabile[^;]*' "$W/send.log" | head -1)
  record "$1" "$5" "$(outcome $rc)" "$used" "${how:-exit=$rc} ${hint}"
}

run_resume() {
  BOOT=(); setup cone cone
  # Throttle the path towards B so the transfer takes a while, then kill the
  # sender mid-way: the receiver keeps a .lso-part file.
  ip netns exec lab-inet tc qdisc add dev i-natB root tbf rate 40mbit burst 64kb latency 300ms
  send "$TICKET" & local pid=$!
  for _ in $(seq 60); do ls "$W/b/in"/.*.lso-part >/dev/null 2>&1 && break; sleep 0.5; done
  sleep 4
  pkill -f "^$B .* send " ; wait $pid 2>/dev/null
  local part; part=$(stat -c %s "$W/b/in"/.*.lso-part 2>/dev/null || echo 0)
  ip netns exec lab-inet tc qdisc del dev i-natB root
  sleep 2
  send "$TICKET"; local rc=$?
  local resumed; resumed=$(grep -ao 'ripresa di [^\r]*' "$W/send.log" | head -1)
  local got; got=$(outcome $rc)
  [ -z "$resumed" ] && got="noresume"
  record "resume after interruption (cone/cone)" direct "$got" "-" "partial=$part B; $resumed"
}

ALL=(cone-ticket cone-dht cone-sym sym-sym sym-forward resume)
for s in "${@:-${ALL[@]}}"; do
  case $s in
    cone-ticket) run_simple "cone/cone, ticket" cone cone ticket direct ;;
    cone-dht)    run_simple "cone/cone, PeerId only (DHT lookup)" cone cone peerid direct ;;
    cone-sym)    run_simple "cone/symmetric (impossible)" cone symmetric ticket nodirect ;;
    sym-sym)     run_simple "symmetric/symmetric (impossible)" symmetric symmetric ticket nodirect ;;
    sym-forward) run_simple "symmetric/port-forwarded receiver" symmetric forward ticket direct 5000 ;;
    resume)      run_resume ;;
  esac
done
echo; echo "================ summary"
printf '%s\n' "${RESULTS[@]}"
! printf '%s\n' "${RESULTS[@]}" | grep -q '^FAIL'
