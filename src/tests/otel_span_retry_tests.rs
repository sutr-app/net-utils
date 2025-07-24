//! Retry workflow integration tests for OpenTelemetry tracing
//! These tests demonstrate how to implement proper span hierarchy for retry operations
use super::test_utils::*;
use crate::trace::{
    attr::{OtelSpanBuilder, OtelSpanType},
    impls::GenericOtelClient,
    otel_span::*,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Test implementation of a retryable workflow with proper parent-child span relationship
/// This ensures that all retry attempts appear grouped under the same parent operation
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_retry_workflow_with_parent_child_spans() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the Otel client
    let client = Arc::new(setup_integration_test().await?);

    // Define common identifiers for tracing
    let user_id = "test-user-123";
    let session_id = "test-session-456";

    // Define a custom error type for our test
    #[derive(Debug)]
    struct RetryableError {
        message: String,
        attempt: u32,
    }

    impl std::fmt::Display for RetryableError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "Operation failed on attempt {}: {}",
                self.attempt, self.message
            )
        }
    }

    impl std::error::Error for RetryableError {}

    // Create a parent span for the entire retry operation
    let parent_attributes = OtelSpanBuilder::new("llm-generation-with-retry")
        .span_type(OtelSpanType::Span)
        .user_id(user_id)
        .session_id(session_id)
        .tags(vec!["retry-workflow".to_string()])
        .input(json!({
            "operation": "text-generation",
            "max_retries": 3
        }))
        .build(); // Execute the retry workflow with proper parent-child span hierarchy
    client
        .clone()
        .with_span_result(parent_attributes, None, async move {
            // Get the parent span context - in a real application, you would store this
            // or pass it to the retry function to establish the relationship
            let max_retries = 3;
            let retry_count = Arc::new(Mutex::new(0));

            tracing::info!("Starting LLM generation with retry logic");

            loop {
                // Check retry count
                {
                    let current_count = *retry_count.lock().await;
                    if current_count >= max_retries {
                        break;
                    }
                }

                // Prepare attempt information
                let attempt_num = {
                    let current = *retry_count.lock().await;
                    current + 1
                };

                // Create a child span for this specific attempt
                let attempt_attributes =
                    OtelSpanBuilder::new(format!("generation-attempt-{attempt_num}"))
                        .span_type(OtelSpanType::Generation)
                        .model("gpt-4")
                        .user_id(user_id)
                        .session_id(session_id)
                        .input(json!({
                            "prompt": "Generate a response",
                            "attempt": attempt_num,
                            "max_retries": max_retries
                        }))
                        .build();

                // Clone counter for the closure
                let count_clone = retry_count.clone();

                // Execute the attempt as a child span
                let result: Result<serde_json::Value, RetryableError> = client
                    .with_span_result(attempt_attributes, None, async move {
                        // Simulate some work
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                        let current_attempt = *count_clone.lock().await;
                        // Simulate failure on first two attempts
                        if current_attempt < 2 {
                            Err(RetryableError {
                                message: "Rate limit exceeded".to_string(),
                                attempt: current_attempt as u32,
                            })
                        } else {
                            Ok(json!({
                                "content": "Successfully generated response after retries",
                                "attempt": current_attempt,
                            }))
                        }
                    })
                    .await;

                match result {
                    Ok(response) => {
                        // Success - log the final result
                        let final_count = *retry_count.lock().await;

                        // Create a child span for the success event
                        let success_attributes = OtelSpanBuilder::new("generation-success")
                            .span_type(OtelSpanType::Event)
                            .level("INFO")
                            .user_id(user_id)
                            .session_id(session_id)
                            .input(json!({
                                "final_attempt": final_count + 1,
                                "response": response
                            }))
                            .build();

                        client
                            .with_span_result(success_attributes, None, async move {
                                tracing::info!(
                                    attempt = final_count + 1,
                                    "LLM generation succeeded after retries"
                                );
                                Ok::<(), std::io::Error>(())
                            })
                            .await
                            .unwrap_or_else(|e| {
                                tracing::error!("Error in success span: {}", e);
                            });

                        break;
                    }
                    Err(_) => {
                        // Increment retry counter
                        {
                            let mut lock = retry_count.lock().await;
                            *lock += 1;
                        }

                        let current_retry = {
                            let lock = retry_count.lock().await;
                            *lock
                        };

                        if current_retry < max_retries {
                            // Log retry event as a child span
                            let retry_attributes =
                                OtelSpanBuilder::new(format!("retry-{current_retry}"))
                                    .span_type(OtelSpanType::Event)
                                    .level("WARNING")
                                    .user_id(user_id)
                                    .session_id(session_id)
                                    .input(json!({
                                        "retry_count": current_retry,
                                        "max_retries": max_retries,
                                        "delay_ms": 500
                                    }))
                                    .build();

                            client
                                .with_span_result(retry_attributes, None, async move {
                                    tracing::warn!(
                                        retry_count = current_retry,
                                        max_retries = max_retries,
                                        "Retrying LLM generation after error"
                                    );
                                    Ok::<(), std::io::Error>(())
                                })
                                .await
                                .unwrap_or_else(|e| {
                                    tracing::error!("Error in retry span: {}", e);
                                });

                            // Wait before retry
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        } else {
                            // Max retries reached, log final error
                            let final_error = RetryableError {
                                message: "Max retries exceeded".to_string(),
                                attempt: current_retry as u32,
                            };

                            let error_attributes = OtelSpanBuilder::new("generation-failed")
                                .span_type(OtelSpanType::Event)
                                .level("ERROR")
                                .user_id(user_id)
                                .session_id(session_id)
                                .input(json!({
                                    "final_attempt": current_retry,
                                    "error": final_error.to_string()
                                }))
                                .build();

                            client
                                .with_span_result(error_attributes, None, async move {
                                    tracing::error!(
                                        error = %final_error,
                                        "LLM generation failed after all retry attempts"
                                    );
                                    Ok::<(), std::io::Error>(())
                                })
                                .await
                                .unwrap_or_else(|e| {
                                    tracing::error!("Error in error span: {}", e);
                                });
                        }
                    }
                }
            }

            tracing::info!("Retry workflow complete");
            Ok::<(), std::io::Error>(())
        })
        .await
        .unwrap();

    cleanup_integration_test().await;
    Ok(())
}

