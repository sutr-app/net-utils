//! Integration tests for remote child span functionality with distributed tracing
use super::test_utils::*;
use crate::trace::attr::{OtelSpanAttributes, OtelSpanBuilder, OtelSpanType};
use crate::trace::otel_span::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Test remote child span with successful operation and response parsing
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_remote_child_span_result_with_response_parser(
) -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(setup_integration_test().await?);

    // Test unique ID for this test run
    let test_id = format!("remote-success-{}", chrono::Utc::now().timestamp());
    let test_user_id = format!("user-{test_id}");
    let test_session_id = format!("session-{test_id}");

    // Simulate distributed tracing by creating a carrier with trace context
    let mut carrier = HashMap::new();

    // Create parent span first to establish trace context
    let parent_attributes = OtelSpanBuilder::new(format!("parent-service-{test_id}"))
        .span_type(OtelSpanType::Span)
        .user_id(test_user_id.clone())
        .session_id(test_session_id.clone())
        .system("parent-service")
        .operation_name("initiate_request")
        .tags(vec!["distributed".to_string(), "parent".to_string()])
        .input(json!({ "request_id": test_id }))
        .build();

    // Create parent span and get context, then inject into carrier
    let (parent_span, parent_context) = client.create_parent_span(parent_attributes);

    // Inject the specific parent context into carrier for distributed tracing
    // This ensures the remote service gets the correct trace context
    client.inject_context_into_carrier(&parent_context, &mut carrier);

    tracing::info!("Parent span created, trace context injected into carrier");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Now use the carrier to create remote child span
    let remote_attributes = OtelSpanBuilder::new(format!("remote-service-operation-{test_id}"))
        .span_type(OtelSpanType::Generation)
        .model("remote-llm-model")
        .user_id(test_user_id.clone())
        .session_id(test_session_id.clone())
        .system("remote-service")
        .operation_name("process_request")
        .tags(vec!["distributed".to_string(), "remote".to_string()])
        .input(json!({
            "query": "Process this distributed request",
            "parent_request_id": test_id
        }))
        .build();

    // Define response parser
    let response_parser = |response: &serde_json::Value| -> Option<OtelSpanAttributes> {
        Some(
            OtelSpanBuilder::new("remote-service-response")
                .span_type(OtelSpanType::Event)
                .level("INFO")
                .output(response.clone())
                .metadata(HashMap::from([
                    ("event_type".to_string(), json!("remote_response")),
                    (
                        "response_size".to_string(),
                        json!(response.to_string().len()),
                    ),
                ]))
                .build(),
        )
    };

    // Execute remote child span with response parsing
    let result = client
        .with_remote_child_span_result_and_response_parser(
            remote_attributes,
            &carrier,
            async move {
                tracing::info!("Processing request in remote service");
                tokio::time::sleep(Duration::from_millis(200)).await;

                // Simulate successful processing
                let response = json!({
                    "status": "success",
                    "result": "Remote processing completed successfully",
                    "processing_time_ms": 200,
                    "model_used": "remote-llm-model",
                    "tokens_used": 150
                });

                Ok::<serde_json::Value, std::io::Error>(response)
            },
            Some(response_parser),
        )
        .await?;

    // Verify response
    assert_eq!(result["status"], "success");
    assert_eq!(result["result"], "Remote processing completed successfully");

    tracing::info!(
        response = %result,
        "Remote child span with response parsing completed successfully"
    );

    // Clean up spans
    drop(parent_span);

    cleanup_integration_test().await;
    Ok(())
}

/// Test remote child span with error handling
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_remote_child_span_result_with_error() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(setup_integration_test().await?);

    // Test unique ID for this test run
    let test_id = format!("remote-error-{}", chrono::Utc::now().timestamp());
    let test_user_id = format!("user-{test_id}");
    let test_session_id = format!("session-{test_id}");

    // Create custom error for testing
    #[derive(Debug)]
    struct RemoteServiceError(String);
    impl std::fmt::Display for RemoteServiceError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Remote service error: {}", self.0)
        }
    }
    impl std::error::Error for RemoteServiceError {}

    // Create carrier with trace context
    let mut carrier = HashMap::new();

    // Create parent span context
    let parent_attributes = OtelSpanBuilder::new(format!("parent-error-service-{test_id}"))
        .span_type(OtelSpanType::Span)
        .user_id(test_user_id.clone())
        .session_id(test_session_id.clone())
        .system("parent-service")
        .operation_name("initiate_failing_request")
        .tags(vec!["distributed".to_string(), "error-test".to_string()])
        .build();

    // Create parent span and inject context into carrier
    let (parent_span, parent_context) = client.create_parent_span(parent_attributes);
    client.inject_context_into_carrier(&parent_context, &mut carrier);

    tracing::info!("Parent span for error test created");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Create remote child span that will fail
    let remote_attributes = OtelSpanBuilder::new(format!("failing-remote-operation-{test_id}"))
        .span_type(OtelSpanType::Generation)
        .model("failing-remote-model")
        .user_id(test_user_id.clone())
        .session_id(test_session_id.clone())
        .system("remote-service")
        .operation_name("failing_process")
        .tags(vec!["distributed".to_string(), "error-prone".to_string()])
        .input(json!({
            "query": "This will fail",
            "parent_request_id": test_id
        }))
        .build();

    // Execute remote child span that will fail
    let result = client
        .with_remote_child_span_result_and_response_parser(
            remote_attributes,
            &carrier,
            async move {
                tracing::info!("Starting operation that will fail in remote service");
                tokio::time::sleep(Duration::from_millis(100)).await;

                // Simulate failure
                Err::<serde_json::Value, _>(RemoteServiceError(
                    "Remote processing failed due to model unavailability".to_string(),
                ))
            },
            None::<fn(&serde_json::Value) -> Option<OtelSpanAttributes>>,
        )
        .await;

    // Verify error was handled correctly
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Remote processing failed"));

    tracing::info!(
        error = %error,
        "Remote child span error handling test completed successfully"
    );

    drop(parent_span);

    cleanup_integration_test().await;
    Ok(())
}

