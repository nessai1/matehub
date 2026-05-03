use std::sync::Arc;

use sqlx::PgPool;

use crate::config::Config;
use crate::mailer::transport::MailTransport;
use crate::provisioning::K8sClient;

pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    // Held on AppState so handlers can grab it from `State` once the
    // landing/contact endpoints stop being TODO stubs. Not currently read.
    #[allow(dead_code)]
    pub mail_transport: Arc<dyn MailTransport>,
    pub k8s: Option<K8sClient>,
}
