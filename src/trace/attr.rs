use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Span types based on OpenTelemetry standards
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OtelSpanType {
    Span,
    Generation,
    Event,
}

/// Input/Output data for spans
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelSpanData {
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Parsed output data for different telemetry systems
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedOutputData {
    /// External API call result - goes to langfuse.observation.output
    pub observation_output: Option<serde_json::Value>,
    /// LLM completion text - goes to gen_ai.completion
    pub completion_output: Option<serde_json::Value>,
    /// Main trace result - goes to langfuse.trace.output
    pub trace_output: Option<serde_json::Value>,
}

/// Generic span attributes for OpenTelemetry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelSpanAttributes {
    pub span_type: OtelSpanType,
    pub name: String,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub version: Option<String>,
    pub release: Option<String>,
    pub tags: Vec<String>,
    pub data: OtelSpanData,
    pub model: Option<String>,
    pub model_parameters: Option<HashMap<String, serde_json::Value>>,
    pub usage: Option<HashMap<String, i64>>,
    pub level: Option<String>,
    pub status_message: Option<String>,
    pub trace_name: Option<String>,
    pub trace_input: Option<serde_json::Value>,
    pub trace_output: Option<serde_json::Value>,
    pub trace_tags: Vec<String>,
    pub trace_public: Option<bool>,
    pub trace_metadata: Option<HashMap<String, serde_json::Value>>,
    pub prompt_name: Option<String>,
    pub prompt_version: Option<i32>,
    pub cost_details: Option<HashMap<String, f64>>,
    pub completion_start_time: Option<String>,
    // Additional OpenTelemetry standard fields
    pub system: Option<String>,
    pub operation_name: Option<String>,
    pub response_id: Option<String>,
    pub finish_reasons: Vec<String>,
    pub is_stream: Option<bool>,
    pub openinference_span_kind: Option<String>,
}

impl Default for OtelSpanAttributes {
    fn default() -> Self {
        Self {
            span_type: OtelSpanType::Span,
            name: String::new(),
            user_id: None,
            session_id: None,
            version: None,
            release: None,
            tags: Vec::new(),
            data: OtelSpanData {
                input: None,
                output: None,
                metadata: None,
            },
            model: None,
            model_parameters: None,
            usage: None,
            level: None,
            status_message: None,
            trace_name: None,
            trace_input: None,
            trace_output: None,
            trace_tags: Vec::new(),
            trace_public: None,
            trace_metadata: None,
            prompt_name: None,
            prompt_version: None,
            cost_details: None,
            completion_start_time: None,
            system: None,
            operation_name: None,
            response_id: None,
            finish_reasons: Vec::new(),
            is_stream: None,
            openinference_span_kind: None,
        }
    }
}

/// Builder for creating spans with fluent API
#[derive(Debug, Clone)]
pub struct OtelSpanBuilder {
    attributes: OtelSpanAttributes,
}

impl OtelSpanBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            attributes: OtelSpanAttributes {
                name: name.into(),
                ..Default::default()
            },
        }
    }

    pub fn span_type(mut self, span_type: OtelSpanType) -> Self {
        self.attributes.span_type = span_type;
        self
    }

    pub fn user_id(mut self, user_id: impl Into<String>) -> Self {
        self.attributes.user_id = Some(user_id.into());
        self
    }

    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.attributes.session_id = Some(session_id.into());
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.attributes.version = Some(version.into());
        self
    }

    pub fn release(mut self, release: impl Into<String>) -> Self {
        self.attributes.release = Some(release.into());
        self
    }

    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.attributes.tags = tags;
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.attributes.model = Some(model.into());
        self
    }

    pub fn input(mut self, input: serde_json::Value) -> Self {
        self.attributes.data.input = Some(input);
        self
    }

    pub fn output(mut self, output: serde_json::Value) -> Self {
        self.attributes.data.output = Some(output);
        self
    }

    pub fn observation_output(mut self, output: serde_json::Value) -> Self {
        self.attributes.data.output = Some(output);
        self
    }

    pub fn completion_output(mut self, completion: serde_json::Value) -> Self {
        // Store completion output in metadata for later extraction
        let mut metadata = self.attributes.data.metadata.unwrap_or_default();
        metadata.insert("completion_output".to_string(), completion);
        self.attributes.data.metadata = Some(metadata);
        self
    }

    pub fn metadata(mut self, metadata: HashMap<String, serde_json::Value>) -> Self {
        self.attributes.data.metadata = Some(metadata);
        self
    }

    pub fn model_parameters(mut self, params: HashMap<String, serde_json::Value>) -> Self {
        self.attributes.model_parameters = Some(params);
        self
    }

    pub fn usage(mut self, usage: HashMap<String, i64>) -> Self {
        self.attributes.usage = Some(usage);
        self
    }

    pub fn level(mut self, level: impl Into<String>) -> Self {
        self.attributes.level = Some(level.into());
        self
    }

    pub fn status_message(mut self, message: impl Into<String>) -> Self {
        self.attributes.status_message = Some(message.into());
        self
    }

    pub fn trace_name(mut self, name: impl Into<String>) -> Self {
        self.attributes.trace_name = Some(name.into());
        self
    }

    pub fn trace_input(mut self, input: serde_json::Value) -> Self {
        self.attributes.trace_input = Some(input);
        self
    }

    pub fn trace_output(mut self, output: serde_json::Value) -> Self {
        self.attributes.trace_output = Some(output);
        self
    }

    pub fn trace_tags(mut self, tags: Vec<String>) -> Self {
        self.attributes.trace_tags = tags;
        self
    }

    pub fn trace_public(mut self, public: bool) -> Self {
        self.attributes.trace_public = Some(public);
        self
    }

    pub fn trace_metadata(mut self, metadata: HashMap<String, serde_json::Value>) -> Self {
        self.attributes.trace_metadata = Some(metadata);
        self
    }

    pub fn prompt_name(mut self, name: impl Into<String>) -> Self {
        self.attributes.prompt_name = Some(name.into());
        self
    }

    pub fn prompt_version(mut self, version: i32) -> Self {
        self.attributes.prompt_version = Some(version);
        self
    }

    pub fn cost_details(mut self, cost_details: HashMap<String, f64>) -> Self {
        self.attributes.cost_details = Some(cost_details);
        self
    }

    pub fn completion_start_time(mut self, time: impl Into<String>) -> Self {
        self.attributes.completion_start_time = Some(time.into());
        self
    }

    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.attributes.system = Some(system.into());
        self
    }

    pub fn operation_name(mut self, operation_name: impl Into<String>) -> Self {
        self.attributes.operation_name = Some(operation_name.into());
        self
    }

    pub fn response_id(mut self, response_id: impl Into<String>) -> Self {
        self.attributes.response_id = Some(response_id.into());
        self
    }

    pub fn finish_reasons(mut self, finish_reasons: Vec<String>) -> Self {
        self.attributes.finish_reasons = finish_reasons;
        self
    }

    pub fn is_stream(mut self, is_stream: bool) -> Self {
        self.attributes.is_stream = Some(is_stream);
        self
    }

    pub fn openinference_span_kind(mut self, span_kind: impl Into<String>) -> Self {
        self.attributes.openinference_span_kind = Some(span_kind.into());
        self
    }

    pub fn build(self) -> OtelSpanAttributes {
        self.attributes
    }

    // Create builder from existing attributes
    pub fn from_attributes(attributes: OtelSpanAttributes) -> Self {
        Self { attributes }
    }
}