/// Test multiple remote child spans with different carriers
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_multiple_remote_child_spans() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(setup_integration_test().await?);

    // Test unique ID for this test run
    let test_id = format!("multi-remote-{}", chrono::Utc::now().timestamp());
    let test_user_id = format!("user-{test_id}");
    let test_session_id = format!("session-{test_id}");

    // Create different carriers for different services
    let mut carrier_service_a = HashMap::new();
    let mut carrier_service_b = HashMap::new();

    // Create parent spans for different services
    let parent_a_attributes = OtelSpanBuilder::new(format!("parent-service-a-{test_id}"))
        .span_type(OtelSpanType::Span)
        .user_id(test_user_id.clone())
        .session_id(test_session_id.clone())
        .system("service-a")
        .operation_name("initiate_workflow")
        .tags(vec!["distributed".to_string(), "service-a".to_string()])
        .build();

    let parent_b_attributes = OtelSpanBuilder::new(format!("parent-service-b-{test_id}"))
        .span_type(OtelSpanType::Span)
        .user_id(test_user_id.clone())
        .session_id(test_session_id.clone())
        .system("service-b")
        .operation_name("initiate_workflow")
        .tags(vec!["distributed".to_string(), "service-b".to_string()])
        .build();

    // Create parent contexts and inject into carriers
    let (parent_span_a, parent_context_a) = client.create_parent_span(parent_a_attributes);
    client.inject_context_into_carrier(&parent_context_a, &mut carrier_service_a);

    let (parent_span_b, parent_context_b) = client.create_parent_span(parent_b_attributes);
    client.inject_context_into_carrier(&parent_context_b, &mut carrier_service_b);

    tracing::info!("Both parent spans created and contexts injected");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Define response parser for Service A
    let response_parser_a = |response: &String| -> Option<OtelSpanAttributes> {
        Some(
            OtelSpanBuilder::new("service-a-response")
                .span_type(OtelSpanType::Event)
                .level("INFO")
                .output(json!(response))
                .metadata(HashMap::from([
                    ("service".to_string(), json!("service-a")),
                    ("response_length".to_string(), json!(response.len())),
                ]))
                .build(),
        )
    };

    // Define response parser for Service B
    let response_parser_b = |response: &i32| -> Option<OtelSpanAttributes> {
        Some(
            OtelSpanBuilder::new("service-b-response")
                .span_type(OtelSpanType::Event)
                .level("INFO")
                .output(json!({ "computed_value": response }))
                .metadata(HashMap::from([
                    ("service".to_string(), json!("service-b")),
                    ("computation_result".to_string(), json!(response)),
                ]))
                .build(),
        )
    };

    // Execute remote operations in parallel
    let (result_a, result_b) = tokio::join!(
        // Service A operation
        client.with_remote_child_span_result_and_response_parser(
            OtelSpanBuilder::new(format!("remote-service-a-operation-{test_id}"))
                .span_type(OtelSpanType::Generation)
                .model("service-a-model")
                .user_id(test_user_id.clone())
                .session_id(test_session_id.clone())
                .system("service-a")
                .operation_name("text_processing")
                .input(json!({ "text": "Process this text" }))
                .build(),
            &carrier_service_a,
            async move {
                tracing::info!("Service A processing text");
                tokio::time::sleep(Duration::from_millis(150)).await;
                Ok::<String, std::io::Error>("Processed text result from Service A".to_string())
            },
            Some(response_parser_a),
        ),
        // Service B operation
        client.with_remote_child_span_result_and_response_parser(
            OtelSpanBuilder::new(format!("remote-service-b-operation-{test_id}"))
                .span_type(OtelSpanType::Generation)
                .model("service-b-model")
                .user_id(test_user_id.clone())
                .session_id(test_session_id.clone())
                .system("service-b")
                .operation_name("numerical_computation")
                .input(json!({ "numbers": [1, 2, 3, 4, 5] }))
                .build(),
            &carrier_service_b,
            async move {
                tracing::info!("Service B computing numbers");
                tokio::time::sleep(Duration::from_millis(120)).await;
                Ok::<i32, std::io::Error>(42)
            },
            Some(response_parser_b),
        )
    );

    // Verify both operations succeeded
    let text_result = result_a?;
    let number_result = result_b?;

    assert_eq!(text_result, "Processed text result from Service A");
    assert_eq!(number_result, 42);

    tracing::info!(
        text_result = %text_result,
        number_result = number_result,
        "Multiple remote child spans completed successfully"
    );

    // Clean up spans
    drop(parent_span_a);
    drop(parent_span_b);

    cleanup_integration_test().await;
    Ok(())
}

