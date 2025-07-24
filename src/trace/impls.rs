use super::{
    attr::{OtelSpanAttributes, OtelSpanType},
    otel_span::{GenAIOtelClient, RemoteSpanClient},
};
use opentelemetry::{
    global::{self, BoxedTracer},
    trace::{SpanBuilder, SpanKind, Tracer},
    Context, KeyValue,
};

/// Generic OpenTelemetry client implementation
/// Uses the global tracer provider initialized by command_utils::util::tracing
#[derive(Debug, Clone)]
pub struct GenericOtelClient {
    tracer_name: String,
}

impl GenericOtelClient {
    /// Create a new client that uses the global tracer provider
    pub fn new(tracer_name: impl Into<String>) -> Self {
        Self {
            tracer_name: tracer_name.into(),
        }
    }

    /// Get the global tracer for this client
    fn get_tracer(&self) -> BoxedTracer {
        global::tracer(self.tracer_name.clone())
    }

    /// Get the tracer name - exposed for compatibility with context-aware client
    pub fn get_tracer_name(&self) -> String {
        self.tracer_name.clone()
    }
}

// Implement the trait for both client types
impl RemoteSpanClient for GenericOtelClient {
    fn get_tracer(&self) -> BoxedTracer {
        self.get_tracer()
    }
}

