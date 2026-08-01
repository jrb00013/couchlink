#!/usr/bin/env bash
# Allow couchlink + Headscale mesh ports through Linux firewall (ufw / firewalld / nft / iptables).
# Invoked by scripts/unblock-firewall.sh on linux (and WSL's Linux side).
#
# Ports:
#   TCP 8443  — signaling
#   TCP/UDP 3478 — couchlink TURN
#   UDP 34790 — Headscale embedded DERP STUN
#   TCP 8080  — Headscale control plane (friend hs= bootstrap)
#   UDP 41641 — Tailscale/Headscale WireGuard
set -euo pipefail

echo "==> Linux unblock-firewall"

run_root() {
  if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    "$@"
    return $?
  fi
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    sudo -n "$@"
    return $?
  fi
  echo "    skip (need passwordless sudo/root): $*" >&2
  return 1
}

opened=0

if command -v ufw >/dev/null 2>&1; then
  status="$(ufw status 2>/dev/null || true)"
  if echo "$status" | grep -qi 'Status: active'; then
    echo "==> ufw active — allowing couchlink/Headscale ports"
    run_root ufw allow 8443/tcp comment 'couchlink-signaling' || true
    run_root ufw allow 3478/tcp comment 'couchlink-turn' || true
    run_root ufw allow 3478/udp comment 'couchlink-turn' || true
    run_root ufw allow 34790/udp comment 'headscale-stun' || true
    run_root ufw allow 8080/tcp comment 'headscale-control' || true
    run_root ufw allow 41641/udp comment 'headscale-wireguard' || true
    opened=1
  else
    echo "==> ufw present but inactive — skipping ufw rules"
  fi
fi

if [[ "$opened" != "1" ]] && command -v firewall-cmd >/dev/null 2>&1; then
  if firewall-cmd --state 2>/dev/null | grep -qi running; then
    echo "==> firewalld — allowing couchlink/Headscale ports"
    run_root firewall-cmd --permanent --add-port=8443/tcp || true
    run_root firewall-cmd --permanent --add-port=3478/tcp || true
    run_root firewall-cmd --permanent --add-port=3478/udp || true
    run_root firewall-cmd --permanent --add-port=34790/udp || true
    run_root firewall-cmd --permanent --add-port=8080/tcp || true
    run_root firewall-cmd --permanent --add-port=41641/udp || true
    run_root firewall-cmd --reload || true
    opened=1
  fi
fi

if [[ "$opened" != "1" ]] && command -v nft >/dev/null 2>&1; then
  echo "==> nftables best-effort (inet filter input accept)"
  # Idempotent-ish: add rules; ignore duplicates
  for spec in \
    "tcp dport 8443 accept" \
    "tcp dport 3478 accept" \
    "udp dport 3478 accept" \
    "udp dport 34790 accept" \
    "tcp dport 8080 accept" \
    "udp dport 41641 accept"; do
    run_root nft add rule inet filter input $spec 2>/dev/null \
      || run_root nft insert rule inet filter input $spec 2>/dev/null \
      || true
  done
  opened=1
fi

if [[ "$opened" != "1" ]] && command -v iptables >/dev/null 2>&1; then
  echo "==> iptables best-effort"
  run_root iptables -C INPUT -p tcp --dport 8443 -j ACCEPT 2>/dev/null \
    || run_root iptables -I INPUT -p tcp --dport 8443 -j ACCEPT || true
  run_root iptables -C INPUT -p tcp --dport 3478 -j ACCEPT 2>/dev/null \
    || run_root iptables -I INPUT -p tcp --dport 3478 -j ACCEPT || true
  run_root iptables -C INPUT -p udp --dport 3478 -j ACCEPT 2>/dev/null \
    || run_root iptables -I INPUT -p udp --dport 3478 -j ACCEPT || true
  run_root iptables -C INPUT -p udp --dport 34790 -j ACCEPT 2>/dev/null \
    || run_root iptables -I INPUT -p udp --dport 34790 -j ACCEPT || true
  run_root iptables -C INPUT -p tcp --dport 8080 -j ACCEPT 2>/dev/null \
    || run_root iptables -I INPUT -p tcp --dport 8080 -j ACCEPT || true
  run_root iptables -C INPUT -p udp --dport 41641 -j ACCEPT 2>/dev/null \
    || run_root iptables -I INPUT -p udp --dport 41641 -j ACCEPT || true
  opened=1
fi

if [[ "$opened" != "1" ]]; then
  echo "WARN: no ufw/firewalld/nft/iptables manager applied rules"
  echo "    ensure inbound TCP 8080/8443/3478 and UDP 3478/34790/41641 are allowed"
  exit 0
fi

echo "OK — Linux firewall unblock attempted"
exit 0
