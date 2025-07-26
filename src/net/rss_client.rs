use anyhow::{Context, Result};
use async_trait::async_trait;
use crate::net::reqwest::ReqwestClient;
use rss::Channel;
use std::time::Duration;

pub trait UseReqwest {
    fn reqwest_client(&self) -> &ReqwestClient;
}

#[async_trait]
pub trait RssClient: UseReqwest {
    // async fn create_list(&self, url: &str) -> Result<Channel> {
    //     let channel = self.request_feed(url).await?;
    //     println!("rss channel info: {:?}", channel);
    //     for item in channel.items() {
    //         println!("channel item: {:?}", item)
    //     }
    //     Ok(channel)
    // }

    async fn request_feed(&self, url: &str) -> Result<Channel> {
        let content = self
            .reqwest_client()
            .client()
            .get(url)
            .send()
            .await
            .context(format!("In reqwest url: {url}"))?
            .bytes()
            .await?;
        let channel =
            Channel::read_from(&content[..]).context(format!("In read rss url: {url}"))?;
        Ok(channel)
    }
}

pub trait UseRssClient {
    type CLIENT: RssClient + Send + Sync;
    fn rss_client(&self) -> &Self::CLIENT;
}

pub struct RssClientImpl {
    reqwest_client: ReqwestClient,
}

impl RssClientImpl {
    pub fn new(
        user_agent: impl Into<String>,
        timeout: Duration,
        connect_timeout: Duration,
    ) -> Self {
        // TODO あとでちゃんと作る
        let client = ReqwestClient::new(
            Some(user_agent.into().as_str()),
            Some(timeout),
            Some(connect_timeout),
            Some(2),
        )
        .unwrap();
        Self {
            reqwest_client: client,
        }
    }
}

impl UseReqwest for RssClientImpl {
    fn reqwest_client(&self) -> &ReqwestClient {
        &self.reqwest_client
    }
}

impl RssClient for RssClientImpl {}
