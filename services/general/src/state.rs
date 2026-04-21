use std::sync::Arc;

use sqlx::PgPool;

use crate::config::Config;
use crate::mailer::transport::MailTransport;
use crate::provisioning::K8sClient;

pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub mail_transport: Arc<dyn MailTransport>,
    pub k8s: Option<K8sClient>,
}
