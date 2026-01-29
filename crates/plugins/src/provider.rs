/// Provider plugin: auth methods, model catalog, credential refresh.
use async_trait::async_trait;

#[async_trait]
pub trait ProviderPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn auth_methods(&self) -> Vec<AuthMethod>;
    async fn list_models(&self) -> anyhow::Result<Vec<String>>;
}

#[derive(Debug, Clone)]
pub enum AuthMethod {
    ApiKey,
    OAuth,
    DeviceCode,
}
