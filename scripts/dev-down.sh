#!/bin/bash
echo "Stopping MateHub dev infrastructure..."
docker compose -f deploy/docker-compose.dev.yml down
echo "Done."
