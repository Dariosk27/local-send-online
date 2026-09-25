#!/bin/bash
# Installs the Local Send Online backup relay on a Linux server with a public
# IP (e.g. an Oracle Cloud "Always Free" VM with Ubuntu). One command:
#
#   curl -fsSL https://raw.githubusercontent.com/Dariosk27/local-send-online/claude/p2p-file-transfer-app-gj44yg/scripts/install-relay.sh | bash
#
# The relay only carries data when two devices cannot connect directly, and
# it cannot read it (end-to-end encryption between the devices).
set -euo pipefail

case "$(uname -m)" in
  x86_64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) echo "Architettura non supportata: $(uname -m)"; exit 1 ;;
esac
PORT=4001
BIN_URL="https://raw.githubusercontent.com/Dariosk27/local-send-online/test-kit/lso-linux-$ARCH"

echo "Scarico il relay ($ARCH)..."
sudo curl -fsSL -o /usr/local/bin/lso "$BIN_URL"
sudo chmod +x /usr/local/bin/lso

IP=$(curl -fsS https://api.ipify.org)
sudo mkdir -p /var/lib/lso

echo "Apro la porta $PORT (TCP e UDP) nel firewall del server..."
# Oracle's Ubuntu images drop everything except SSH by default.
sudo iptables -C INPUT -p tcp --dport $PORT -j ACCEPT 2>/dev/null || sudo iptables -I INPUT -p tcp --dport $PORT -j ACCEPT
sudo iptables -C INPUT -p udp --dport $PORT -j ACCEPT 2>/dev/null || sudo iptables -I INPUT -p udp --dport $PORT -j ACCEPT
if command -v netfilter-persistent >/dev/null; then sudo netfilter-persistent save >/dev/null 2>&1 || true; fi

sudo tee /etc/systemd/system/lso-relay.service >/dev/null <<UNIT
[Unit]
Description=Local Send Online relay
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/lso --identity /var/lib/lso/identity.key --port $PORT --no-default-bootstrap --no-mdns --no-upnp relay --external /ip4/$IP/udp/$PORT/quic-v1 --external /ip4/$IP/tcp/$PORT
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable --now lso-relay >/dev/null
sleep 2
PEER=$(sudo /usr/local/bin/lso --identity /var/lib/lso/identity.key id)

echo
echo "Relay attivo. Manda questa riga a chi prepara l'app:"
echo
echo "   /ip4/$IP/udp/$PORT/quic-v1/p2p/$PEER"
echo
echo "Ricorda: nella console di Oracle apri anche la porta $PORT TCP e UDP"
echo "(Networking > Virtual Cloud Networks > Security List > Add Ingress Rules)."
