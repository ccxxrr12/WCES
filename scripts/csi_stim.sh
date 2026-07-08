#!/bin/bash
# csi_stim.sh — Stimulate CSI frame generation on ESP32-C5 nodes
#
# Without promiscuous mode or NDP injection, the C5's CSI callback only fires
# when it receives WiFi frames.  On a quiet channel the beacon interval (100ms)
# dominates, yielding ~10 Hz.  This script sends ICMP pings to each C5 at a
# configurable rate, forcing the AP to transmit unicast frames — each triggers
# one CSI callback at the target C5.
#
# Usage:
#   ./csi_stim.sh                # default: 50 Hz per node
#   ./csi_stim.sh 100            # 100 Hz per node
#   ./csi_stim.sh 0              # stop
#
# Deploy on RZ/G2L: /opt/WCES/scripts/csi_stim.sh
# Run as systemd service or in a tmux/screen session.

set -euo pipefail

# ── Configuration ──────────────────────────────────────────────────────────

# C5 node IPs (match wces.config.toml → [deploy] → node_ips)
C5_IPS=("${C5_IPS[@]:-10.172.111.101 10.172.111.102 10.172.111.103}")

# Stimulation rate: pings per second per node
RATE_HZ="${1:-50}"

# ── Sanity checks ─────────────────────────────────────────────────────────

if [[ "$RATE_HZ" == "0" ]]; then
    echo "csi_stim: stopping (rate=0)"
    killall -q csi_stim.sh 2>/dev/null || true
    exit 0
fi

if ! command -v ping &> /dev/null; then
    echo "csi_stim: ERROR — ping not found" >&2
    exit 1
fi

INTERVAL_SEC=$(awk "BEGIN { printf \"%.6f\", 1.0 / ($RATE_HZ * ${#C5_IPS[@]}) }")

echo "csi_stim: targeting ${#C5_IPS[@]} nodes at ${RATE_HZ} Hz each"
echo "csi_stim: aggregate rate = $(( RATE_HZ * ${#C5_IPS[@]} )) pings/s, interval = ${INTERVAL_SEC}s"
echo "csi_stim: press Ctrl+C to stop"

# ── Main loop ─────────────────────────────────────────────────────────────

trap 'echo "csi_stim: stopped"; exit 0' INT TERM

while true; do
    for ip in "${C5_IPS[@]}"; do
        ping -c1 -W1 "$ip" > /dev/null 2>&1 &
    done
    sleep "$INTERVAL_SEC"
done
