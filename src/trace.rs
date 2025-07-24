use anyhow::Result;
use opentelemetry::propagation::Injector;
use opentelemetry::trace::{SpanKind, SpanRef, TraceContextExt};
use opentelemetry::{global, Context};
use opentelemetry::{
    propagation::Extractor,
    trace::{Span, Tracer},
    KeyValue,
};
use opentelemetry_otlp::tonic_types;
use std::collections::HashMap;
use std::fmt::Debug;
use tonic::Request;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub mod attr;
pub mod impls;
pub mod otel_span;

struct MetadataMap<'a>(&'a tonic_types::metadata::MetadataMap);
struct MetadataMutMap<'a>(&'a mut tonic_types::metadata::MetadataMap);

// for server-side metadata extraction
impl Extractor for MetadataMap<'_> {
    /// Get a value for a key from the MetadataMap.  If the value can't be converted to &str, returns None
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|metadata| metadata.to_str().ok())
    }

    /// Collect all the keys from the MetadataMap.
    fn keys(&self) -> Vec<&str> {
        self.0
            .keys()
            .map(|key| match key {
                tonic::metadata::KeyRef::Ascii(v) => v.as_str(),
                tonic::metadata::KeyRef::Binary(v) => v.as_str(),
            })
            .collect::<Vec<_>>()
    }
}

// Trait for tracing requests in OpenTelemetry
impl Injector for MetadataMutMap<'_> {
    /// Set a key and value in the MetadataMap.  Does nothing if the key or value are not valid inputs
    fn set(&mut self, key: &str, value: String) {
        if let Ok(key) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes()) {
            if let Ok(val) = tonic::metadata::MetadataValue::try_from(&value) {
                self.0.insert(key, val);
            }
        }
    }
}

//https://opentelemetry.io/docs/specs/semconv/general/trace/
pub trait Tracing {
    fn create_context(metadata: &HashMap<String, String>) -> Context {
        global::get_text_map_propagator(|propagator| propagator.extract(metadata))
    }
    fn metadata_from_context(cx: &Context) -> HashMap<String, String> {
        let mut metadata = HashMap::new();
        global::get_text_map_propagator(|propagator| propagator.inject_context(cx, &mut metadata));
        metadata
    }
    fn inject_metadata_from_context(metadata: &mut HashMap<String, String>, cx: &Context) {
        global::get_text_map_propagator(|propagator| propagator.inject_context(cx, metadata));
    }
    // XXX not working...
    fn tracing_span_from_metadata(
        metadata: &HashMap<String, String>,
        app_name: &'static str,
        span_name: &'static str,
    ) -> tracing::Span {
        let parent_cx = global::get_text_map_propagator(|prop| prop.extract(metadata));
        let child_tracing_span = tracing::span!(
            tracing::Level::INFO,
            "_",
            "app.name" = app_name,
            "span.name" = span_name
        );
        child_tracing_span.set_parent(parent_cx.clone());
        child_tracing_span
    }
    // XXX not working...
    fn child_tracing_span(
        parent_cx: &opentelemetry::Context,
        app_name: &'static str,
        span_name: String,
    ) -> tracing::Span {
        let span = tracing::info_span!("_", "app.name" = app_name, "span.name" = span_name);
        span.set_parent(parent_cx.clone());
        span
    }
    fn start_child_otel_span(
        parent_cx: &opentelemetry::Context,
        app_name: &'static str,
        span_name: String,
    ) -> global::BoxedSpan {
        global::tracer(app_name).start_with_context(span_name, parent_cx)
    }
    fn otel_span_from_metadata(
        metadata: &HashMap<String, String>,
        app_name: &'static str,
        span_name: &'static str,
    ) -> global::BoxedSpan {
        let parent_cx = global::get_text_map_propagator(|prop| prop.extract(metadata));
        global::tracer(app_name).start_with_context(span_name, &parent_cx)
    }

    fn trace_request<T: Debug>(
        name: &'static str,
        span_name: &'static str,
        request: &Request<T>,
    ) -> global::BoxedSpan {
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let mut span = global::tracer(name).start_with_context(span_name, &parent_cx);

        span.set_attribute(KeyValue::new("service.name", name));
        span.set_attribute(KeyValue::new("service.method", span_name));
        span.set_attribute(KeyValue::new("request", format!("{request:?}")));

        if let Some(req_path) = request.metadata().get("path") {
            if let Ok(path_str) = req_path.to_str() {
                // Clone the string to own it, avoiding reference lifetime issues
                span.set_attribute(KeyValue::new("request.path", path_str.to_string()));
            }
        }

        span
    }
    fn trace_response<T: Debug>(span: &mut global::BoxedSpan, response: &T) {
        span.set_attribute(KeyValue::new("response", format!("{response:?}")));
        span.end();
    }
    fn trace_error(span: &mut global::BoxedSpan, error: &dyn std::error::Error) {
        span.record_error(error);
        span.set_status(opentelemetry::trace::Status::error(error.to_string()));
        span.end();
    }

    // Helper function for tracing gRPC client operations with custom request and context injection
    fn trace_grpc_client_with_request<D, F, Fut, T>(
        context: Option<Context>,
        name: &'static str,
        span_name: &'static str,
        method_name: &'static str,
        mut request_data: tonic::Request<D>,
        operation: F,
    ) -> impl std::future::Future<Output = Result<T>> + Send
    where
        D: Debug + Send + 'static,
        F: FnOnce(tonic::Request<D>) -> Fut + Send,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
        T: Debug + Send + 'static,
    {
        async move {
            let attributes = vec![
                KeyValue::new("rpc.system", "grpc"),
                KeyValue::new("service.name", name),
                KeyValue::new("rpc.method", method_name),
                KeyValue::new("input.value", format!("{:?}", &request_data)),
            ];
            let span =
                Self::start_client_span_with_context(name, span_name, attributes, context.as_ref());
            let cx = context.unwrap_or_else(|| Context::current_with_span(span));

            // Create request and inject trace context
            global::get_text_map_propagator(|propagator| {
                propagator.inject_context(&cx, &mut MetadataMutMap(request_data.metadata_mut()))
            });

            let response = operation(request_data).await;

            match response {
                Ok(res) => {
                    let span = cx.span();
                    span.set_attribute(KeyValue::new("rpc.result", "success"));
                    span.end();
                    Ok(res)
                }
                Err(e) => {
                    let span = cx.span();
                    span.record_error(e.as_ref());
                    span.set_status(opentelemetry::trace::Status::error(e.to_string()));
                    span.end();
                    Err(e)
                }
            }
        }
    }
    fn start_client_span_with_context(
        name: &'static str,
        span_name: &'static str,
        attributes: Vec<KeyValue>,
        context: Option<&Context>,
    ) -> opentelemetry::global::BoxedSpan {
        let tracer = global::tracer(name);
        if let Some(ctx) = context {
            tracer
                .span_builder(span_name)
                .with_kind(SpanKind::Client)
                .with_attributes(attributes)
                .start_with_context(&tracer, ctx)
        } else {
            tracer
                .span_builder(span_name)
                .with_kind(SpanKind::Client)
                .with_attributes(attributes)
                .start(&tracer)
        }
    }
    fn record_error(span: &SpanRef<'_>, error: &str) {
        span.set_status(opentelemetry::trace::Status::error(error.to_string()));
        span.set_attribute(KeyValue::new("error", error.to_string()));
    }
}
