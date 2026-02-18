use super::GrpcConnection;
use anyhow::{Context, Result};
use command_utils::protobuf::ProtobufDescriptor;
use futures::stream::StreamExt;
use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, MessageDescriptor, prost_types::FileDescriptorProto,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tonic::transport::{Channel, Endpoint};
use tonic_reflection::pb::v1::{
    ServerReflectionRequest, server_reflection_client::ServerReflectionClient,
    server_reflection_request::MessageRequest, server_reflection_response::MessageResponse,
};

// Descriptor cache to avoid repeated resolution of the same symbols and files
#[derive(Debug, Default)]
struct DescriptorCache {
    // Cache of processed symbols - symbols we've already seen and handled
    processed_symbols: HashSet<String>,
    // Cache of processed files - file descriptors we've already processed
    processed_files: HashSet<String>,
    // Cache of resolved file descriptors - to avoid re-fetching
    // Using HashMap for O(1) lookup by file name
    file_descriptors: HashMap<String, FileDescriptorProto>,
}

// We'll be working with descriptors via references/handles - no direct dependency on prost_reflect

#[derive(Debug, Clone)]
pub struct GrpcReflectionClient {
    pub address: String,
    pub request_timeout: Option<Duration>,
    connection: GrpcConnection,
    // Cache for descriptor resolution - wrapped in Arc<RwLock> for interior mutability and thread safety
    descriptor_cache: Arc<RwLock<DescriptorCache>>,
}

impl GrpcReflectionClient {
    pub async fn new(addr: String, request_timeout: Option<Duration>) -> Result<Self> {
        let con = GrpcConnection::new(addr.clone(), request_timeout).await?;
        Ok(Self {
            address: addr,
            request_timeout,
            connection: con,
            descriptor_cache: Arc::new(RwLock::new(DescriptorCache::default())),
        })
    }
    pub async fn connect(
        endpoint: Endpoint,
        channel: Channel,
        request_timeout: Option<Duration>,
    ) -> Result<Self> {
        let address = endpoint.uri().to_string();
        let con = GrpcConnection::create(endpoint, channel).await?;
        Ok(Self {
            address,
            request_timeout,
            connection: con,
            descriptor_cache: Arc::new(RwLock::new(DescriptorCache::default())),
        })
    }
    pub async fn init_grpc_connection(&self) -> Result<()> {
        // TODO create new conection only when connection test failed
        self.connection.reconnect().await
    }
    pub async fn reflection_client(&self) -> ServerReflectionClient<tonic::transport::Channel> {
        let cell = self.connection.read_channel().await;
        ServerReflectionClient::new(cell.clone())
    }

    /// Get a list of services from the server using the reflection API
    pub async fn list_services(&self) -> Result<Vec<String>> {
        let mut client = self.reflection_client().await;

        // Create a reflection request to list services
        let list_services_request = ServerReflectionRequest {
            host: "".to_string(),
            message_request: Some(MessageRequest::ListServices("".to_string())),
        };

        // Send the request and receive the response
        let mut stream = client
            .server_reflection_info(tonic::Request::new(futures::stream::iter(vec![
                list_services_request,
            ])))
            .await
            .context("Failed to call server_reflection_info")?
            .into_inner();

        // Process the response
        if let Some(response_result) = stream.next().await {
            let response = response_result?;
            if let Some(MessageResponse::ListServicesResponse(list_response)) =
                response.message_response
            {
                let service_names = list_response.service.into_iter().map(|s| s.name).collect();
                return Ok(service_names);
            }
        }

        Err(anyhow::anyhow!(
            "Failed to get service list from reflection API"
        ))
    }

    /// Get message descriptor for a specific message by its full name
    pub async fn get_message_descriptor(&self, message_name: &str) -> Result<MessageDescriptor> {
        // Use the same dependency resolution mechanism as get_service_with_dependencies
        // This ensures all required dependencies are properly resolved
        let pool = self.get_message_with_dependencies(message_name).await?;

        // Get the message descriptor from the pool
        pool.get_message_by_name(message_name).ok_or_else(|| {
            anyhow::anyhow!("Message not found in descriptor pool: {}", message_name)
        })
    }

    /// Helper method to get a message descriptor with all its dependencies resolved
    async fn get_message_with_dependencies(&self, message_name: &str) -> Result<DescriptorPool> {
        let mut client = self.reflection_client().await;
        let mut processed_symbols = std::collections::HashSet::new();
        let mut processed_files = std::collections::HashSet::new();

        // Collect all file descriptors first before adding to the pool
        let mut file_descriptors = Vec::new();

        // Start with the message name and process all related symbols
        self.fetch_file_descriptors_recursive(
            &mut client,
            message_name,
            &mut file_descriptors,
            &mut processed_symbols,
            &mut processed_files,
        )
        .await?;

        // Now create a pool and add all collected descriptors
        let mut pool = DescriptorPool::new();
        tracing::debug!(
            "Adding {} file descriptors to pool for message {}",
            file_descriptors.len(),
            message_name
        );

        // Add all collected file descriptors to the pool
        pool.add_file_descriptor_protos(file_descriptors)
            .context("Failed to add file descriptors to pool")?;

        Ok(pool)
    }

