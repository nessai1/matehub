#!/bin/bash
set -e

echo "Tearing down MateHub dev infrastructure (including volumes)..."
docker compose -f deploy/docker-compose.dev.yml down -v

echo ""
echo "Bringing it back up..."
docker compose -f deploy/docker-compose.dev.yml up -d

echo ""
echo "Waiting for services..."
sleep 2

echo ""
echo "Infrastructure reset. Volumes are empty, migrations will re-apply on next service start."
echo "  Redis:      localhost:6379"
echo "  NATS:       localhost:4222 (monitoring: http://localhost:8222)"
echo "  MinIO:      localhost:9000 (console: http://localhost:9001, user: matehub / matehub-dev)"
echo "  Postgres:   localhost:5432 (user: matehub / matehub-dev, db: matehub)"
echo "  ScyllaDB:   localhost:9042 (CQL)"
echo "  TURN:       localhost:3478 (user: matehub / matehub-dev)"
