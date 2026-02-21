#!/usr/bin/env bash
set -euo pipefail

IMAGE="pastebom-dev"
PORT="${DEV_PORT:-8080}"

usage() {
    echo "Usage: ./dev.sh <start|stop|status> [port]"
    exit 1
}

cmd_start() {
    if [ -n "${1:-}" ]; then
        PORT="$1"
    fi

    echo "=== Building Docker image ==="
    docker build -t "$IMAGE" .

    echo "=== Starting container on port $PORT ==="
    docker rm -f "$IMAGE" 2>/dev/null || true
    docker run -d --name "$IMAGE" -p "$PORT:8080" -e "BASE_URL=http://localhost:$PORT" "$IMAGE"
    echo "Running at http://localhost:$PORT"
}

cmd_stop() {
    docker rm -f "$IMAGE" 2>/dev/null && echo "Stopped" || echo "Not running"
}

cmd_status() {
    if docker inspect -f '{{.State.Status}}' "$IMAGE" 2>/dev/null; then
        PORT=$(docker inspect -f '{{(index (index .NetworkSettings.Ports "8080/tcp") 0).HostPort}}' "$IMAGE" 2>/dev/null)
        echo "http://localhost:$PORT"
    else
        echo "Not running"
    fi
}

case "${1:-}" in
    start)  cmd_start "${2:-}" ;;
    stop)   cmd_stop ;;
    status) cmd_status ;;
    *)      usage ;;
esac
