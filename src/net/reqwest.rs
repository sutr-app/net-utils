use std::time::Duration;

use anyhow::Result;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};

#[derive(Debug, Clone)]
pub struct ReqwestClient {
    client: ClientWithMiddleware,
}
impl ReqwestClient {
    pub fn new(
        user_agent: Option<&str>,
        timeout: Option<Duration>,
        connect_timeout: Option<Duration>,
        retry: Option<u32>,
    ) -> Result<Self> {
        let mut client_builder = reqwest::Client::builder();
        if let Some(ua) = user_agent {
            client_builder = client_builder.user_agent(ua);
        }
        let client = client_builder
            .timeout(timeout.unwrap_or_else(|| Duration::new(30, 0)))
            .connect_timeout(connect_timeout.unwrap_or_else(|| Duration::new(30, 0)))
            .build()?;
        let client = ClientBuilder::new(client);
        let retry_policy = retry.map(|r| ExponentialBackoff::builder().build_with_max_retries(r));
        let client = if let Some(retry_policy) = retry_policy {
            client.with(RetryTransientMiddleware::new_with_policy(retry_policy))
        } else {
            client
        };
        let client = client
            .with(TracingMiddleware::<SpanBackendWithUrl>::new())
            .build();
        Ok(Self { client })
    }
    pub fn client(&self) -> &ClientWithMiddleware {
        &self.client
    }
}
