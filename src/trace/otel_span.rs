use super::attr::{OtelSpanAttributes, OtelSpanBuilder, OtelSpanType};
use opentelemetry::{
    global::BoxedTracer,
    trace::{Span, SpanBuilder},
    Context,
};
use serde::Serialize;
use std::collections::HashMap;

/// Carrier extractor for extracting context from HTTP headers or similar key-value maps
struct CarrierExtractor<'a>(&'a HashMap<String, String>);

impl opentelemetry::propagation::Extractor for CarrierExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|s| s.as_str())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|s| s.as_str()).collect()
    }
}

/// Carrier injector for injecting context into HTTP headers or similar key-value maps
struct CarrierInjector<'a>(&'a mut HashMap<String, String>);

impl opentelemetry::propagation::Injector for CarrierInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }
}

/// Trait for GenAI OpenTelemetry clients
pub trait GenAIOtelClient: Send + Sync {
    /// Create a span builder with given attributes
    fn create_span_builder(&self, attributes: OtelSpanAttributes) -> SpanBuilder;

    /// Create and start a span with given attributes
    fn start_new_span(&self, attributes: OtelSpanAttributes) -> opentelemetry::global::BoxedSpan;

    fn start_with_context(
        &self,
        attributes: OtelSpanAttributes,
        context: Context,
    ) -> opentelemetry::global::BoxedSpan;

    /// Create a trace for an event
    fn create_event_span(
        &self,
        name: impl Into<String>,
        level: Option<String>,
    ) -> OtelSpanAttributes {
        let mut builder = OtelSpanBuilder::new(name).span_type(OtelSpanType::Event);

        if let Some(level) = level {
            builder = builder.level(level);
        }

        builder.build()
    }

    /// Start and automatically finish a span with error handling
    fn with_span_result<F, T, E>(
        &self,
        attributes: OtelSpanAttributes,
        context: Option<Context>,
        f: F,
    ) -> impl std::future::Future<Output = Result<T, E>> + Send + '_
    where
        F: std::future::Future<Output = Result<T, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
        T: Serialize + Send + 'static,
    {
        self.with_span_result_and_response_parser(
            attributes,
            context,
            f,
            None::<fn(&T) -> Option<OtelSpanAttributes>>,
        )
    }

    /// Start and automatically finish a span with error handling and optional response parsing
    fn with_span_result_and_response_parser<F, T, E, P>(
        &self,
        attributes: OtelSpanAttributes,
        context: Option<Context>,
        f: F,
        response_parser: Option<P>,
    ) -> impl std::future::Future<Output = Result<T, E>> + Send + '_
    where
        F: std::future::Future<Output = Result<T, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
        T: Serialize + Send + 'static,
        P: Fn(&T) -> Option<OtelSpanAttributes> + Send + Sync + 'static,
    {
        async move {
            use opentelemetry::trace::{Span, Status};

            let mut span = if let Some(context) = context {
                // If context is provided, start span with it
                self.start_with_context(attributes, context)
            } else {
                self.start_new_span(attributes)
            };

            // Execute the future and handle errors
            let result = f.await;

            match &result {
                Ok(success_value) => {
                    // Set success status
                    span.set_status(Status::Ok);

                    // Parse outputs using response parser if provided
                    if let Some(parser) = response_parser {
                        if let Some(response_attributes) = parser(success_value) {
                            // Extract parsed outputs from response attributes
                            self.set_parsed_outputs_for_span(&mut span, &response_attributes);

                            // Create response event span as child of current span
                            use opentelemetry::trace::TraceContextExt;

                            // Create context with current span as parent
                            let span_context = span.span_context().clone();
                            let current_context =
                                Context::current().with_remote_span_context(span_context);

                            // Execute within the parent span context
                            let _guard = current_context.attach();

                            // Create child response span within the parent context
                            let mut response_span = self.start_new_span(response_attributes);

                            // Log response information
                            tracing::debug!(
                                parent_span_id = span.span_context().span_id().to_string(),
                                response_span_id =
                                    response_span.span_context().span_id().to_string(),
                                "Response event span created as child of successful result span"
                            );

                            // End response span immediately since it's just an event
                            response_span.end();
                        } else {
                            // Fallback: use default serialization rules
                            self.set_default_outputs_for_span(&mut span, success_value);
                        }
                    } else {
                        // No parser provided: use default serialization rules
                        self.set_default_outputs_for_span(&mut span, success_value);
                    }
                }
                Err(error) => {
                    // Create context with current span as parent for error handling
                    use opentelemetry::trace::TraceContextExt;
                    let span_context = span.span_context().clone();
                    let current_context = Context::current().with_remote_span_context(span_context);

                    // Execute error handling within the parent span context
                    let _guard = current_context.attach();

                    // Use the record_error method for consistent error handling
                    self.record_error(&mut span, &error);

                    // Log the error for additional visibility
                    tracing::error!(
                        error = %error,
                        span_name = span.span_context().span_id().to_string(),
                        "Error occurred in traced span"
                    );
                }
            }

            // End the span manually
            span.end();
            result
        }
    }