/// Example of implementing a reusable retry utility function with proper span hierarchy
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_reusable_retry_function() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(setup_integration_test().await?);

    // Define common identifiers for tracing
    let user_id = "test-user-123";
    let session_id = "test-session-456";

    // Define a helper retry function
    #[allow(clippy::too_many_arguments)]
    async fn with_retry<F, Fut, T, E>(
        client: Arc<GenericOtelClient>,
        operation_name: String,
        max_retries: u32,
        retry_delay_ms: u64,
        model: Option<String>,
        input: serde_json::Value,
        user_id: Option<String>,
        session_id: Option<String>,
        operation_fn: F,
    ) -> Result<T, E>
    where
        F: Fn(u32) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<T, E>> + Send + 'static,
        T: Send + Clone + serde::Serialize + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        // Create parent span for entire retry operation
        let mut parent_attributes = OtelSpanBuilder::new(format!("{operation_name}-with-retry"))
            .span_type(OtelSpanType::Span)
            .tags(vec!["retry".to_string()])
            .input(json!({
                "operation": operation_name,
                "max_retries": max_retries,
                "retry_delay_ms": retry_delay_ms
            }))
            .build();

        if let Some(model_name) = &model {
            parent_attributes = OtelSpanBuilder::from_attributes(parent_attributes)
                .model(model_name.clone())
                .build();
        }

        // Add user and session IDs if provided
        if let Some(uid) = &user_id {
            parent_attributes = OtelSpanBuilder::from_attributes(parent_attributes)
                .user_id(uid.clone())
                .build();
        }

        if let Some(sid) = &session_id {
            parent_attributes = OtelSpanBuilder::from_attributes(parent_attributes)
                .session_id(sid.clone())
                .build();
        }

        // Start parent span and execute retry logic
        client
            .clone()
            .with_span_result(parent_attributes, None, async move {
                let mut attempt = 0;
                let mut last_error: Option<E> = None;

                while attempt < max_retries {
                    let mut attempt_attributes =
                        OtelSpanBuilder::new(format!("{}-attempt-{}", operation_name, attempt + 1))
                            .span_type(OtelSpanType::Generation)
                            .input(json!({
                                "attempt": attempt + 1,
                                "max_retries": max_retries,
                                "operation_input": input
                            }))
                            .build();

                    // Add model if available
                    if let Some(model_name) = &model {
                        attempt_attributes = OtelSpanBuilder::from_attributes(attempt_attributes)
                            .model(model_name.clone())
                            .build();
                    }

                    // Clone operation function for this attempt
                    let op_fn = operation_fn.clone();
                    let current_attempt = attempt;

                    // Execute attempt as child span
                    let result = client
                        .with_span_result(attempt_attributes, None, async move {
                            op_fn(current_attempt).await
                        })
                        .await;

                    match result {
                        Ok(value) => {
                            // Success! Record the successful attempt
                            let mut success_attributes =
                                OtelSpanBuilder::new(format!("{operation_name}-success"))
                                    .span_type(OtelSpanType::Event)
                                    .level("INFO")
                                    .input(json!({
                                        "final_attempt": attempt + 1,
                                        "success": true
                                    }))
                                    .build();

                            // Add user and session IDs if provided
                            if let Some(uid) = &user_id {
                                success_attributes =
                                    OtelSpanBuilder::from_attributes(success_attributes)
                                        .user_id(uid.clone())
                                        .build();
                            }

                            if let Some(sid) = &session_id {
                                success_attributes =
                                    OtelSpanBuilder::from_attributes(success_attributes)
                                        .session_id(sid.clone())
                                        .build();
                            }

                            let _success_result = client
                                .with_span_result(success_attributes, None, async move {
                                    tracing::info!(
                                        operation = %operation_name,
                                        final_attempt = attempt + 1,
                                        "Operation succeeded after retries"
                                    );
                                    Ok::<(), std::io::Error>(())
                                })
                                .await;

                            return Ok(value);
                        }
                        Err(err) => {
                            // Store error for potential final error message
                            last_error = Some(err);

                            // Increment attempt counter
                            attempt += 1;

                            if attempt < max_retries {
                                // Record retry event
                                let mut retry_attributes = OtelSpanBuilder::new(format!(
                                    "{operation_name}-retry-{attempt}"
                                ))
                                .span_type(OtelSpanType::Event)
                                .level("WARNING")
                                .input(json!({
                                    "attempt": attempt,
                                    "next_attempt": attempt + 1,
                                    "delay_ms": retry_delay_ms
                                }))
                                .build();

                                // Add user and session IDs if provided
                                if let Some(uid) = &user_id {
                                    retry_attributes =
                                        OtelSpanBuilder::from_attributes(retry_attributes)
                                            .user_id(uid.clone())
                                            .build();
                                }

                                if let Some(sid) = &session_id {
                                    retry_attributes =
                                        OtelSpanBuilder::from_attributes(retry_attributes)
                                            .session_id(sid.clone())
                                            .build();
                                }

                                let opname = operation_name.clone();
                                let _retry_result = client
                                    .with_span_result(retry_attributes, None, async move {
                                        tracing::warn!(
                                            operation = %opname,
                                            attempt = attempt,
                                            max_retries = max_retries,
                                            "Operation failed, retrying"
                                        );
                                        Ok::<(), std::io::Error>(())
                                    })
                                    .await;

                                // Wait before retry
                                tokio::time::sleep(std::time::Duration::from_millis(
                                    retry_delay_ms,
                                ))
                                .await;
                            }
                        }
                    }
                }

                // If we get here, all retries failed
                let mut failure_attributes =
                    OtelSpanBuilder::new(format!("{operation_name}-failed"))
                        .span_type(OtelSpanType::Event)
                        .level("ERROR")
                        .input(json!({
                            "max_attempts_reached": true,
                            "attempts": max_retries
                        }))
                        .build();

                // Add user and session IDs if provided
                if let Some(uid) = &user_id {
                    failure_attributes = OtelSpanBuilder::from_attributes(failure_attributes)
                        .user_id(uid.clone())
                        .build();
                }

                if let Some(sid) = &session_id {
                    failure_attributes = OtelSpanBuilder::from_attributes(failure_attributes)
                        .session_id(sid.clone())
                        .build();
                }

                let _failure_result = client
                    .clone()
                    .with_span_result(failure_attributes, None, async move {
                        tracing::error!(
                            operation = %operation_name,
                            max_retries = max_retries,
                            "Operation failed after all retry attempts"
                        );
                        Ok::<(), std::io::Error>(())
                    })
                    .await;

                // Return last error
                Err(last_error.unwrap())
            })
            .await
    }

    // Now use our reusable retry function
    let result = with_retry(
        client.clone(),
        "test-llm-generation".to_string(),
        3,
        200,
        Some("gpt-4".to_string()),
        json!({"prompt": "Test prompt"}),
        Some(user_id.to_string()),
        Some(session_id.to_string()),
        |attempt| async move {
            // Simulate API call that succeeds on the third attempt
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            if attempt < 2 {
                Err(std::io::Error::other(format!(
                    "API rate limit exceeded on attempt {}",
                    attempt + 1
                )))
            } else {
                Ok(json!({"result": "Success on attempt 3"}).to_string())
            }
        },
    )
    .await;

    assert!(result.is_ok());
    tracing::info!("Retry function test completed successfully");

    cleanup_integration_test().await;
    Ok(())
}

