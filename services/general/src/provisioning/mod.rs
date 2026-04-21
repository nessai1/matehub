use anyhow::Result;
use uuid::Uuid;

#[derive(Clone)]
pub struct K8sClient {
    // TODO: hold kube::Client here. Separated out so the rest of the app can
    // run without a cluster (dev mode, tests).
    #[allow(dead_code)]
    client: kube::Client,
}

impl K8sClient {
    /// Connect using the in-cluster ServiceAccount token (when running as a
    /// pod), falling back to $KUBECONFIG / ~/.kube/config for local dev.
    /// Returns None if neither works — provisioning is disabled gracefully
    /// rather than failing startup.
    pub async fn try_connect() -> Option<Self> {
        match kube::Client::try_default().await {
            Ok(client) => {
                tracing::info!("k8s client connected");
                Some(Self { client })
            }
            Err(e) => {
                tracing::warn!(error = %e, "k8s client unavailable — hub provisioning disabled");
                None
            }
        }
    }

    /// Create a namespace + deployments for a new hub.
    ///
    /// Target layout per-hub:
    ///   namespace: hub-<slug>
    ///     Deployment/hub-api      (services/hub binary)
    ///     Deployment/chat-api     (services/chat binary)
    ///     Deployment/video-sfu    (services/video binary)
    ///     Service/ingress routing for <slug>.matehub.io
    ///
    /// For the scaffold this is a stub; the real implementation will live in
    /// `k8s.rs` and use kube::Api + server-side apply of templated manifests.
    pub async fn provision_hub(&self, slug: &str, hub_id: Uuid) -> Result<()> {
        tracing::info!(%slug, %hub_id, "TODO: provision hub namespace + workloads");
        // Simulate work so the status UI has something to observe during dev.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Ok(())
    }
}