/// Test remote child span without carrier (should still work with empty trace context)
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_remote_child_span_without_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(setup_integration_test().await?);

    // Test unique ID for this test run
    let test_id = format!("no-carrier-{}", chrono::Utc::now().timestamp());
    let test_user_id = format!("user-{test_id}");
    let test_session_id = format!("session-{test_id}");

    // Empty carrier (no trace context)
    let empty_carrier = HashMap::new();

    let remote_attributes = OtelSpanBuilder::new(format!("standalone-remote-operation-{test_id}"))
        .span_type(OtelSpanType::Span)
        .user_id(test_user_id)
        .session_id(test_session_id)
        .system("standalone-service")
        .operation_name("standalone_process")
        .tags(vec!["standalone".to_string(), "no-parent".to_string()])
        .input(json!({ "data": "standalone processing" }))
        .build();

    // Execute remote child span without parent context
    let result = client
        .with_remote_child_span_result(remote_attributes, &empty_carrier, async move {
            tracing::info!("Processing standalone request without parent trace");
            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok::<String, std::io::Error>("Standalone processing completed".to_string())
        })
        .await?;

    assert_eq!(result, "Standalone processing completed");

    tracing::info!(
        result = %result,
        "Standalone remote child span completed successfully"
    );

    cleanup_integration_test().await;
    Ok(())
}

/// Test error propagation through multiple distributed services
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_distributed_error_propagation() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(setup_integration_test().await?);

    // Test unique ID for this test run
    let test_id = format!("error-propagation-{}", chrono::Utc::now().timestamp());
    let test_user_id = format!("user-{test_id}");
    let test_session_id = format!("session-{test_id}");

    // Custom error types for different services
    #[derive(Debug)]
    struct ServiceAError(String);
    impl std::fmt::Display for ServiceAError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Service A error: {}", self.0)
        }
    }
    impl std::error::Error for ServiceAError {}

    #[derive(Debug)]
    struct ServiceBError(String);
    impl std::fmt::Display for ServiceBError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Service B error: {}", self.0)
        }
    }
    impl std::error::Error for ServiceBError {}

    // Create carrier chain
    let mut carrier_a = HashMap::new();
    let carrier_b = HashMap::new();

    // Service A creates trace context
    let parent_attributes = OtelSpanBuilder::new(format!("error-chain-parent-{test_id}"))
        .span_type(OtelSpanType::Span)
        .user_id(test_user_id.clone())
        .session_id(test_session_id.clone())
        .system("orchestrator")
        .operation_name("error_chain_test")
        .tags(vec!["error-chain".to_string(), "distributed".to_string()])
        .build();

    // Create parent span and inject context
    let (parent_span, parent_context) = client.create_parent_span(parent_attributes);
    client.inject_context_into_carrier(&parent_context, &mut carrier_a);

    // Service A operation succeeds
    let service_a_result = client
        .with_remote_child_span_result(
            OtelSpanBuilder::new(format!("service-a-{test_id}"))
                .span_type(OtelSpanType::Span)
                .user_id(test_user_id.clone())
                .session_id(test_session_id.clone())
                .system("service-a")
                .operation_name("prepare_for_service_b")
                .build(),
            &carrier_a,
            async move {
                tracing::info!("Service A preparing for Service B call");
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok::<String, ServiceAError>("Service A preparation complete".to_string())
            },
        )
        .await;

    assert!(service_a_result.is_ok());

    // Service B operation that fails
    let service_b_result = client
        .with_remote_child_span_result(
            OtelSpanBuilder::new(format!("service-b-{test_id}"))
                .span_type(OtelSpanType::Generation)
                .model("service-b-model")
                .user_id(test_user_id.clone())
                .session_id(test_session_id.clone())
                .system("service-b")
                .operation_name("process_with_failure")
                .build(),
            &carrier_b,
            async move {
                tracing::info!("Service B processing (will fail)");
                tokio::time::sleep(Duration::from_millis(100)).await;
                Err::<String, _>(ServiceBError(
                    "Critical failure in downstream service".to_string(),
                ))
            },
        )
        .await;

    assert!(service_b_result.is_err());
    let error = service_b_result.unwrap_err();
    assert!(error
        .to_string()
        .contains("Critical failure in downstream service"));

    tracing::info!(
        service_a_result = ?service_a_result.unwrap(),
        service_b_error = %error,
        "Error propagation test completed - Service A succeeded, Service B failed as expected"
    );

    drop(parent_span);

    cleanup_integration_test().await;
    Ok(())
}