    /// Record an error on an existing span without ending it
    fn record_error(
        &self,
        span: &mut opentelemetry::global::BoxedSpan,
        error: &(dyn std::error::Error + Send + Sync),
    ) {
        use opentelemetry::trace::{Span, Status, TraceContextExt};

        // Set error status
        span.set_status(Status::error(error.to_string()));

        // Record error information as span attributes
        span.set_attribute(opentelemetry::KeyValue::new(
            "error.type",
            std::any::type_name::<dyn std::error::Error>().to_string(),
        ));
        span.set_attribute(opentelemetry::KeyValue::new(
            "error.message",
            error.to_string(),
        ));
        span.set_attribute(opentelemetry::KeyValue::new("error.occurred", true));

        // Add error chain if available
        let mut error_chain = Vec::new();
        let mut current_error: &dyn std::error::Error = error;
        while let Some(source) = current_error.source() {
            error_chain.push(source.to_string());
            current_error = source;
        }

        if !error_chain.is_empty() {
            span.set_attribute(opentelemetry::KeyValue::new(
                "error.chain",
                error_chain.join(" -> "),
            ));
        }

        // Create error event span as child of current span to ensure proper context
        let error_attributes = self.create_error_span(error, Some("error_event"));

        // Create context with current span as parent
        let span_context = span.span_context().clone();
        let current_context = Context::current().with_remote_span_context(span_context);

        // Execute within the parent span context
        let _guard = current_context.attach();

        // Create child error event span within the parent context
        let mut error_event_span = self.start_new_span(error_attributes);

        // Record error as an event with stack trace if available
        let mut event_attributes = vec![
            opentelemetry::KeyValue::new("error.occurred", true),
            opentelemetry::KeyValue::new("error.message", error.to_string()),
            opentelemetry::KeyValue::new("error.timestamp", chrono::Utc::now().to_rfc3339()),
        ];

        // Add stack trace if available (simplified for this example)
        if let Some(backtrace) = std::backtrace::Backtrace::capture()
            .to_string()
            .lines()
            .take(10)
            .collect::<Vec<_>>()
            .first()
        {
            event_attributes.push(opentelemetry::KeyValue::new(
                "error.stack_trace",
                backtrace.to_string(),
            ));
        }

        // Add error event to both the main span and the error event span
        span.add_event("error".to_string(), event_attributes.clone());
        error_event_span.add_event("error_detail".to_string(), event_attributes);

        // Log response information
        tracing::debug!(
            parent_span_id = span.span_context().span_id().to_string(),
            error_span_id = error_event_span.span_context().span_id().to_string(),
            "Error event span created as child of failed operation span"
        );

        // End error event span immediately
        error_event_span.end();

        // Log the error for additional visibility
        tracing::error!(
            error = %error,
            error_chain = ?error_chain,
            span_id = span.span_context().span_id().to_string(),
            "Error recorded in span with child error event"
        );
    }

    /// Create an error span for standalone error logging
    fn create_error_span(
        &self,
        error: &(dyn std::error::Error + Send + Sync),
        span_name: Option<&str>,
    ) -> OtelSpanAttributes {
        let span_name = span_name.map_or_else(
            || format!("error: {error}"),
            |ctx| format!("error in {ctx}: {error}"),
        );

        let mut metadata = HashMap::new();
        metadata.insert(
            "error_type".to_string(),
            serde_json::json!(format!("{:?}", error)),
        );
        metadata.insert(
            "error_message".to_string(),
            serde_json::json!(error.to_string()),
        );
        metadata.insert(
            "error_timestamp".to_string(),
            serde_json::json!(chrono::Utc::now().to_rfc3339()),
        );

        // Add error chain
        let mut error_chain = Vec::new();
        let mut current_error: &dyn std::error::Error = error;
        while let Some(source) = current_error.source() {
            error_chain.push(source.to_string());
            current_error = source;
        }
        if !error_chain.is_empty() {
            metadata.insert("error_chain".to_string(), serde_json::json!(error_chain));
        }

        OtelSpanBuilder::new(span_name)
            .span_type(OtelSpanType::Event)
            .level("ERROR")
            .status_message(error.to_string())
            .metadata(metadata)
            .tags(vec!["error".to_string(), "exception".to_string()])
            .build()
    }

