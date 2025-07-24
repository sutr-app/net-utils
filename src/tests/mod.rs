#[cfg(test)]
mod otel_span_basic_tests;
#[cfg(test)]
mod otel_span_error_tests;
#[cfg(test)]
mod otel_span_retry_tests;
#[cfg(test)]
mod remote_span_tests;

/// Integration test utilities for OpenTelemetry tests
#[cfg(test)]
pub mod test_utils {
    use crate::trace::impls::GenericOtelClient;

    /// Common setup for integration tests that require OTLP endpoints
    pub async fn setup_integration_test() -> Result<GenericOtelClient, Box<dyn std::error::Error>> {
        // Set up environment variables for testing
        // We're explicitly setting the OTLP endpoint for the test
        let otlp_addr = "http://otel-collector.default.svc.cluster.local:4317".to_string();
        // Force override any existing configuration
        std::env::set_var("OTLP_ADDR", &otlp_addr);

        println!("Running integration test with OTLP endpoint: {otlp_addr}");

        // Initialize tracing with OTLP exporter using command_utils
        let logging_config = command_utils::util::tracing::LoggingConfig {
            app_name: Some("otel-span-integration-test".to_string()),
            level: Some("DEBUG".to_string()), // Use DEBUG to see more OTEL logs
            file_name: None,
            file_dir: None,
            use_json: false,
            use_stdout: true,
        };

        // Force cleanup of any previous tracer provider
        command_utils::util::tracing::shutdown_tracer_provider();

        // Initialize tracer with our settings
        command_utils::util::tracing::tracing_init(logging_config)
            .await
            .unwrap();

        println!("OpenTelemetry tracer initialized for OTLP endpoint: {otlp_addr}");

        // Create and return client
        Ok(GenericOtelClient::new("otel-span-integration-test"))
    }

    /// Common cleanup for integration tests
    pub async fn cleanup_integration_test() {
        println!("Waiting for spans to be exported...");

        // Allow more time for OTLP export - increase the wait time
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Shutdown tracer providers to ensure data is flushed
        // This is critical to ensure all data is sent
        println!("Shutting down tracer provider...");
        command_utils::util::tracing::shutdown_tracer_provider();

        // Additional wait to ensure export completes after shutdown
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        println!("Integration test completed successfully");
        println!("Check your OTLP endpoint/Langfuse dashboard for the test spans");
    }
}