    /// Get all file descriptors for a service and its dependencies
    pub async fn get_service_with_dependencies(
        &self,
        service_name: &str,
    ) -> Result<DescriptorPool> {
        let mut client = self.reflection_client().await;
        let mut processed_symbols = std::collections::HashSet::new();
        let mut processed_files = std::collections::HashSet::new();

        // Collect all file descriptors first before adding to the pool
        let mut file_descriptors = Vec::new();

        // Start with the service name and process all related symbols
        self.fetch_file_descriptors_recursive(
            &mut client,
            service_name,
            &mut file_descriptors,
            &mut processed_symbols,
            &mut processed_files,
        )
        .await?;

        // Now create a pool and add all collected descriptors
        let mut pool = DescriptorPool::new();
        tracing::debug!("Adding {} file descriptors to pool", file_descriptors.len());

        // Add all collected file descriptors to the pool
        if let Err(e) = pool.add_file_descriptor_protos(file_descriptors.clone()) {
            tracing::error!("Failed to add file descriptors to pool: {}", e);
            tracing::debug!("Collected {} file descriptors:", file_descriptors.len());
            for (i, desc) in file_descriptors.iter().enumerate() {
                let unnamed = "<unnamed>".to_string();
                let name = desc.name.as_ref().unwrap_or(&unnamed);
                tracing::debug!("  {}: {} (deps: {:?})", i, name, desc.dependency);
            }
            return Err(e).context("Failed to add file descriptors to pool");
        }

        Ok(pool)
    }

    fn fetch_file_descriptors_recursive<'a>(
        &'a self,
        client: &'a mut ServerReflectionClient<tonic::transport::Channel>,
        symbol: &'a str,
        file_descriptors: &'a mut Vec<FileDescriptorProto>,
        processed_symbols: &'a mut std::collections::HashSet<String>,
        processed_files: &'a mut std::collections::HashSet<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // First check both local tracking set and global cache for this symbol
            if processed_symbols.contains(symbol) {
                return Ok(());
            }

            // Check in global cache
            {
                let cache = self.descriptor_cache.read().unwrap();
                if cache.processed_symbols.contains(symbol) {
                    // If the symbol is already in the global cache, add all cached file descriptors to our collection
                    // This is much more efficient than selectively checking each one
                    for (name, descriptor) in &cache.file_descriptors {
                        if !processed_files.contains(name) {
                            // Add this cached descriptor to our local collection
                            file_descriptors.push(descriptor.clone());
                            processed_files.insert(name.clone());
                        }
                    }
                    tracing::debug!(
                        "Cache hit for symbol: {}, added {} file descriptors from cache",
                        symbol,
                        cache.file_descriptors.len()
                    );
                    return Ok(());
                }
            }

            // Mark this symbol as processed in our local tracking
            processed_symbols.insert(symbol.to_string());

            // Request the file descriptor containing this symbol
            let symbol_request = ServerReflectionRequest {
                host: "".to_string(),
                message_request: Some(MessageRequest::FileContainingSymbol(symbol.to_string())),
            };

            let mut stream = client
                .server_reflection_info(tonic::Request::new(futures::stream::iter(vec![
                    symbol_request,
                ])))
                .await
                .context("Failed to call server_reflection_info for recursive descriptor")?
                .into_inner();

            if let Some(response_result) = stream.next().await {
                let response = response_result?;
                if let Some(MessageResponse::FileDescriptorResponse(file_response)) =
                    response.message_response
                {
                    // Collect new file descriptors and their dependencies
                    let mut new_dependencies = Vec::new();

                    for file_descriptor_bytes in file_response.file_descriptor_proto {
                        let file_descriptor =
                            FileDescriptorProto::decode(file_descriptor_bytes.as_slice())
                                .context("Failed to decode file descriptor")?;

                        // If we've already processed this file, skip it
                        if let Some(name) = &file_descriptor.name {
                            if processed_files.contains(name) {
                                continue;
                            }
                            processed_files.insert(name.clone());

                            // Collect all dependencies that need to be fetched
                            for dependency in &file_descriptor.dependency {
                                if !processed_files.contains(dependency) {
                                    new_dependencies.push(dependency.clone());
                                }
                            }
                        }

                        // Add file descriptor to our collection
                        file_descriptors.push(file_descriptor);
                    }

                    // Resolve all dependencies iteratively until no new dependencies are found
                    self.resolve_dependencies_iteratively(
                        client,
                        new_dependencies,
                        file_descriptors,
                        processed_files,
                    )
                    .await?;

                    // Extract and process additional symbols from temporary pool
                    let new_symbols =
                        self.extract_additional_symbols(file_descriptors, processed_symbols)?;

                    // Recursively process all new symbols
                    for symbol in new_symbols {
                        if !processed_symbols.contains(&symbol) {
                            self.fetch_file_descriptors_recursive(
                                client,
                                &symbol,
                                file_descriptors,
                                processed_symbols,
                                processed_files,
                            )
                            .await?;
                        }
                    }
                } else {
                    return Err(anyhow::anyhow!(
                        "Unexpected response type from reflection API"
                    ));
                }
            }

            // Update the global cache with new symbols and file descriptors
            {
                let mut cache = self.descriptor_cache.write().unwrap();
                // Add the symbol we just processed to the global cache
                cache.processed_symbols.insert(symbol.to_string());

                // Add all new file descriptors to the global cache
                for descriptor in file_descriptors.iter() {
                    if let Some(name) = &descriptor.name
                        && !cache.processed_files.contains(name)
                    {
                        cache.processed_files.insert(name.clone());
                        cache
                            .file_descriptors
                            .insert(name.clone(), descriptor.clone());
                    }
                }
            }