    /// Set parsed outputs based on response attributes
    fn set_parsed_outputs_for_span<S>(&self, span: &mut S, response_attributes: &OtelSpanAttributes)
    where
        S: opentelemetry::trace::Span,
    {
        // Set observation output (external API call result)
        if let Some(ref output) = response_attributes.data.output {
            if let Ok(output_str) = serde_json::to_string(output) {
                span.set_attribute(opentelemetry::KeyValue::new(
                    "langfuse.observation.output",
                    output_str,
                ));
            }
        }

        // Set completion output (LLM specific)
        if let Some(ref metadata) = response_attributes.data.metadata {
            if let Some(completion_output) = metadata.get("completion_output") {
                if let Ok(completion_str) = serde_json::to_string(completion_output) {
                    span.set_attribute(opentelemetry::KeyValue::new(
                        "gen_ai.completion",
                        completion_str,
                    ));
                }
            }
        }

        // Set trace output (main processing result)
        if let Some(ref trace_output) = response_attributes.trace_output {
            if let Ok(trace_str) = serde_json::to_string(trace_output) {
                span.set_attribute(opentelemetry::KeyValue::new(
                    "langfuse.trace.output",
                    trace_str,
                ));
            }
        }
    }

    /// Set default outputs when no parser is provided
    fn set_default_outputs_for_span<S, T>(&self, span: &mut S, success_value: &T)
    where
        S: opentelemetry::trace::Span,
        T: Serialize,
    {
        if let Ok(output_value) = serde_json::to_value(success_value) {
            // Default serialization rules:
            // 1. If it's an LLM-related operation (has model info), treat as completion
            // 2. Otherwise, treat as observation output
            // 3. Only use as trace output if it represents main processing result

            let output_str = serde_json::to_string(&output_value).unwrap_or_default();

            // Always set observation output (external call result)
            span.set_attribute(opentelemetry::KeyValue::new(
                "langfuse.observation.output",
                output_str.clone(),
            ));

            // Check if this looks like an LLM completion
            if self.is_llm_completion(&output_value) {
                span.set_attribute(opentelemetry::KeyValue::new(
                    "gen_ai.completion",
                    output_str.clone(),
                ));
            }

            // For now, use the same value for trace output as fallback
            // In practice, this should be set explicitly by the caller
            span.set_attribute(opentelemetry::KeyValue::new(
                "langfuse.trace.output",
                output_str,
            ));
        }
    }

    /// Determine if output looks like an LLM completion
    fn is_llm_completion(&self, output: &serde_json::Value) -> bool {
        // Check if output contains typical LLM completion fields
        if let Some(obj) = output.as_object() {
            obj.contains_key("content")
                || obj.contains_key("message")
                || obj.contains_key("text")
                || obj.contains_key("completion")
        } else if output.is_string() {
            // String output could be LLM completion
            true
        } else {
            false
        }
    }
}

/// Tracer client with support for hierarchical spans
pub trait RemoteSpanClient: GenAIOtelClient + Send + Sync {
    /// Create a new parent span and return its context for child span creation
    fn create_parent_span(
        &self,
        attributes: OtelSpanAttributes,
    ) -> (opentelemetry::global::BoxedSpan, Context) {
        use opentelemetry::trace::{TraceContextExt, Tracer};

        // let extracted_context = global::get_text_map_propagator(|propagator| {
        //     propagator.extract(&CarrierExtractor(&empty_carrier))
        // });
        let context = Context::current();

        // Get the tracer provided by the concrete implementation.
        let tracer = self.get_tracer();
        // Create a span builder using the helper from GenAIOtelClient.
        let span_builder = self.create_span_builder(attributes);

        // Build the span using the extracted_context. This ensures the span's parentage
        // is determined by extracted_context (likely making it a root).
        let span = tracer.build_with_context(span_builder, &context);

        // The context to return should be one where the newly created span is active
        // as a parent for future child spans. Using `with_remote_span_context`
        // achieves this by setting the span's context as the remote parent,
        // which is consulted during child span creation if no local active span is present.
        let new_context_with_span_active =
            context.with_remote_span_context(span.span_context().clone());

        (span, new_context_with_span_active)
    }