impl GenAIOtelClient for GenericOtelClient {
    /// Create a span builder with OpenTelemetry attributes
    fn create_span_builder(&self, attributes: OtelSpanAttributes) -> SpanBuilder {
        let tracer = self.get_tracer();
        let mut builder = tracer.span_builder(attributes.name.clone());

        // Set span kind based on type
        builder = builder.with_kind(match attributes.span_type {
            OtelSpanType::Generation => SpanKind::Producer,
            OtelSpanType::Event => SpanKind::Internal,
            OtelSpanType::Span => SpanKind::Internal,
        });

        let mut key_values = vec![];

        // Core Langfuse observation attributes
        key_values.push(KeyValue::new(
            "langfuse.observation.type",
            match attributes.span_type {
                OtelSpanType::Span => "span",
                OtelSpanType::Generation => "generation",
                OtelSpanType::Event => "event",
            },
        ));

        // User and session identification
        if let Some(user_id) = attributes.user_id {
            key_values.push(KeyValue::new("user.id", user_id));
        }

        if let Some(session_id) = attributes.session_id {
            key_values.push(KeyValue::new("session.id", session_id));
        }

        // Version and release information
        if let Some(version) = attributes.version {
            key_values.push(KeyValue::new("langfuse.version", version));
        }

        if let Some(release) = attributes.release {
            key_values.push(KeyValue::new("langfuse.release", release));
        }

        // Tags as array values or comma-separated strings
        if !attributes.tags.is_empty() {
            // For compatibility with Langfuse server, use array format
            let tags_str = serde_json::to_string(&attributes.tags).unwrap_or_default();
            key_values.push(KeyValue::new("langfuse.trace.tags", tags_str));
        }

        // Model information (OpenTelemetry gen_ai.* standard attributes)
        if let Some(model) = attributes.model {
            key_values.push(KeyValue::new(
                "langfuse.observation.model.name",
                model.clone(),
            ));
            // Also add gen_ai standard attributes for compatibility
            key_values.push(KeyValue::new("gen_ai.request.model", model));
        }

        // OpenTelemetry gen_ai.* standard attributes for model parameters
        if let Some(model_parameters) = &attributes.model_parameters {
            // Add as JSON string for Langfuse
            if let Ok(params_str) = serde_json::to_string(model_parameters) {
                key_values.push(KeyValue::new(
                    "langfuse.observation.model_parameters",
                    params_str,
                ));
            }

            // Also add individual gen_ai.request.* attributes for OpenTelemetry compatibility
            for (key, value) in model_parameters {
                match value {
                    serde_json::Value::Number(n) if n.is_f64() => {
                        key_values.push(KeyValue::new(
                            format!("gen_ai.request.{key}"),
                            n.as_f64().unwrap(),
                        ));
                    }
                    serde_json::Value::Number(n) if n.is_i64() => {
                        key_values.push(KeyValue::new(
                            format!("gen_ai.request.{key}"),
                            n.as_i64().unwrap(),
                        ));
                    }
                    serde_json::Value::Bool(b) => {
                        key_values.push(KeyValue::new(format!("gen_ai.request.{key}"), *b));
                    }
                    serde_json::Value::String(s) => {
                        key_values.push(KeyValue::new(format!("gen_ai.request.{key}"), s.clone()));
                    }
                    _ => {
                        if let Ok(value_str) = serde_json::to_string(value) {
                            key_values
                                .push(KeyValue::new(format!("gen_ai.request.{key}"), value_str));
                        }
                    }
                }
            }
        }

        // Observation level and status
        if let Some(level) = attributes.level {
            key_values.push(KeyValue::new("langfuse.observation.level", level));
        }

        if let Some(status_message) = attributes.status_message {
            key_values.push(KeyValue::new(
                "langfuse.observation.status_message",
                status_message,
            ));
        }

        // Input/output data as JSON strings and gen_ai.prompt/completion attributes
        if let Some(ref input) = attributes.data.input {
            if let Ok(input_str) = serde_json::to_string(input) {
                key_values.push(KeyValue::new(
                    "langfuse.observation.input",
                    input_str.clone(),
                ));
                // Add gen_ai.prompt for OpenTelemetry compatibility
                key_values.push(KeyValue::new("gen_ai.prompt", input_str));
            }
        }

        if let Some(ref output) = attributes.data.output {
            if let Ok(output_str) = serde_json::to_string(output) {
                key_values.push(KeyValue::new(
                    "langfuse.observation.output",
                    output_str.clone(),
                ));
                // Add gen_ai.completion for OpenTelemetry compatibility
                key_values.push(KeyValue::new("gen_ai.completion", output_str));
            }
        }

        // Metadata with proper Langfuse naming
        if let Some(metadata) = attributes.data.metadata {
            for (key, value) in metadata {
                if let Ok(value_str) = serde_json::to_string(&value) {
                    key_values.push(KeyValue::new(
                        format!("langfuse.observation.metadata.{key}"),
                        value_str,
                    ));
                }
            }
        }

        // Model parameters as JSON string
        if let Some(model_parameters) = attributes.model_parameters {
            if let Ok(params_str) = serde_json::to_string(&model_parameters) {
                key_values.push(KeyValue::new(
                    "langfuse.observation.model_parameters",
                    params_str,
                ));
            }
        }

        // Usage details as JSON string and individual gen_ai.usage.* attributes
        if let Some(usage) = &attributes.usage {
            if let Ok(usage_str) = serde_json::to_string(usage) {
                key_values.push(KeyValue::new(
                    "langfuse.observation.usage_details",
                    usage_str,
                ));
            }

            // Add individual gen_ai.usage.* attributes for OpenTelemetry compatibility
            for (key, value) in usage {
                match key.as_str() {
                    "input_tokens" | "promptTokens" | "prompt" => {
                        key_values.push(KeyValue::new("gen_ai.usage.input_tokens", *value));
                        key_values.push(KeyValue::new("llm.token_count.prompt", *value));
                    }
                    "output_tokens" | "completionTokens" | "completion" => {
                        key_values.push(KeyValue::new("gen_ai.usage.completion_tokens", *value));
                        key_values.push(KeyValue::new("llm.token_count.completion", *value));
                    }
                    "total_tokens" | "totalTokens" | "total" => {
                        key_values.push(KeyValue::new("gen_ai.usage.total_tokens", *value));
                        key_values.push(KeyValue::new("llm.token_count.total", *value));
                    }
                    _ => {
                        // Custom usage metrics
                        key_values.push(KeyValue::new(format!("gen_ai.usage.{key}"), *value));
                    }
                }
            }
        }

        // Cost details as JSON string and gen_ai.usage.cost
        if let Some(cost_details) = &attributes.cost_details {
            if let Ok(cost_str) = serde_json::to_string(cost_details) {
                key_values.push(KeyValue::new("langfuse.observation.cost_details", cost_str));
            }

            // Add total cost as gen_ai.usage.cost for OpenTelemetry compatibility
            if let Some(total_cost) = cost_details.get("total") {
                key_values.push(KeyValue::new("gen_ai.usage.cost", *total_cost));
            }
        }

        // Prompt information
        if let Some(prompt_name) = attributes.prompt_name {
            key_values.push(KeyValue::new("langfuse.prompt.name", prompt_name));
        }

        if let Some(prompt_version) = attributes.prompt_version {
            key_values.push(KeyValue::new(
                "langfuse.prompt.version",
                prompt_version.to_string(),
            ));
        }

        // Completion start time
        if let Some(completion_start_time) = attributes.completion_start_time {
            key_values.push(KeyValue::new(
                "langfuse.observation.completion_start_time",
                completion_start_time,
            ));
        }

        if let Some(trace_name) = attributes.trace_name {
            key_values.push(KeyValue::new("langfuse.trace.name", trace_name));
        }

        if let Some(trace_input) = attributes.trace_input {
            if let Ok(input_str) = serde_json::to_string(&trace_input) {
                key_values.push(KeyValue::new("langfuse.trace.input", input_str));
            }
        }

        if let Some(trace_output) = attributes.trace_output {
            if let Ok(output_str) = serde_json::to_string(&trace_output) {
                key_values.push(KeyValue::new("langfuse.trace.output", output_str));
            }
        }

        if !attributes.trace_tags.is_empty() {
            let trace_tags_str = serde_json::to_string(&attributes.trace_tags).unwrap_or_default();
            key_values.push(KeyValue::new("langfuse.trace.tags", trace_tags_str));
        }

        if let Some(trace_public) = attributes.trace_public {
            key_values.push(KeyValue::new("langfuse.trace.public", trace_public));
        }

        if let Some(trace_metadata) = attributes.trace_metadata {
            for (key, value) in trace_metadata {
                if let Ok(value_str) = serde_json::to_string(&value) {
                    key_values.push(KeyValue::new(
                        format!("langfuse.trace.metadata.{key}"),
                        value_str,
                    ));
                }
            }
        }

        // Add OpenTelemetry gen_ai.* standard attributes for broader compatibility
        if let Some(system) = attributes.system {
            key_values.push(KeyValue::new("gen_ai.system", system));
        }

        if let Some(operation_name) = attributes.operation_name {
            key_values.push(KeyValue::new("gen_ai.operation.name", operation_name));
        }

        if let Some(response_id) = attributes.response_id {
            key_values.push(KeyValue::new("gen_ai.response.id", response_id));
        }

        if !attributes.finish_reasons.is_empty() {
            for (i, reason) in attributes.finish_reasons.iter().enumerate() {
                key_values.push(KeyValue::new(
                    format!("gen_ai.completion.{i}.finish_reason"),
                    reason.clone(),
                ));
            }
        }

        if let Some(is_stream) = attributes.is_stream {
            key_values.push(KeyValue::new("gen_ai.request.is_stream", is_stream));
        }

        // OpenInference span kind for compatibility with Phoenix/Arize
        if let Some(openinference_span_kind) = attributes.openinference_span_kind {
            key_values.push(KeyValue::new(
                "openinference.span.kind",
                openinference_span_kind,
            ));
        }

        // // TraceLoop compatibility attributes
        // if let Some(ref input) = attributes.data.input {
        //     if let Ok(input_str) = serde_json::to_string(input) {
        //         key_values.push(KeyValue::new("traceloop.entity.input", input_str));
        //     }
        // }

        // if let Some(ref output) = attributes.data.output {
        //     if let Ok(output_str) = serde_json::to_string(output) {
        //         key_values.push(KeyValue::new("traceloop.entity.output", output_str));
        //     }
        // }

        // // MLFlow compatibility attributes
        // if let Some(ref input) = attributes.data.input {
        //     if let Ok(input_str) = serde_json::to_string(input) {
        //         key_values.push(KeyValue::new("mlflow.spanInputs", input_str));
        //     }
        // }

        // if let Some(ref output) = attributes.data.output {
        //     if let Ok(output_str) = serde_json::to_string(output) {
        //         key_values.push(KeyValue::new("mlflow.spanOutputs", output_str));
        //     }
        // }

        // // SmolAgents compatibility
        // if let Some(ref input) = attributes.data.input {
        //     if let Ok(input_str) = serde_json::to_string(input) {
        //         key_values.push(KeyValue::new("input.value", input_str));
        //     }
        // }

        // if let Some(ref output) = attributes.data.output {
        //     if let Ok(output_str) = serde_json::to_string(output) {
        //         key_values.push(KeyValue::new("output.value", output_str));
        //     }
        // }

        // // Pydantic/Pipecat compatibility
        // if let Some(ref input) = attributes.data.input {
        //     if let Ok(input_str) = serde_json::to_string(input) {
        //         key_values.push(KeyValue::new("input", input_str));
        //     }
        // }

        // if let Some(ref output) = attributes.data.output {
        //     if let Ok(output_str) = serde_json::to_string(output) {
        //         key_values.push(KeyValue::new("output", output_str));
        //     }
        // }

        builder.with_attributes(key_values)
    }

    /// Create and start a span with attributes
    fn start_new_span(&self, attributes: OtelSpanAttributes) -> opentelemetry::global::BoxedSpan {
        let builder = self.create_span_builder(attributes);
        let tracer = self.get_tracer();
        tracer.build(builder)
    }
    fn start_with_context(
        &self,
        attributes: OtelSpanAttributes,
        context: Context,
    ) -> opentelemetry::global::BoxedSpan {
        let builder = self.create_span_builder(attributes);
        let tracer = self.get_tracer();
        tracer.build_with_context(builder, &context)
    }
}
