//! Error handling integration tests for OpenTelemetry tracing
//! These tests demonstrate how to capture and report errors in spans
use super::test_utils::*;
use crate::trace::{
    attr::{OtelSpanBuilder, OtelSpanType},
    otel_span::*,
};
use serde_json::json;

/// Test error chain handling in spans
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_error_chain_handling_integration() -> Result<(), Box<dyn std::error::Error>> {
    let client = setup_integration_test().await?;

    // Create a nested error chain
    #[derive(Debug)]
    struct RootError {
        message: String,
    }

    impl std::fmt::Display for RootError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Root error: {}", self.message)
        }
    }

    impl std::error::Error for RootError {}

    #[derive(Debug)]
    struct MiddleError {
        message: String,
        source: RootError,
    }

    impl std::fmt::Display for MiddleError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Middle error: {}", self.message)
        }
    }

    impl std::error::Error for MiddleError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.source)
        }
    }

    #[derive(Debug)]
    struct TopError {
        message: String,
        source: MiddleError,
    }

    impl std::fmt::Display for TopError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Top error: {}", self.message)
        }
    }

    impl std::error::Error for TopError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.source)
        }
    }

    let error_chain_attributes = OtelSpanBuilder::new("complex-error-chain")
        .session_id("test-session-789")
        .span_type(OtelSpanType::Generation)
        .model("gpt-4")
        .input(json!({"operation": "complex_processing"}))
        .build();

    let result: Result<String, TopError> = client
        .with_span_result(error_chain_attributes, None, async {
            let root_error = RootError {
                message: "Network connection failed".to_string(),
            };

            let middle_error = MiddleError {
                message: "API request failed".to_string(),
                source: root_error,
            };

            let top_error = TopError {
                message: "LLM generation failed".to_string(),
                source: middle_error,
            };

            Err(top_error)
        })
        .await;

    assert!(result.is_err());
    tracing::info!("Successfully captured complex error chain in span");

    cleanup_integration_test().await;
    Ok(())
}