    fn create_named_span(&self, span_name: impl Into<String>) -> opentelemetry::global::BoxedSpan {
        use opentelemetry::trace::Tracer;
        let tracer = self.get_tracer();
        tracer.start(span_name.into())
    }

    fn create_child_span(
        &self,
        cx: &Context,
        span_name: impl Into<String>,
    ) -> opentelemetry::global::BoxedSpan {
        use opentelemetry::trace::Tracer;
        let tracer = self.get_tracer();
        tracer.start_with_context(span_name.into(), cx)
    }

    /// Create a child span from a parent context
    fn create_child_span_from_context(
        &self,
        attributes: OtelSpanAttributes,
        parent_context: &Context,
    ) -> opentelemetry::global::BoxedSpan {
        use opentelemetry::trace::Tracer;

        let tracer = self.get_tracer();
        let builder = self.create_span_builder(attributes);
        tracer.build_with_context(builder, parent_context)
    }
    /// Create a new trace context with a root span
    fn create_trace_context(
        &self,
        trace_name: impl Into<String>,
        tags: Vec<String>,
    ) -> (opentelemetry::global::BoxedSpan, Context) {
        // Create trace span attributes
        let trace_attributes = OtelSpanBuilder::new(trace_name)
            .span_type(OtelSpanType::Span)
            .trace_tags(tags)
            .build();

        self.create_parent_span(trace_attributes)
    }

    // Keep the existing remote span methods and other methods unchanged
    /// Create a remote child span from propagated trace context (for distributed tracing)
    fn with_remote_child_span_result<F, T, E>(
        &self,
        attributes: OtelSpanAttributes,
        carrier: &HashMap<String, String>,
        f: F,
    ) -> impl std::future::Future<Output = Result<T, E>> + '_
    where
        F: std::future::Future<Output = Result<T, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
        T: Serialize + Send + 'static,
    {
        self.with_remote_child_span_result_and_response_parser(
            attributes,
            carrier,
            f,
            None::<fn(&T) -> Option<OtelSpanAttributes>>,
        )
    }

