#!/usr/bin/env bash
set -euo pipefill
SVC="${1:-xau_scalper_v4}"
LOG_DIR="${2:-./monitor-logs}"
mkdir -p "$LOG_DIR"
while true; do
  ts=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
  status=$(docker inspect --format='{{json .State.Health.Status}}' "$SVC" 2>/dev/null || echo '"unknown"')
  echo "$ts status=$status" | tee -a "$LOG_DIR/health.log"
  sleep 30
done
