use anyhow::{Context, Result};
use prost::bytes::{Buf, BufMut};
use std::convert::Infallible;
use std::{sync::Arc, time::Duration};
use tokio::sync::{RwLock, RwLockReadGuard};
use tonic::body::Body;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::server::NamedService;
use tonic::service::LayerExt as _;
use tonic::transport::{ClientTlsConfig, Endpoint};
use tonic_web::GrpcWebService;
use tower::Service;
use tower_http::cors::Cors;
use tower_http::cors::CorsLayer;

pub mod reflection;

#[derive(Debug, Clone)]
pub struct GrpcConnection {
    endpoint: tonic::transport::Endpoint,
    channel: Arc<RwLock<tonic::transport::Channel>>,
}

impl GrpcConnection {
    pub async fn new(addr: String, request_timeout: Option<Duration>) -> Result<Self> {
        let use_tls: bool = addr.as_str().starts_with("https://");
        let endpoint = if let Some(timeout) = request_timeout {
            Endpoint::try_from(addr.clone())?.timeout(timeout)
        } else {
            Endpoint::try_from(addr.clone())?
        };
        let endpoint = if use_tls {
            endpoint.tls_config(ClientTlsConfig::new().with_enabled_roots())?
        } else {
            endpoint
        };

        let channel =
            Arc::new(RwLock::new(endpoint.connect().await.context(format!(
                "Failed to connect to gRPC server at {}",
                &addr
            ))?));
        Ok(Self { endpoint, channel })
    }
    pub async fn create(endpoint: Endpoint, channel: tonic::transport::Channel) -> Result<Self> {
        let channel = Arc::new(RwLock::new(channel));
        Ok(Self { endpoint, channel })
    }

    pub async fn reconnect(&self) -> Result<()> {
        let c = self.channel.clone();
        let mut m = c.write().await;
        *m = self.endpoint.connect().await?;
        Ok(())
    }

    pub async fn read_channel(&self) -> RwLockReadGuard<'_, tonic::transport::Channel> {
        self.channel.read().await
    }
}

// Define a custom codec that passes through raw bytes without additional protobuf encoding
#[derive(Debug, Clone)]
pub struct RawBytesCodec;

impl Default for RawBytesCodec {
    fn default() -> Self {
        RawBytesCodec
    }
}

impl Encoder for RawBytesCodec {
    type Item = Vec<u8>;
    type Error = tonic::Status;

    fn encode(&mut self, item: Self::Item, buf: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        // Simply write the raw bytes as-is
        buf.reserve(item.len());
        buf.put_slice(&item);
        Ok(())
    }
}

impl Decoder for RawBytesCodec {
    type Item = Vec<u8>;
    type Error = tonic::Status;

    fn decode(&mut self, buf: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        if !buf.has_remaining() {
            return Ok(None);
        }

        // Just copy the entire buffer into a new Vec<u8>
        let bytes = buf.copy_to_bytes(buf.remaining());
        Ok(Some(bytes.to_vec()))
    }
}

impl Codec for RawBytesCodec {
    type Encode = Vec<u8>;
    type Decode = Vec<u8>;
    type Encoder = RawBytesCodec;
    type Decoder = RawBytesCodec;

    fn encoder(&mut self) -> Self::Encoder {
        RawBytesCodec
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawBytesCodec
    }
}

pub fn enable_grpc_web<S>(service: S) -> tonic::service::Layered<Cors<GrpcWebService<S>>, S>
where
    S: Service<axum::http::Request<Body>, Error = Infallible>
        + NamedService
        + Clone
        + Send
        + Sync
        + 'static,
    S::Response: axum::response::IntoResponse,
    S::Future: Send + 'static,
{
    tower::ServiceBuilder::new()
        .layer(CorsLayer::permissive())
        .layer(tonic_web::GrpcWebLayer::new())
        .into_inner()
        .named_layer(service)
}