    /// Create a remote child span from propagated trace context with response parsing (for distributed tracing)
    fn with_remote_child_span_result_and_response_parser<F, T, E, P>(
        &self,
        attributes: OtelSpanAttributes,
        carrier: &HashMap<String, String>,
        f: F,
        response_parser: Option<P>,
    ) -> impl std::future::Future<Output = Result<T, E>> + '_
    where
        F: std::future::Future<Output = Result<T, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
        T: Serialize + Send + 'static,
        P: Fn(&T) -> Option<OtelSpanAttributes> + Send + Sync + 'static,
    {
        let carrier = carrier.clone();
        async move {
            use opentelemetry::{
                global,
                trace::{Span, Status, TraceContextExt, Tracer},
            };

            // Extract context from carrier using the global propagator
            let parent_context = global::get_text_map_propagator(|propagator| {
                propagator.extract(&CarrierExtractor(&carrier))
            });

            let tracer = self.get_tracer();
            let builder = self.create_span_builder(attributes);

            // Build span with extracted parent context
            let span = tracer.build_with_context(builder, &parent_context);

            let new_context = parent_context.with_span(span);
            let new_context_clone = new_context.clone();

            // Execute the function with the child span context and handle errors
            let result = {
                let _guard = new_context.attach();
                f.await
            };

            // Get span back from context to use after the execution
            let span = new_context_clone.span();

            match &result {
                Ok(success_value) => {
                    // Set success status
                    span.set_status(Status::Ok);

                    // Parse outputs using response parser if provided
                    if let Some(parser) = response_parser {
                        if let Some(response_attributes) = parser(success_value) {
                            // For SpanRef, we need to set attributes directly without helper method
                            // Set observation output (external API call result)
                            if let Some(ref output) = response_attributes.data.output {
                                if let Ok(output_str) = serde_json::to_string(output) {
                                    span.set_attribute(opentelemetry::KeyValue::new(
                                        "langfuse.observation.output",
                                        output_str,
                                    ));
                                }
                            }

                            // Set completion output (LLM specific)
                            if let Some(ref metadata) = response_attributes.data.metadata {
                                if let Some(completion_output) = metadata.get("completion_output") {
                                    if let Ok(completion_str) =
                                        serde_json::to_string(completion_output)
                                    {
                                        span.set_attribute(opentelemetry::KeyValue::new(
                                            "gen_ai.completion",
                                            completion_str,
                                        ));
                                    }
                                }
                            }

                            // Set trace output (main processing result)
                            if let Some(ref trace_output) = response_attributes.trace_output {
                                if let Ok(trace_str) = serde_json::to_string(trace_output) {
                                    span.set_attribute(opentelemetry::KeyValue::new(
                                        "langfuse.trace.output",
                                        trace_str,
                                    ));
                                }
                            }

                            // Create response event span as child of current span
                            let span_context = span.span_context().clone();
                            let current_context =
                                new_context_clone.with_remote_span_context(span_context);

                            // Execute within the parent span context
                            let _guard = current_context.attach();

                            // Create child response span within the parent context
                            let mut response_span = self.start_new_span(response_attributes);

                            // Log response information
                            tracing::debug!(
                                parent_span_id = span.span_context().span_id().to_string(),
                                response_span_id = response_span.span_context().span_id().to_string(),
                                "Remote response event span created as child of successful remote child span"
                            );

                            // End response span immediately since it's just an event
                            response_span.end();
                        } else {
                            // Fallback: use default serialization rules for SpanRef
                            if let Ok(output_value) = serde_json::to_value(success_value) {
                                let output_str =
                                    serde_json::to_string(&output_value).unwrap_or_default();

                                // Always set observation output (external call result)
                                span.set_attribute(opentelemetry::KeyValue::new(
                                    "langfuse.observation.output",
                                    output_str.clone(),
                                ));

                                // Check if this looks like an LLM completion
                                if self.is_llm_completion(&output_value) {
                                    span.set_attribute(opentelemetry::KeyValue::new(
                                        "gen_ai.completion",
                                        output_str.clone(),
                                    ));
                                }

                                // Use the same value for trace output as fallback
                                span.set_attribute(opentelemetry::KeyValue::new(
                                    "langfuse.trace.output",
                                    output_str,
                                ));
                            }
                        }
                    } else {
                        // No parser provided: use default serialization rules for SpanRef
                        if let Ok(output_value) = serde_json::to_value(success_value) {
                            let output_str =
                                serde_json::to_string(&output_value).unwrap_or_default();

                            // Always set observation output (external call result)
                            span.set_attribute(opentelemetry::KeyValue::new(
                                "langfuse.observation.output",
                                output_str.clone(),
                            ));

                            // Check if this looks like an LLM completion
                            if self.is_llm_completion(&output_value) {
                                span.set_attribute(opentelemetry::KeyValue::new(
                                    "gen_ai.completion",
                                    output_str.clone(),
                                ));
                            }

                            // Use the same value for trace output as fallback
                            span.set_attribute(opentelemetry::KeyValue::new(
                                "langfuse.trace.output",
                                output_str,
                            ));
                        }
                    }
                }
                Err(error) => {
                    // Create context with current span as parent for error handling
                    let span_context = span.span_context().clone();
                    let current_context = new_context_clone.with_remote_span_context(span_context);
                    span.set_status(Status::error(error.to_string()));

                    // Execute error handling within the parent span context
                    let _guard = current_context.attach();

                    // Use the record_error method for consistent error handling
                    span.record_error(&error);

                    // Log the error for additional visibility
                    tracing::error!(
                        error = %error,
                        span_name = span.span_context().span_id().to_string(),
                        "Error occurred in traced remote child span"
                    );
                }
            }

            // End the span manually
            span.end();
            self.inject_context_into_carrier(&new_context_clone, &mut carrier.clone());

            result
        }
    }

    /// Inject specific context into a carrier for distributed tracing
    fn inject_context_into_carrier(
        &self,
        context: &Context,
        carrier: &mut HashMap<String, String>,
    ) {
        use opentelemetry::global;

        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(context, &mut CarrierInjector(carrier));
        });
    }

    /// Get the tracer for span operations - should be implemented by the concrete client
    fn get_tracer(&self) -> BoxedTracer;
}