            Ok(())
        })
    }

    /// Helper method to fetch a file descriptor by name
    /// This method only retrieves the FileDescriptorProto objects and doesn't modify any pools
    /// It's the caller's responsibility to handle dependencies and add descriptors to pools
    async fn fetch_file_by_name(
        &self,
        client: &mut ServerReflectionClient<tonic::transport::Channel>,
        file_name: &str,
    ) -> Result<Vec<FileDescriptorProto>> {
        // Check the global cache first for this file
        {
            let cache = self.descriptor_cache.read().unwrap();
            if let Some(descriptor) = cache.file_descriptors.get(file_name) {
                tracing::debug!("Cache hit for file: {}", file_name);
                return Ok(vec![descriptor.clone()]);
            }
        }

        tracing::debug!("Fetching file by name: {}", file_name);

        // Request the file descriptor by name
        let file_request = ServerReflectionRequest {
            host: "".to_string(),
            message_request: Some(MessageRequest::FileByFilename(file_name.to_string())),
        };

        let mut stream = client
            .server_reflection_info(tonic::Request::new(futures::stream::iter(vec![
                file_request,
            ])))
            .await
            .context(format!("Failed to fetch file descriptor for {file_name}"))?
            .into_inner();

        match stream.next().await {
            Some(response_result) => {
                let response = response_result?;
                if let Some(MessageResponse::FileDescriptorResponse(file_response)) =
                    response.message_response
                {
                    // Collect and return decoded FileDescriptorProto objects
                    let descriptors = file_response
                        .file_descriptor_proto
                        .iter()
                        .map(|bytes| {
                            FileDescriptorProto::decode(bytes.as_slice())
                                .context("Failed to decode file descriptor")
                        })
                        .collect::<Result<Vec<_>>>()?;

                    // Add the file descriptors to the global cache for future use
                    {
                        let mut cache = self.descriptor_cache.write().unwrap();
                        for descriptor in &descriptors {
                            if let Some(name) = &descriptor.name
                                && !cache.processed_files.contains(name)
                            {
                                cache.processed_files.insert(name.clone());
                                cache
                                    .file_descriptors
                                    .insert(name.clone(), descriptor.clone());
                                tracing::debug!("Added file to cache: {}", name);
                            }
                        }
                    }

                    Ok(descriptors)
                } else if let Some(MessageResponse::ErrorResponse(error)) =
                    response.message_response
                {
                    tracing::error!(
                        "Server returned error for file '{}': {} (code: {})",
                        file_name,
                        error.error_message,
                        error.error_code
                    );
                    Err(anyhow::anyhow!(
                        "Error response from server when fetching {}: {} (code: {})",
                        file_name,
                        error.error_message,
                        error.error_code
                    ))
                } else {
                    Err(anyhow::anyhow!(
                        "Unexpected response type from reflection API"
                    ))
                }
            }
            _ => {
                tracing::error!("No response from reflection API for file '{}'", file_name);
                Err(anyhow::anyhow!(
                    "No response from server when fetching {}",
                    file_name
                ))
            }
        }
    }

    /// Iteratively resolve dependencies until all are fetched
    async fn resolve_dependencies_iteratively(
        &self,
        client: &mut ServerReflectionClient<tonic::transport::Channel>,
        initial_dependencies: Vec<String>,
        file_descriptors: &mut Vec<FileDescriptorProto>,
        processed_files: &mut std::collections::HashSet<String>,
    ) -> Result<()> {
        let mut dependencies_to_process = initial_dependencies;
        let mut iteration = 0;
        const MAX_ITERATIONS: usize = 30; // Prevent infinite loops

        while !dependencies_to_process.is_empty() && iteration < MAX_ITERATIONS {
            iteration += 1;
            let mut next_round_dependencies = Vec::new();

            for dependency in dependencies_to_process {
                // Skip if already processed
                if processed_files.contains(&dependency) {
                    continue;
                }

                // Fetch the file descriptor for this dependency
                let descriptors = self.fetch_file_by_name(client, &dependency).await?;
                processed_files.insert(dependency.clone());

                // Add all descriptors to our collection and collect their dependencies
                for descriptor in descriptors {
                    // Add the descriptor to our collection first
                    file_descriptors.push(descriptor.clone());

                    // Collect dependencies from this descriptor for the next iteration
                    for nested_dep in &descriptor.dependency {
                        if !processed_files.contains(nested_dep)
                            && !next_round_dependencies.contains(nested_dep)
                        {
                            next_round_dependencies.push(nested_dep.clone());
                        }
                    }
                }
            }

            dependencies_to_process = next_round_dependencies;
        }

        if iteration >= MAX_ITERATIONS && !dependencies_to_process.is_empty() {
            tracing::warn!(
                "Reached maximum iteration limit ({}) with {} unresolved dependencies: {:?}",
                MAX_ITERATIONS,
                dependencies_to_process.len(),
                dependencies_to_process
            );
        }

        Ok(())
    }

    /// Extract additional symbols from a temporary descriptor pool
    fn extract_additional_symbols(
        &self,
        file_descriptors: &[FileDescriptorProto],
        processed_symbols: &std::collections::HashSet<String>,
    ) -> Result<Vec<String>> {
        let mut new_symbols = Vec::new();

        // Create a temporary pool to scan for additional symbols
        let mut temp_pool = DescriptorPool::new();

        // Try to add all descriptors we've collected so far to the temporary pool
        // Errors are expected here since we might not have all dependencies yet
        if temp_pool
            .add_file_descriptor_protos(file_descriptors.to_vec())
            .is_ok()
        {
            // Check for any new message types in the temp pool that we haven't processed yet
            for message in temp_pool.all_messages() {
                let message_name = message.full_name();
                if !processed_symbols.contains(message_name) {
                    new_symbols.push(message_name.to_string());
                }
            }
        }

        Ok(new_symbols)
    }

    /// Parse a JSON message into bytes using reflection
    pub async fn parse_json_to_message(
        &self,
        message_name: &str,
        json_str: &str,
    ) -> Result<Vec<u8>> {
        // Get the message descriptor
        let descriptor = self.get_message_descriptor(message_name).await?;

        // Use ProtobufDescriptor's json_to_message method
        ProtobufDescriptor::json_to_message(descriptor, json_str, true)
            .context("Failed to convert JSON to message")
    }

    /// Parse bytes into a JSON message using reflection
    pub async fn parse_bytes_to_json(&self, message_name: &str, bytes: &[u8]) -> Result<String> {
        // Get the message descriptor
        let descriptor = self.get_message_descriptor(message_name).await?;

        // Parse the bytes into a dynamic message
        let dynamic_message = ProtobufDescriptor::get_message_from_bytes(descriptor, bytes)
            .context("Failed to decode bytes to dynamic message")?;

        // Encode the dynamic message to JSON
        ProtobufDescriptor::message_to_json(&dynamic_message)
            .context("Failed to serialize dynamic message to JSON")
    }

    /// Get all message types registered in a descriptor pool obtained from a service
    pub fn get_all_messages_from_pool(&self, pool: &DescriptorPool) -> Vec<MessageDescriptor> {
        pool.all_messages().collect()
    }

    /// Get a list of all message names from a descriptor pool
    pub fn get_all_message_names_from_pool(&self, pool: &DescriptorPool) -> Vec<String> {
        pool.all_messages()
            .map(|m| m.full_name().to_string())
            .collect()
    }

    /// Convert a dynamic message to a more readable debug format
    pub fn format_dynamic_message(&self, message: &DynamicMessage) -> String {
        ProtobufDescriptor::dynamic_message_to_string(message, true)
    }

    /// Efficiently fetch all service methods for a service
    pub async fn get_service_methods(&self, service_name: &str) -> Result<Vec<String>> {
        let pool = self.get_service_with_dependencies(service_name).await?;

        // Find the service descriptor and extract methods
        let service_descriptor = pool
            .get_service_by_name(service_name)
            .ok_or_else(|| anyhow::anyhow!("Service not found: {}", service_name))?;

        let method_names = service_descriptor
            .methods()
            .map(|method| method.name().to_string())
            .collect();

        Ok(method_names)
    }

    /// Get a preview of message structure - useful for development and debugging
    pub async fn get_message_field_names(&self, message_name: &str) -> Result<Vec<String>> {
        let descriptor = self.get_message_descriptor(message_name).await?;
        let field_names = descriptor.fields().map(|f| f.name().to_string()).collect();
        Ok(field_names)
    }

    /// Construct a sample/template JSON for a message type - useful for API exploration
    pub async fn get_message_template(&self, message_name: &str) -> Result<String> {
        let descriptor = self.get_message_descriptor(message_name).await?;
        let mut template = std::collections::HashMap::new();

        for field in descriptor.fields() {
            // Match kinds by name to avoid compatibility issues between prost_reflect versions
            let kind_name = format!("{:?}", field.kind());
            let default_value = if kind_name.contains("Bool") {
                serde_json::Value::Bool(false)
            } else if kind_name.contains("Int32")
                || kind_name.contains("Sint32")
                || kind_name.contains("Sfixed32")
                || kind_name.contains("Fixed32")
                || kind_name.contains("Uint32")
            {
                serde_json::Value::Number(serde_json::Number::from(0))
            } else if kind_name.contains("Int64")
                || kind_name.contains("Sint64")
                || kind_name.contains("Uint64")
                || kind_name.contains("Fixed64")
                || kind_name.contains("Sfixed64")
            {
                serde_json::Value::String("0".to_string()) // JSON can't handle 64-bit ints directly
            } else if kind_name.contains("Float") || kind_name.contains("Double") {
                serde_json::Value::Number(serde_json::Number::from_f64(0.0).unwrap())
            } else if kind_name.contains("String") {
                serde_json::Value::String("string".to_string())
            } else if kind_name.contains("Bytes") {
                serde_json::Value::String("base64encoded".to_string())
            } else if kind_name.contains("Message") {
                serde_json::Value::Object(serde_json::Map::new())
            } else if kind_name.contains("Enum") {
                serde_json::Value::Number(serde_json::Number::from(0))
            } else {
                serde_json::Value::Null
            };

            // Handle JSON field naming convention (camelCase)
            let json_name = field.json_name().to_string();
            template.insert(json_name, default_value);
        }

        serde_json::to_string_pretty(&template).context("Failed to serialize message template")
    }
}

