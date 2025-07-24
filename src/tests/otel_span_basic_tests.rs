//! Basic integration tests for OpenTelemetry tracing

use super::test_utils::*;
use crate::trace::attr::{OtelSpanBuilder, OtelSpanType};
use crate::trace::otel_span::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

/// Test basic span creation and attributes
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_basic_span_creation() -> Result<(), Box<dyn std::error::Error>> {
    let client = setup_integration_test().await?;

    // Test a simple span
    let simple_attributes = OtelSpanBuilder::new("simple-test-span")
        .span_type(OtelSpanType::Span)
        .build();
    let (_span, context) = client.create_trace_context(
        "test context",
        vec!["test".to_string(), "integration".to_string()],
    );

    client
        .with_span_result(simple_attributes, Some(context.clone()), async {
            tracing::info!("Inside a simple test span");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok::<(), std::io::Error>(())
        })
        .await
        .unwrap();

    // Test a span with additional attributes
    let detailed_attributes = OtelSpanBuilder::new("detailed-test-span")
        .span_type(OtelSpanType::Generation)
        .model("gpt-4")
        .user_id("test-user-123")
        .session_id("test-session-456")
        .tags(vec!["test".to_string(), "integration".to_string()])
        .input(json!({"prompt": "Test prompt for integration test"}))
        .build();

    client
        .with_span_result(detailed_attributes, Some(context.clone()), async {
            tracing::info!("Inside a detailed test span");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok::<(), std::io::Error>(())
        })
        .await
        .unwrap();

    // Test span with custom metadata
    let mut metadata = HashMap::new();
    metadata.insert("custom_field".to_string(), json!("custom value"));
    metadata.insert("test_number".to_string(), json!(42));

    let metadata_attributes = OtelSpanBuilder::new("metadata-test-span")
        .span_type(OtelSpanType::Event)
        .level("INFO")
        .metadata(metadata)
        .build();

    client
        .with_span_result(metadata_attributes, Some(context.clone()), async {
            tracing::info!("Inside a metadata test span");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok::<(), std::io::Error>(())
        })
        .await
        .unwrap();

    // Test span with usage metrics
    let mut usage = HashMap::new();
    usage.insert("input_tokens".to_string(), 10);
    usage.insert("output_tokens".to_string(), 20);
    usage.insert("total_tokens".to_string(), 30);

    let usage_attributes = OtelSpanBuilder::new("usage-test-span")
        .span_type(OtelSpanType::Generation)
        .model("gpt-3.5-turbo")
        .usage(usage)
        .build();

    client
        .with_span_result(usage_attributes, Some(context.clone()), async {
            tracing::info!("Inside a usage metrics test span");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok::<(), std::io::Error>(())
        })
        .await
        .unwrap();

    cleanup_integration_test().await;
    Ok(())
}

/// Test nested span relationships
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_nested_spans() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(setup_integration_test().await?);

    // Create parent span
    let parent_attributes = OtelSpanBuilder::new("parent-span")
        .span_type(OtelSpanType::Span)
        .tags(vec!["parent".to_string()])
        .build();

    let client_clone = client.clone();
    client
        .clone()
        .with_span_result(parent_attributes.clone(), None, async move {
            tracing::info!("Inside parent span");

            // Create first child span
            let child1_attributes = OtelSpanBuilder::new("child-span-1")
                .span_type(OtelSpanType::Event)
                .tags(vec!["child".to_string()])
                .build();

            client_clone
                .with_span_result(child1_attributes, None, async {
                    tracing::info!("Inside first child span");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    Ok::<(), std::io::Error>(())
                })
                .await
                .unwrap();

            // Create second child span
            let child2_attributes = OtelSpanBuilder::new("child-span-2")
                .span_type(OtelSpanType::Generation)
                .model("gpt-4")
                .input(json!({"prompt": "Test prompt in child span"}))
                .build();

            let client_clone2 = client_clone.clone();
            client_clone
                .with_span_result(child2_attributes, None, async move {
                    tracing::info!("Inside second child span");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

                    // Create grandchild span
                    let grandchild_attributes = OtelSpanBuilder::new("grandchild-span")
                        .span_type(OtelSpanType::Event)
                        .tags(vec!["grandchild".to_string()])
                        .build();

                    client_clone2
                        .with_span_result(grandchild_attributes, None, async {
                            tracing::info!("Inside grandchild span");
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            Ok::<(), std::io::Error>(())
                        })
                        .await
                        .unwrap();
                    Ok::<(), std::io::Error>(())
                })
                .await
                .unwrap();
            Ok::<(), std::io::Error>(())
        })
        .await
        .unwrap();

    cleanup_integration_test().await;
    Ok(())
}
