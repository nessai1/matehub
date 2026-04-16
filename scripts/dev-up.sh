#!/bin/bash
set -e

echo "Starting MateHub dev infrastructure..."
docker compose -f deploy/docker-compose.dev.yml up -d

echo ""
echo "Waiting for services..."
sleep 2

echo ""
echo "Infrastructure ready:"
echo "  Redis:      localhost:6379"
echo "  NATS:       localhost:4222 (monitoring: http://localhost:8222)"
echo "  MinIO:      localhost:9000 (console: http://localhost:9001, user: matehub / matehub-dev)"
echo "  Postgres:   localhost:5432 (user: matehub / matehub-dev, db: matehub)"
echo "  ScyllaDB:   localhost:9042 (CQL)"
echo "  TURN:       localhost:3478 (user: matehub / matehub-dev)"
echo ""
echo "Run services:"
echo "  cargo run -p matehub-hub"
echo "  cargo run -p matehub-video"
echo "  cargo run -p matehub-chat"
echo "  npm run dev:frontend"