pub trait UseServerReflectionClient {
    fn grpc_reflection_client(&self) -> &GrpcReflectionClient;
}

// Import test module
#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::time::Duration;

    // This is an integration test that requires a running gRPC server with reflection enabled
    // The server should be running on localhost:9000
    // and expose FunctionService and FunctionSetService
    // To run, use: cargo test -- --ignored reflection_client

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running server
    async fn test_reflection_client_services() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        // List all services
        let services = client.list_services().await?;
        println!("Available services: {services:?}");

        // Verify expected services exist
        assert!(
            services.contains(&"jobworkerp.function.service.FunctionService".to_string()),
            "FunctionService not found in available services"
        );
        assert!(
            services.contains(&"jobworkerp.function.service.FunctionSetService".to_string()),
            "FunctionSetService not found in available services"
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running server
    async fn test_function_service_methods() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        let service_name = "jobworkerp.function.service.FunctionService";

        // Get methods for FunctionService
        let methods = client.get_service_methods(service_name).await?;
        println!("FunctionService methods: {methods:?}");

        // Verify expected methods exist
        assert!(
            methods.contains(&"FindList".to_string()),
            "FindList method not found"
        );
        assert!(
            methods.contains(&"FindListBySet".to_string()),
            "FindListBySet method not found"
        );

        // Get service descriptors
        let pool = client.get_service_with_dependencies(service_name).await?;
        let messages = client.get_all_message_names_from_pool(&pool);
        println!("Messages from FunctionService: {messages:?}");

        // Test getting message descriptor for FindFunctionRequest
        let req_message = "jobworkerp.function.service.FindFunctionRequest";
        let _descriptor = client.get_message_descriptor(req_message).await?;

        // Get field names for the request message
        let fields = client.get_message_field_names(req_message).await?;
        println!("FindFunctionRequest fields: {fields:?}");
        assert!(fields.contains(&"exclude_runner".to_string()));
        assert!(fields.contains(&"exclude_worker".to_string()));

        // Get message template
        let template = client.get_message_template(req_message).await?;
        println!("Template for FindFunctionRequest: {template}");

        // Parse and serialize a simple request
        let json = r#"{"excludeRunner": false, "excludeWorker": false}"#;
        let bytes = client.parse_json_to_message(req_message, json).await?;
        let json_back = client.parse_bytes_to_json(req_message, &bytes).await?;
        println!("JSON roundtrip: {json_back}");

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running server
    async fn test_function_set_service_methods() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        let service_name = "jobworkerp.function.service.FunctionSetService";

        // Get methods for FunctionSetService
        let methods = client.get_service_methods(service_name).await?;
        println!("FunctionSetService methods: {methods:?}");

        // Verify expected methods exist
        assert!(
            methods.contains(&"Create".to_string()),
            "Create method not found"
        );
        assert!(
            methods.contains(&"Find".to_string()),
            "Find method not found"
        );
        assert!(
            methods.contains(&"FindByName".to_string()),
            "FindByName method not found"
        );
        assert!(
            methods.contains(&"FindList".to_string()),
            "FindList method not found"
        );

        // Get message descriptor for FunctionSetData
        let data_message = "jobworkerp.function.data.FunctionSetData";
        let template = client.get_message_template(data_message).await?;
        println!("Template for FunctionSetData: {template}");

        // Test the recursive descriptor fetching
        let pool = client.get_service_with_dependencies(service_name).await?;
        let all_messages = client.get_all_message_names_from_pool(&pool);
        println!("All messages from FunctionSetService with deps: {all_messages:?}");

        // Check if dependencies are correctly fetched
        assert!(
            all_messages.contains(&"jobworkerp.function.data.FunctionSet".to_string()),
            "FunctionSet message not found in dependencies"
        );
        assert!(
            all_messages.contains(&"jobworkerp.function.data.FunctionSetData".to_string()),
            "FunctionSetData message not found in dependencies"
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running server
    async fn test_message_conversion() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        // Create a FunctionSetData message using reflection
        let message_name = "jobworkerp.function.data.FunctionSetData";
        let json = r#"{
            "name": "test-function-set",
            "description": "Test function set created via reflection",
            "category": 1,
            "targets": [
                {
                    "id": 123,
                    "type": "RUNNER"
                }
            ]
        }"#;

        // Convert JSON to protobuf binary
        let bytes = client.parse_json_to_message(message_name, json).await?;

        // Convert back to JSON to verify roundtrip
        let json_back = client.parse_bytes_to_json(message_name, &bytes).await?;
        println!("Roundtrip JSON: {json_back}");

        // Get the message descriptor and create a dynamic message
        let descriptor = client.get_message_descriptor(message_name).await?;
        let dynamic_message = ProtobufDescriptor::get_message_from_bytes(descriptor, &bytes)?;

        // Format the dynamic message for debugging
        let formatted = client.format_dynamic_message(&dynamic_message);
        println!("Formatted message: {formatted}");

        // Verify fields
        let name_field = dynamic_message.get_field_by_name("name").unwrap();
        assert_eq!(name_field.as_str().unwrap(), "test-function-set");

        let category_field = dynamic_message.get_field_by_name("category").unwrap();
        assert_eq!(category_field.as_i32().unwrap(), 1);

        let targets_field = dynamic_message.get_field_by_name("targets").unwrap();
        assert!(targets_field.as_list().is_some());

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running server
    async fn test_function_specs_reflection() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        // Get descriptor for FunctionSpecs message
        let message_name = "jobworkerp.function.data.FunctionSpecs";
        let descriptor = client.get_message_descriptor(message_name).await?;

        // Get template to understand structure
        let template = client.get_message_template(message_name).await?;
        println!("Template for FunctionSpecs: {template}");

        // Create a FunctionSpecs message via JSON
        let json = r#"{
            "name": "test-function-via-reflection",
            "description": "A function created using gRPC reflection",
            "runnerId": {
                "value": "123"
            },
            "singleSchema": {
                "arguments": "{\"type\":\"object\",\"properties\":{\"input\":{\"type\":\"string\"}}}"
            }
        }"#;

        // Convert to binary and back
        let bytes = client.parse_json_to_message(message_name, json).await?;
        let parsed_json = client.parse_bytes_to_json(message_name, &bytes).await?;
        println!("FunctionSpecs roundtrip: {parsed_json}");

        // Get field structure
        let field_names = client.get_message_field_names(message_name).await?;
        println!("Fields in FunctionSpecs: {field_names:?}");

        // Check that expected fields exist
        assert!(field_names.contains(&"name".to_string()));
        assert!(field_names.contains(&"description".to_string()));
        assert!(field_names.contains(&"runner_id".to_string()));

        // Get oneof fields
        println!("Testing oneof fields detection...");
        let schema_field = descriptor.get_field_by_name("single_schema");
        assert!(
            schema_field.is_some(),
            "Expected to find single_schema field"
        );

        // Test dependencies
        let pool = client
            .get_service_with_dependencies("jobworkerp.function.service.FunctionService")
            .await?;
        let all_messages = client.get_all_message_names_from_pool(&pool);
        assert!(
            all_messages.contains(&message_name.to_string()),
            "Expected FunctionSpecs to be part of FunctionService descriptors"
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running server
    async fn test_descriptor_cache() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        // First request - should populate the cache
        let service_name = "jobworkerp.function.service.FunctionService";

        // Check initial cache state
        {
            let cache = client.descriptor_cache.read().unwrap();
            let initial_symbols_count = cache.processed_symbols.len();
            let initial_files_count = cache.processed_files.len();
            println!(
                "Initial cache state: {initial_symbols_count} symbols, {initial_files_count} files"
            );
        }

        // First call to get_service_with_dependencies - should populate the cache
        let start = std::time::Instant::now();
        let pool1 = client.get_service_with_dependencies(service_name).await?;
        let first_duration = start.elapsed();
        println!("First request duration: {first_duration:?}");

        // Get count of messages to verify the pool is populated correctly
        let messages1 = client.get_all_message_names_from_pool(&pool1);
        println!("Found {} messages in first request", messages1.len());

        // Check cache state after first request
        {
            let cache = client.descriptor_cache.read().unwrap();
            let cache_size_after_first = cache.processed_symbols.len();
            println!(
                "Cache after first request: {} symbols, {} files",
                cache.processed_symbols.len(),
                cache.processed_files.len()
            );

            // Verify the requested service is in the cache
            assert!(
                cache.processed_symbols.contains(service_name),
                "Service name should be in the cache after first request"
            );

            // Verify cache has entries
            assert!(
                cache_size_after_first > 0,
                "Cache should have entries after first request"
            );
        }

        // Second call to the same service - should use the cache
        let start = std::time::Instant::now();
        let pool2 = client.get_service_with_dependencies(service_name).await?;
        let second_duration = start.elapsed();
        println!("Second request duration: {second_duration:?}");

        // Get messages from second request
        let messages2 = client.get_all_message_names_from_pool(&pool2);
        println!("Found {} messages in second request", messages2.len());

        // Both requests should yield the same number of messages
        assert_eq!(
            messages1.len(),
            messages2.len(),
            "Both requests should yield the same number of messages"
        );

        // Request a specific message that should have been cached from the service request
        let message_name = messages1.first().unwrap();
        let start = std::time::Instant::now();
        let message_descriptor = client.get_message_descriptor(message_name).await?;
        let message_request_duration = start.elapsed();
        println!("Message request duration: {message_request_duration:?}");

        // Verify the message descriptor was retrieved correctly
        assert_eq!(message_descriptor.full_name(), message_name);

        // Cache usage effectiveness test - the second request should be significantly faster
        // This is a heuristic test, as timing can vary based on system load
        // We're looking for the second request to be at least 30% faster than the first
        // However, on CI systems with high variability, this might not always hold true
        println!("First request: {first_duration:?}, Second request: {second_duration:?}");
        println!(
            "Speed improvement: {:.2}x",
            first_duration.as_secs_f64() / second_duration.as_secs_f64()
        );

        // This assertion might be too strict depending on the environment
        // It's commented out but can be uncommented if needed for stricter testing
        // assert!(second_duration.as_secs_f64() < first_duration.as_secs_f64() * 0.7,
        //     "Second request should be significantly faster than first request due to caching");

        // Instead, log the timing information for manual verification
        if second_duration.as_secs_f64() < first_duration.as_secs_f64() * 0.7 {
            println!("Cache is working effectively - second request was >30% faster");
        } else {
            println!("Cache timing test inconclusive - could be due to system conditions");
        }

        // Test specific cache mechanism functions directly
        // Request the same message again - should be fully cached with minimal network activity
        let start = std::time::Instant::now();
        let _descriptor2 = client.get_message_descriptor(message_name).await?;
        let cached_request_duration = start.elapsed();
        println!("Fully cached message request duration: {cached_request_duration:?}");

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running server
    async fn test_cross_method_cache_sharing() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        let service_name = "jobworkerp.function.service.FunctionService";
        let pool = client.get_service_with_dependencies(service_name).await?;
        let messages = client.get_all_message_names_from_pool(&pool);

        // Find a message that should now be in the cache
        let message_name = messages
            .iter()
            .find(|m| m.contains("Request") || m.contains("Response"))
            .expect("Should find at least one request or response message");

        // Check cache state after service resolution
        {
            let cache = client.descriptor_cache.read().unwrap();
            println!(
                "Cache after service resolution: {} symbols, {} files",
                cache.processed_symbols.len(),
                cache.processed_files.len()
            );

            // Verify the service and message are in the cache
            assert!(
                cache.processed_symbols.contains(service_name),
                "Service name should be in the cache"
            );
            assert!(
                cache.processed_symbols.contains(message_name),
                "Message should be in the cache"
            );
        }

        // Now try to get the message descriptor directly
        // This should use the cache instead of making a network call
        let start = std::time::Instant::now();
        let message_descriptor = client.get_message_descriptor(message_name).await?;
        let duration = start.elapsed();

        // Verify we got the correct descriptor
        assert_eq!(message_descriptor.full_name(), message_name);
        println!("Getting cached message took: {duration:?}");

        // Since this should be a cache hit, it should be very fast (sub-millisecond typically)
        // but we'll use a more conservative threshold for CI environments
        assert!(
            duration.as_millis() < 100,
            "Cache retrieval should be fast, took {duration:?}"
        );

        // Now try getting another service - this should populate additional cache entries
        let another_service = "jobworkerp.function.service.FunctionSetService";
        let start = std::time::Instant::now();
        let _ = client
            .get_service_with_dependencies(another_service)
            .await?;
        let _duration = start.elapsed();

        // Cache should now have additional entries
        {
            let cache = client.descriptor_cache.read().unwrap();
            println!(
                "Cache after second service: {} symbols, {} files",
                cache.processed_symbols.len(),
                cache.processed_files.len()
            );

            // Both services should be in the cache
            assert!(
                cache.processed_symbols.contains(service_name),
                "First service should still be in cache"
            );
            assert!(
                cache.processed_symbols.contains(another_service),
                "Second service should be in cache"
            );

            // Cache should have more entries after second service resolution
            assert!(
                cache.processed_symbols.len() > messages.len(),
                "Cache should have more symbols after resolving second service"
            );
        }

        // Re-request both services to verify they're cached
        let start = std::time::Instant::now();
        let _ = client.get_service_with_dependencies(service_name).await?;
        let _ = client
            .get_service_with_dependencies(another_service)
            .await?;
        let duration = start.elapsed();

        println!("Re-requesting both services took: {duration:?}");
        // This should be faster than getting just one service the first time
        // but we avoid strict timing assertions for CI reliability

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a running server
    async fn test_cache_efficiency_multiple_messages() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        // Get service with dependencies to populate cache
        let service_name = "jobworkerp.function.service.FunctionSetService";
        let pool = client.get_service_with_dependencies(service_name).await?;

        // Get all message names from the pool
        let message_names = client.get_all_message_names_from_pool(&pool);
        println!("Found {} messages in service", message_names.len());

        // Select a few different messages to test
        let message_samples: Vec<&String> = message_names
            .iter()
            .filter(|m| {
                m.contains("FunctionSet") || m.contains("Request") || m.contains("Response")
            })
            .take(3)
            .collect();

        assert!(
            !message_samples.is_empty(),
            "Should find some messages to test"
        );
        println!("Testing with messages: {message_samples:?}");

        // First batch: get each message descriptor - first request should be slower
        let mut first_request_durations = Vec::new();
        for &message_name in &message_samples {
            let start = std::time::Instant::now();
            let descriptor = client.get_message_descriptor(message_name).await?;
            let duration = start.elapsed();
            println!("First request for {message_name}: {duration:?}");
            first_request_durations.push(duration);

            // Verify we got the right descriptor
            assert_eq!(descriptor.full_name(), message_name);
        }

        // Second batch: get the same messages again - should be faster due to caching
        let mut second_request_durations = Vec::new();
        for &message_name in &message_samples {
            let start = std::time::Instant::now();
            let descriptor = client.get_message_descriptor(message_name).await?;
            let duration = start.elapsed();
            println!("Second request for {message_name}: {duration:?}");
            second_request_durations.push(duration);

            // Verify we got the right descriptor
            assert_eq!(descriptor.full_name(), message_name);
        }

        // Calculate average improvement
        let avg_first: f64 = first_request_durations
            .iter()
            .map(|d| d.as_secs_f64())
            .sum::<f64>()
            / first_request_durations.len() as f64;
        let avg_second: f64 = second_request_durations
            .iter()
            .map(|d| d.as_secs_f64())
            .sum::<f64>()
            / second_request_durations.len() as f64;

        println!("Average first request: {avg_first:.6}s");
        println!("Average second request: {avg_second:.6}s");
        println!(
            "Average improvement: {:.2}x",
            if avg_second > 0.0 {
                avg_first / avg_second
            } else {
                0.0
            }
        );

        // Test for field names retrieval - should also use cache
        for &message_name in &message_samples {
            let start = std::time::Instant::now();
            let field_names = client.get_message_field_names(message_name).await?;
            let duration = start.elapsed();
            println!(
                "Field names for {}: {:?} (took: {:?})",
                message_name,
                field_names.len(),
                duration
            );
        }

        // Try getting message template - should use cached descriptors
        for &message_name in &message_samples {
            let start = std::time::Instant::now();
            let template = client.get_message_template(message_name).await?;
            let duration = start.elapsed();
            println!("Template generation for {message_name} took: {duration:?}");
            assert!(!template.is_empty(), "Template should not be empty");
        }

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Ignore by default as it requires a server
    async fn test_hashmap_cache_efficiency() -> Result<()> {
        let client = GrpcReflectionClient::new(
            "http://localhost:9000".to_string(),
            Some(Duration::from_secs(5)),
        )
        .await?;

        // First, populate the cache with a service definition
        let service_name = "jobworkerp.function.service.FunctionService";
        tracing::info!("Initial service resolution to populate cache");
        let _ = client.get_service_with_dependencies(service_name).await?;

        // Get cache state metrics after initial population
        let (symbols_count, files_count, cache_size) = {
            let cache = client.descriptor_cache.read().unwrap();
            (
                cache.processed_symbols.len(),
                cache.processed_files.len(),
                cache.file_descriptors.len(),
            )
        };
        tracing::info!(
            "Cache state: {} symbols, {} files, {} descriptors",
            symbols_count,
            files_count,
            cache_size
        );
        assert!(cache_size > 0, "Cache should contain descriptor entries");

        let file_names: Vec<String> = {
            let cache = client.descriptor_cache.read().unwrap();
            cache.file_descriptors.keys().cloned().collect()
        };

        if let Some(file_name) = file_names.first() {
            tracing::info!("Testing direct file lookup by name: {}", file_name);

            // Test direct access to a specific file descriptor
            let start = std::time::Instant::now();
            let descriptor_exists = {
                let cache = client.descriptor_cache.read().unwrap();
                cache.file_descriptors.contains_key(file_name)
            };
            let lookup_duration = start.elapsed();

            assert!(descriptor_exists, "File should exist in cache");
            tracing::info!("Direct HashMap lookup took: {:?}", lookup_duration);

            // This should be extremely fast due to HashMap O(1) lookup
            assert!(
                lookup_duration.as_nanos() < 1_000_000,
                "HashMap lookup should be very fast (<1ms)"
            );

            // Now test a more realistic scenario - fetch a file by name
            tracing::info!("Testing fetch_file_by_name with cached file");
            let mut client_ref = client.reflection_client().await;
            let start = std::time::Instant::now();
            let descriptors = client
                .fetch_file_by_name(&mut client_ref, file_name)
                .await?;
            let fetch_duration = start.elapsed();

            assert!(!descriptors.is_empty(), "Should retrieve cached descriptor");
            tracing::info!("Fetch of cached file took: {:?}", fetch_duration);

            // Fetch should be fast as it uses cache
            assert!(
                fetch_duration.as_millis() < 100,
                "Cached file fetch should be fast (<100ms)"
            );
        } else {
            tracing::warn!("No files in cache to test with");
        }

        // Test pool creation with cached descriptors
        tracing::info!("Testing repeated service resolution with cache");
        let start = std::time::Instant::now();
        let _ = client.get_service_with_dependencies(service_name).await?;
        let cached_resolution_duration = start.elapsed();

        tracing::info!(
            "Cached service resolution took: {:?}",
            cached_resolution_duration
        );

        // Check final cache state - should be same or larger
        let (final_symbols, final_files, final_cache_size) = {
            let cache = client.descriptor_cache.read().unwrap();
            (
                cache.processed_symbols.len(),
                cache.processed_files.len(),
                cache.file_descriptors.len(),
            )
        };

        assert!(
            final_symbols >= symbols_count,
            "Cache symbol count should not decrease"
        );
        assert!(
            final_files >= files_count,
            "Cache file count should not decrease"
        );
        assert!(
            final_cache_size >= cache_size,
            "Cache descriptor count should not decrease"
        );

        tracing::info!(
            "Final cache state: {} symbols, {} files, {} descriptors",
            final_symbols,
            final_files,
            final_cache_size
        );

        Ok(())
    }

    /// Test nested dependency resolution for complex protobuf structures
    /// This test specifically targets the case where dependencies have their own dependencies
    /// that need to be recursively resolved (like rss_item.proto -> rss_channel.proto)
    #[tokio::test]
    #[ignore]
    async fn test_nested_dependency_resolution() -> Result<()> {
        let _ = tracing_subscriber::fmt::try_init();

        // Connect to the gRPC server that has nested dependencies
        let client = GrpcReflectionClient::new("http://localhost:9010".to_string(), None)
            .await
            .context("Failed to create reflection client")?;

        tracing::info!("Testing nested dependency resolution with complex proto structure");

        // Test with a service that has nested dependencies
        // Based on the log output, rss_item.proto depends on rss_channel.proto
        // but rss_channel.proto is not being fetched properly
        let service_name = "news_aggregator.service.RssItemService";

        tracing::info!("Attempting to resolve service: {}", service_name);

        let result = client.get_service_with_dependencies(service_name).await;

        match result {
            Ok(pool) => {
                tracing::info!("Successfully resolved nested dependencies");

                // Verify that all necessary types are available in the pool
                let service = pool
                    .get_service_by_name(service_name)
                    .ok_or_else(|| anyhow::anyhow!("Service {} not found in pool", service_name))?;

                tracing::info!(
                    "Service {} found with {} methods",
                    service_name,
                    service.methods().count()
                );

                // Try to find some expected message types that should be available
                // if dependencies were resolved correctly
                let rss_item_data = pool.get_message_by_name("news_aggregator.data.RssItemData");
                let rss_channel_data =
                    pool.get_message_by_name("news_aggregator.data.RssChannelData");

                if rss_item_data.is_some() && rss_channel_data.is_some() {
                    tracing::info!(
                        "✓ All expected message types found - nested dependencies resolved correctly"
                    );
                } else {
                    tracing::warn!("Some expected message types missing:");
                    if rss_item_data.is_none() {
                        tracing::warn!("  - news_aggregator.data.RssItemData not found");
                    }
                    if rss_channel_data.is_none() {
                        tracing::warn!("  - news_aggregator.data.RssChannelData not found");
                    }
                    return Err(anyhow::anyhow!("Nested dependency resolution incomplete"));
                }
            }
            Err(e) => {
                tracing::error!("Failed to resolve nested dependencies: {}", e);
                return Err(e).context("Nested dependency resolution test failed");
            }
        }

        Ok(())
    }

    /// Test that specifically reproduces the rss_channel.proto dependency issue
    #[tokio::test]
    #[ignore]
    async fn test_rss_channel_dependency_resolution() -> Result<()> {
        let _ = tracing_subscriber::fmt::try_init();

        let client = GrpcReflectionClient::new("http://localhost:9010".to_string(), None)
            .await
            .context("Failed to create reflection client")?;

        tracing::info!("Testing specific rss_channel.proto dependency resolution");

        // Test with LinkScrapingStatusService which was failing in the logs
        let service_name = "news_aggregator.service.LinkScrapingStatusService";

        tracing::info!("Attempting to resolve service: {}", service_name);

        let result = client.get_service_with_dependencies(service_name).await;

        // This should fail with the current implementation due to missing rss_channel.proto
        match result {
            Ok(_) => {
                tracing::info!(
                    "✓ Successfully resolved all dependencies including rss_channel.proto"
                );
            }
            Err(e) => {
                let error_msg = format!("{}", e);
                if error_msg.contains("news_aggregator/data/rss_channel.proto") {
                    tracing::error!(
                        "✗ Expected failure: rss_channel.proto dependency not resolved"
                    );
                    tracing::error!("Error: {}", e);
                    return Err(e)
                        .context("rss_channel.proto dependency resolution failed as expected");
                } else {
                    tracing::error!("Unexpected error: {}", e);
                    return Err(e).context("Unexpected error during dependency resolution");
                }
            }
        }

        Ok(())
    }
}
