# MateHub observability stack

ELK (Elasticsearch + Kibana + Filebeat) for logs, Prometheus + Grafana for
metrics. Ships the same five components locally (docker-compose) and in
production (raw K8s manifests).

## Local development

From the repo root:

```sh
# Bring up the matehub infra (Postgres, Scylla, Redis, NATS, MinIO, coturn)
# together with the observability stack in a single compose run:
docker compose \
  -f deploy/docker-compose.dev.yml \
  -f deploy/observability/docker-compose.yaml \
  up -d
```

After ~30 seconds everything is up. UIs:

| Service       | URL                      | Credentials    |
|---------------|--------------------------|----------------|
| Kibana        | http://localhost:5601    | —              |
| Grafana       | http://localhost:3000    | admin / admin  |
| Prometheus    | http://localhost:9090    | —              |
| Elasticsearch | http://localhost:9200    | —              |

### Feeding logs in

Run matehub services with `LOG_FORMAT=json` so the output parses cleanly:

```sh
LOG_FORMAT=json cargo run -p matehub-chat
LOG_FORMAT=json cargo run -p matehub-video
# etc.
```

If the service runs in docker, tag the container with
`--label matehub.service=<name>` (or set in compose) so Filebeat picks it up.

Kibana → **Discover** → index pattern `matehub-*` → see JSON fields
`call_id`, `hub_id`, `request_id` as first-class columns.

### Feeding metrics in

Prometheus scrape config (`prometheus/prometheus.yml`) expects services to
listen on their usual host ports and be reachable from inside docker via
`host.docker.internal:<port>`:

| Service  | Port |
|----------|------|
| general  | 3001 |
| hub      | 3002 |
| chat     | 3003 |
| video    | 4000 |

Reload config without restart after edits:
```sh
curl -X POST http://localhost:9090/-/reload
```

## Production (K8s)

Minimal raw manifests in `k8s/`. No operator dependencies — you can apply
them on a fresh YC managed cluster and everything comes up.

```sh
kubectl apply -f deploy/observability/k8s/namespace.yaml
kubectl apply -f deploy/observability/k8s/elasticsearch.yaml
kubectl apply -f deploy/observability/k8s/kibana.yaml
kubectl apply -f deploy/observability/k8s/filebeat.yaml
kubectl apply -f deploy/observability/k8s/prometheus.yaml
kubectl apply -f deploy/observability/k8s/grafana.yaml
```

### What each does

- **elasticsearch.yaml** — single-node StatefulSet, 100 GB PV on
  `yc-network-ssd`. Swap to a 3-node cluster when log volume approaches
  the 100 GB mark.
- **kibana.yaml** — Deployment + ClusterIP. Add your own Ingress for
  `kibana.matehub.io`, behind auth.
- **filebeat.yaml** — DaemonSet (one per node) reading
  `/var/log/containers/*.log`. Autodiscovers pods labelled
  `matehub_service=<name>`. Requires cluster-level RBAC for kubernetes_sd.
- **prometheus.yaml** — Deployment + RBAC. Uses `kubernetes_sd_configs`
  so new matehub pods are picked up as scrape targets automatically, as
  long as they carry `prometheus.io/scrape: "true"` and
  `prometheus.io/port: "<port>"` annotations.
- **grafana.yaml** — Deployment + Secret. Dashboard JSONs live in the
  `grafana-dashboards` ConfigMap; ship them with:
  ```sh
  kubectl -n observability create configmap grafana-dashboards \
    --from-file=deploy/observability/grafana/dashboards/ \
    -o yaml --dry-run=client | kubectl apply -f -
  ```

### Things you will need to do before going live

1. **Change Grafana admin password.** The manifest ships a `CHANGE_ME_BEFORE_DEPLOY`
   placeholder. Either edit the Secret before apply, or (better) swap to
   external-secrets pulling from YC Lockbox / Vault.
2. **Add Ingress + auth** for Kibana and Grafana. Anything exposing dashboards
   and raw log query UI should live behind mTLS, VPN, or at minimum
   Basic auth + TLS. Hostnames are set in each Deployment's env so you only
   need to point DNS and add an Ingress resource.
3. **ILM (Index Lifecycle Management)** for Elasticsearch — currently
   disabled in Filebeat config. Decide retention policy (30 days hot,
   90 days warm?) and add an ILM policy + rollover alias.
4. **Deployment label `matehub_service`** — every matehub service pod
   manifest must carry this label. Filebeat only ingests pods that have it
   (so Scylla, Redis, NATS, etc. don't flood the log store).
5. **Pod annotations for scraping** — see `prometheus.yaml`. Matehub pods
   need `prometheus.io/scrape: "true"` + `prometheus.io/port: "<port>"`.

### Chaining with matehub service deployments

When we write the matehub service Deployments (hub, chat, video, general
shared-multi-tenant variant — tracked separately), they'll need:

```yaml
metadata:
  labels:
    matehub_service: chat     # picked up by Filebeat
  annotations:
    prometheus.io/scrape: "true"
    prometheus.io/port: "3003"
spec:
  template:
    spec:
      containers:
        - name: chat
          env:
            - name: LOG_FORMAT
              value: json
```