/// Test to demonstrate a real-world example with a more structured retry approach
#[tokio::test]
#[ignore = "Integration test requiring OTLP endpoint - run with --ignored"]
async fn test_structured_retry_workflow() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(setup_integration_test().await?);

    // Define common identifiers for tracing
    let user_id = "test-user-123";
    let session_id = "test-session-456";

    /// Create a structured retry workflow to group related spans
    async fn execute_with_retry_workflow<F, Fut, T, E>(
        client: Arc<GenericOtelClient>,
        workflow_name: impl Into<String>,
        max_retries: u32,
        user_id: Option<&str>,
        session_id: Option<&str>,
        operation_fn: F,
    ) -> Result<T, E>
    where
        F: Fn(u32, String) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<T, E>> + Send + 'static,
        T: Send + Clone + serde::Serialize + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        use uuid::Uuid;

        // Generate a unique ID for this workflow to link all spans together
        let workflow_id = Uuid::new_v4().to_string();
        let workflow_name = workflow_name.into();

        // Create parent span for the entire workflow
        let mut parent_attributes = OtelSpanBuilder::new(workflow_name.to_string())
            .span_type(OtelSpanType::Span)
            .tags(vec!["workflow".to_string(), "retry".to_string()])
            .input(json!({
                "workflow": workflow_name,
                "max_retries": max_retries
            }))
            .build();

        // Add user and session IDs if provided
        if let Some(uid) = user_id {
            parent_attributes = OtelSpanBuilder::from_attributes(parent_attributes)
                .user_id(uid)
                .build();
        }

        if let Some(sid) = session_id {
            parent_attributes = OtelSpanBuilder::from_attributes(parent_attributes)
                .session_id(sid)
                .build();
        }

        let client_clone = client.clone();
        let user_id_clone = user_id.map(|s| s.to_string());
        let session_id_clone = session_id.map(|s| s.to_string());
        client
            .clone()
            .with_span_result(parent_attributes, None, async move {
                let mut retry_count = 0;
                let mut last_error: Option<E> = None;

                while retry_count < max_retries {
                    // Create attempt span with parent relationship
                    let mut attempt_attributes = OtelSpanBuilder::new(format!(
                        "{}-attempt-{}",
                        workflow_name,
                        retry_count + 1
                    ))
                    .span_type(OtelSpanType::Generation)
                    .input(json!({
                        "attempt": retry_count + 1,
                        "max_retries": max_retries
                    }))
                    .build();

                    // Add user and session IDs if provided
                    if let Some(uid) = user_id_clone.clone() {
                        attempt_attributes = OtelSpanBuilder::from_attributes(attempt_attributes)
                            .user_id(uid)
                            .build();
                    }

                    if let Some(sid) = session_id_clone.clone() {
                        attempt_attributes = OtelSpanBuilder::from_attributes(attempt_attributes)
                            .session_id(sid)
                            .build();
                    }

                    let client_clone_l = client_clone.clone();
                    // Execute the attempt with its own span
                    let op_fn = operation_fn.clone();
                    let current_attempt = retry_count;
                    let wid = workflow_id.clone();
                    let attempt_result = client_clone_l
                        .with_span_result(attempt_attributes, None, async move {
                            op_fn(current_attempt, wid).await
                        })
                        .await;

                    match attempt_result {
                        Ok(result) => {
                            // Success - log event and return result
                            let mut success_event =
                                OtelSpanBuilder::new(format!("{workflow_name}-success"))
                                    .span_type(OtelSpanType::Event)
                                    .level("INFO")
                                    .input(json!({
                                        "workflow": workflow_name,
                                        "successful_attempt": retry_count + 1
                                    }))
                                    .build();

                            // Add user and session IDs if provided
                            if let Some(uid) = user_id_clone {
                                success_event = OtelSpanBuilder::from_attributes(success_event)
                                    .user_id(uid)
                                    .build();
                            }

                            if let Some(sid) = session_id_clone {
                                success_event = OtelSpanBuilder::from_attributes(success_event)
                                    .session_id(sid)
                                    .build();
                            }

                            client_clone_l
                                .with_span_result(success_event, None, async move {
                                    tracing::info!(
                                        workflow = %workflow_name,
                                        attempt = retry_count + 1,
                                        "Workflow completed successfully"
                                    );
                                    Ok::<(), std::io::Error>(())
                                })
                                .await
                                .unwrap();

                            return Ok(result);
                        }
                        Err(error) => {
                            // Store error for potential final error report
                            last_error = Some(error);

                            retry_count += 1;

                            if retry_count < max_retries {
                                // Create retry event span
                                let retry_event = OtelSpanBuilder::new(format!(
                                    "{workflow_name}-retry-{retry_count}"
                                ))
                                .span_type(OtelSpanType::Event)
                                .level("WARNING")
                                .input(json!({
                                    "workflow": workflow_name,
                                    "retry_count": retry_count,
                                    "max_retries": max_retries
                                }))
                                .build();

                                let wname = workflow_name.clone();
                                client_clone_l
                                    .with_span_result(retry_event, None, async move {
                                        tracing::warn!(
                                            workflow = %wname,
                                            retry = retry_count,
                                            "Retrying failed operation"
                                        );
                                        Ok::<(), std::io::Error>(())
                                    })
                                    .await
                                    .unwrap();

                                // Add delay between retries with backoff
                                let delay = 200 * (2_u64.pow(retry_count - 1));
                                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                            }
                        }
                    }
                }

                // If we get here, all retries failed
                let failure_event = OtelSpanBuilder::new(format!("{workflow_name}-failed"))
                    .span_type(OtelSpanType::Event)
                    .level("ERROR")
                    .input(json!({
                        "workflow": workflow_name,
                        "max_retries_exceeded": true,
                        "attempts": max_retries
                    }))
                    .build();

                let wname = workflow_name.clone();
                client_clone
                    .with_span_result(failure_event, None, async move {
                        tracing::error!(
                            workflow = %wname,
                            max_retries = max_retries,
                            "Workflow failed after exhausting all retry attempts"
                        );
                        Ok::<(), std::io::Error>(())
                    })
                    .await
                    .unwrap();

                match last_error {
                    Some(err) => Err(err),
                    None => panic!("Unknown error during workflow execution"),
                }
            })
            .await
    }

    let client_clone = client.clone();
    // Now use our structured retry workflow
    let result = execute_with_retry_workflow(
        client_clone.clone(),
        "llm-chat-completion",
        3,
        Some(user_id),
        Some(session_id),
        move |attempt, _workflow_id| {
            let client_clone2 = client_clone.clone();
            async move {
                // Simulate an operation that fails on first two attempts
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                // Create a child span for a sub-operation, still linked to workflow
                let child_attributes =
                    OtelSpanBuilder::new(format!("tokenize-input-{}", attempt + 1))
                        .span_type(OtelSpanType::Span)
                        .input(json!({"sub_operation": "tokenize", "attempt": attempt + 1}))
                        .build();

                client_clone2
                    .with_span_result(child_attributes, None, async {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        // Some sub-operation work here
                        Ok::<(), std::io::Error>(())
                    })
                    .await
                    .unwrap();

                if attempt < 2 {
                    Err(std::io::Error::other(format!(
                        "Service temporarily unavailable (attempt {})",
                        attempt + 1
                    )))
                } else {
                    Ok("Chat completion successful".to_string())
                }
            }
        },
    )
    .await;

    assert!(result.is_ok());
    tracing::info!("Structured retry workflow test completed successfully");

    cleanup_integration_test().await;
    Ok(())
}
