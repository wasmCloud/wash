# Comprehensive OpenTelemetry Guide for Rust Projects

This guide documents OpenTelemetry patterns and best practices for the wash codebase, based on existing implementations and Rust ecosystem standards.

## 1. Recommended Crate Stack

### Core Dependencies (Cargo.toml)

```toml
[dependencies]
# OpenTelemetry core and SDK
opentelemetry = { version = "0.28", default-features = false }
opentelemetry_sdk = { version = "0.28", default-features = false }
opentelemetry-semantic-conventions = { version = "0.28", default-features = false, features = ["semconv_experimental"] }

# Exporters (choose based on needs)
opentelemetry-otlp = { version = "0.28", default-features = false }  # Production: OTLP export
opentelemetry-stdout = { version = "0.28", default-features = false } # Development: console output

# Tracing integration (Rust ecosystem standard)
tracing = { version = "0.1.41", default-features = false, features = ["attributes"] }
tracing-subscriber = { version = "0.3.19", default-features = false, features = ["env-filter", "ansi", "time", "json"] }
tracing-opentelemetry = { version = "0.28" }  # Bridge tracing spans to OTLP

# Optional: Bridge tracing logs to OTLP
opentelemetry-appender-tracing = { version = "0.28", default-features = false }

```

### Crate Purpose Summary

| Crate | Purpose |
|-------|---------|
| `opentelemetry` | Core APIs for traces, metrics, context propagation |
| `opentelemetry_sdk` | SDK implementation (TracerProvider, MeterProvider, Resource) |
| `opentelemetry-semantic-conventions` | Standard attribute names (SERVICE_NAME, etc.) |
| `opentelemetry-otlp` | OTLP exporter for collectors (Jaeger, Grafana, Aspire) |
| `tracing` | Rust's async-aware instrumentation facade |
| `tracing-subscriber` | Subscriber configuration with composable layers |
| `tracing-opentelemetry` | Bridge `tracing` spans to OpenTelemetry |

**Note:** Minimum Rust version is 1.75+ for OpenTelemetry crates.

---

## 2. Style Guide for Instrumentation

### 2.1 The `#[instrument]` Macro

**When to use:**

- Async functions with I/O operations
- Operations taking >10ms
- Entry points to logical subsystems
- Functions where tracing context is valuable for debugging

**When NOT to use:**

- Simple getters/setters
- Pure functions with no I/O
- Hot path functions called thousands of times per second
- Functions that only delegate to other instrumented functions

```rust
// GOOD: Long-running operations with skip_all and named spans
#[instrument(level = "debug", skip_all, name = "component_build")]
async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
    // ...
}

// GOOD: Operations with meaningful fields to capture
#[instrument(level = "debug", skip_all, fields(subject = %subject, timeout_ms))]
async fn request(&mut self, subject: String, body: Vec<u8>, timeout_ms: u32) -> Result<Vec<u8>> {
    // ...
}

// GOOD: Skip sensitive/large data, capture identifiers
#[instrument(skip(config, component_data), fields(reference = %reference, size = component_data.len()))]
pub async fn push_component(reference: &str, component_data: Vec<u8>, config: Config) -> Result<String> {
    // ...
}

// BAD: Instrumenting hot paths - creates spans thousands of times/sec
#[instrument]
fn get_item(&self, key: &str) -> Option<&Item> {
    self.items.get(key)
}

// BAD: Capturing large data structures via Debug
#[instrument]  // Formats entire Vec via Debug
fn process_items(&self, items: Vec<LargeStruct>) { }
```

### 2.2 Field Naming Conventions

```rust
// Use snake_case for all field names
fields(workload_id = %id, component_path = ?path)

// Use % for Display formatting, ? for Debug formatting
fields(subject = %subject)           // String-like types
fields(resource_id = ?rep)           // Complex types needing Debug

// Common naming patterns:
// - *_id: identifiers (workload_id, component_id, request_id)
// - *_path: file paths (component_path, project_path)
// - *_count: numeric counts (item_count, retry_count)
// - *_ms: durations in milliseconds (timeout_ms, latency_ms)
// - *_bytes: sizes (payload_bytes, response_bytes)
```

### 2.3 Span Naming Conventions

```rust
// PascalCase for domain concepts
tracing::span!(Level::INFO, "HostInvocation", ...)
tracing::span!(Level::INFO, "DatabaseQuery", ...)

// snake_case for CLI/operation names in #[instrument]
#[instrument(name = "component_build")]
#[instrument(name = "registry_push")]
```

### 2.4 Logging Level Guidelines

| Level | Use Case | Example |
|-------|----------|---------|
| `error!` | Unrecoverable errors, failures that stop execution | `error!(err = ?e, "failed to initialize component")` |
| `warn!` | Recoverable issues, deprecated usage, degraded operation | `warn!("falling back to default config")` |
| `info!` | User-facing progress, key milestones | `info!(path = %self.project_path.display(), "building component")` |
| `debug!` | Developer debugging, detailed flow | `debug!(component_path = %path, "reading cached artifact")` |
| `trace!` | Very detailed execution flow, performance-sensitive areas | `trace!("entering critical section")` |

### 2.5 Error Recording Pattern

```rust
// BAD: Error without context
tracing::error!("operation failed");

// GOOD: Error with structured context
tracing::error!(
    err = ?e,
    workload_id = %self.id,
    operation = "get_key",
    "JetStream error getting key"
);

// GOOD: Using anyhow context for error chains
result.with_context(|| format!("failed to pull component from {reference}"))?;
```

---

## 3. Metrics Pattern

### 3.1 Counter Pattern

```rust
use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;
use std::sync::Arc;

struct ServiceMetrics {
    operations_total: Counter<u64>,
    errors_total: Counter<u64>,
}

impl ServiceMetrics {
    fn new(meter: &opentelemetry::metrics::Meter) -> Self {
        Self {
            operations_total: meter
                .u64_counter("service_operations_total")
                .with_description("Total number of operations performed")
                .build(),
            errors_total: meter
                .u64_counter("service_errors_total")
                .with_description("Total number of errors encountered")
                .build(),
        }
    }
}

impl Service {
    pub fn new() -> Self {
        let meter = opentelemetry::global::meter("my-service");
        Self {
            metrics: Arc::new(ServiceMetrics::new(&meter)),
        }
    }

    fn record_operation(&self, operation: &str, success: bool) {
        let attrs = [KeyValue::new("operation", operation.to_string())];
        self.metrics.operations_total.add(1, &attrs);
        if !success {
            self.metrics.errors_total.add(1, &attrs);
        }
    }
}
```

### 3.2 Metric Naming Conventions

```text
# Pattern: <namespace>_<metric>_<unit>
service_operations_total          # Counter - total operations
http_request_duration_seconds     # Histogram - request latency
component_instances_active        # Gauge - current count

# Standard label names:
# - operation: get, set, delete, list
# - status: success, error
# - method: GET, POST, PUT
# - component_id: identifier
```

---

## 4. Resource Configuration

```rust
use opentelemetry::KeyValue;
use opentelemetry_sdk::resource::{Resource, ResourceBuilder};
use opentelemetry_semantic_conventions::resource;

pub fn build_resource() -> Resource {
    Resource::builder()
        // Required semantic conventions
        .with_attribute(KeyValue::new(
            resource::SERVICE_NAME.to_string(),
            env!("CARGO_PKG_NAME"),
        ))
        .with_attribute(KeyValue::new(
            resource::SERVICE_VERSION.to_string(),
            env!("CARGO_PKG_VERSION"),
        ))
        .with_attribute(KeyValue::new(
            resource::SERVICE_INSTANCE_ID.to_string(),
            uuid::Uuid::new_v4().to_string(),
        ))
        // Environment context
        .with_attribute(KeyValue::new(
            "deployment.environment",
            std::env::var("ENVIRONMENT").unwrap_or_else(|_| "development".into()),
        ))
        .build()
}
```

---

## 5. SDK Initialization Patterns

### 5.1 CLI Application (tracing only, no OTLP)

```rust
use tracing::Level;
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt, util::SubscriberInitExt};

fn initialize_tracing(log_level: Level, verbose: bool) {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_level.as_str()))
        // Suppress noisy dependencies
        .add_directive("hyper=warn".parse().unwrap())
        .add_directive("h2=warn".parse().unwrap());

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(verbose)
        .with_thread_ids(verbose)
        .with_level(true)
        .with_file(verbose)
        .with_line_number(verbose)
        .with_ansi(atty::is(atty::Stream::Stderr));

    Registry::default()
        .with(env_filter)
        .with(fmt_layer)
        .init();
}
```

### 5.2 Service with OTLP Export

```rust
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Registry};

fn init_telemetry() -> anyhow::Result<SdkTracerProvider> {
    let resource = build_resource();

    // Configure OTLP exporter
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4317".into()))
        .build()?;

    // Build tracer provider
    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    // Set as global provider
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());

    // Create OpenTelemetry layer for tracing
    let tracer = tracer_provider.tracer("my-service");
    let otel_layer = OpenTelemetryLayer::new(tracer);

    // Combine with fmt layer for local output
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr);

    Registry::default()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(otel_layer)
        .with(fmt_layer)
        .init();

    Ok(tracer_provider)
}

// Graceful shutdown
fn shutdown_telemetry(provider: SdkTracerProvider) {
    if let Err(e) = provider.shutdown() {
        eprintln!("Error shutting down tracer provider: {e:?}");
    }
}
```

---

## 6. Manual Span Creation

For cross-component or plugin tracing where `#[instrument]` isn't suitable:

```rust
pub fn host_invocation_span(
    component_id: impl AsRef<str>,
    operation: impl AsRef<str>,
) -> tracing::Span {
    tracing::span!(
        tracing::Level::INFO,
        "HostInvocation",
        component_id = component_id.as_ref(),
        operation = operation.as_ref(),
    )
}

// Usage
async fn invoke_component(&self, id: &str, op: &str) -> Result<()> {
    let span = host_invocation_span(id, op);
    async {
        // ... operation
    }.instrument(span).await
}
```

---

## 7. Async/Tokio Patterns

### 7.1 Instrumenting Async Functions

The `#[instrument]` macro works seamlessly with async functions - spans automatically stay open across `.await` points:

```rust
use tracing::instrument;

#[instrument(level = "debug", skip(client))]
async fn fetch_user(client: &HttpClient, user_id: u64) -> Result<User> {
    // Span remains active across all await points
    let response = client.get(&format!("/users/{user_id}")).await?;
    let user: User = response.json().await?;
    Ok(user)
}
```

### 7.2 Manual Instrumentation with `.instrument()`

For futures that aren't async fn, use the `Instrument` trait:

```rust
use tracing::Instrument;

async fn process_batch(items: Vec<Item>) -> Result<()> {
    let futures: Vec<_> = items
        .into_iter()
        .map(|item| {
            let span = tracing::info_span!("process_item", item_id = %item.id);
            async move {
                // Process item...
                Ok::<_, Error>(())
            }
            .instrument(span)  // Attach span to this future
        })
        .collect();

    futures::future::try_join_all(futures).await?;
    Ok(())
}
```

### 7.3 Spawning Tasks with Tracing Context

**Problem:** `tokio::spawn` creates a new task without inheriting the current span.

```rust
use tracing::Instrument;

async fn parent_operation() {
    let span = tracing::info_span!("parent");
    let _guard = span.enter();

    // BAD: Spawned task loses tracing context
    tokio::spawn(async {
        tracing::info!("orphaned log");  // No parent span!
    });

    // GOOD: Explicitly instrument the spawned future
    let child_span = tracing::info_span!("child_task");
    tokio::spawn(
        async {
            tracing::info!("connected to parent");
        }
        .instrument(child_span)
    );
}
```

### 7.4 Span Context in Select/Race

When using `tokio::select!`, each branch should have its own span:

```rust
use tokio::select;
use tracing::Instrument;

async fn with_timeout<F, T>(future: F, timeout: Duration) -> Result<T, Elapsed>
where
    F: Future<Output = T>,
{
    select! {
        result = future.instrument(tracing::debug_span!("operation")) => {
            Ok(result)
        }
        _ = tokio::time::sleep(timeout).instrument(tracing::debug_span!("timeout")) => {
            Err(Elapsed)
        }
    }
}
```

### 7.5 Stream Processing

For async streams, instrument each item processing:

```rust
use futures::StreamExt;
use tracing::Instrument;

async fn process_stream<S>(stream: S)
where
    S: Stream<Item = Message> + Unpin,
{
    let mut stream = stream;
    while let Some(msg) = stream.next().await {
        let span = tracing::info_span!("handle_message", msg_id = %msg.id);
        async {
            // Process message
            handle_message(msg).await;
        }
        .instrument(span)
        .await;
    }
}
```

### 7.6 Batch Exporter Runtime Configuration

OpenTelemetry batch exporters need a runtime. Configure for tokio:

```rust
use opentelemetry_sdk::runtime::Tokio;

// For batch span export (recommended for production)
let tracer_provider = SdkTracerProvider::builder()
    .with_batch_exporter(exporter, Tokio)  // Use tokio runtime
    .with_resource(resource)
    .build();

// For simple/sync export (debugging only)
let tracer_provider = SdkTracerProvider::builder()
    .with_simple_exporter(exporter)  // Blocks on each span
    .build();
```

### 7.7 Graceful Shutdown with Tokio

Ensure spans are flushed before shutdown:

```rust
use tokio::signal;

#[tokio::main]
async fn main() -> Result<()> {
    let tracer_provider = init_telemetry()?;

    // Run your application
    let app_handle = tokio::spawn(run_app());

    // Wait for shutdown signal
    signal::ctrl_c().await?;
    tracing::info!("shutting down");

    // Cancel application tasks
    app_handle.abort();

    // Flush remaining spans (with timeout)
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || {
            tracer_provider.shutdown()
        })
    ).await??;

    Ok(())
}
```

### 7.8 Common Async Pitfalls

| Pitfall | Symptom | Solution |
|---------|---------|----------|
| Span dropped before await | Incomplete spans in traces | Use `#[instrument]` or `.instrument()` |
| Spawned tasks lose context | Orphaned spans in traces | Wrap with `.instrument(span)` |
| Blocking in async context | Metrics/spans delayed | Use `spawn_blocking` for shutdown |
| No batch runtime specified | Compilation error | Add `runtime::Tokio` to batch exporter |
| Span entered across await | Span timing incorrect | Use `instrument()` instead of `enter()` |

### 7.9 Correct Span Entry in Async Code

```rust
// BAD: .enter() guard held across await - span timing will be wrong
async fn bad_example() {
    let span = tracing::info_span!("operation");
    let _guard = span.enter();  // Guard lives across await!
    some_async_work().await;
}  // Guard dropped here, span duration includes await time incorrectly

// GOOD: Use .instrument() for async code
async fn good_example() {
    async {
        some_async_work().await;
    }
    .instrument(tracing::info_span!("operation"))
    .await;
}

// ALSO GOOD: Use #[instrument] macro
#[instrument]
async fn also_good() {
    some_async_work().await;
}
```

---

## 8. Context Propagation (Distributed Tracing)

### HTTP Headers (W3C Trace Context)

```rust
use opentelemetry::propagation::{TextMapPropagator, Injector, Extractor};
use opentelemetry_sdk::propagation::TraceContextPropagator;

struct HeaderInjector<'a>(&'a mut http::HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(name) = http::header::HeaderName::from_bytes(key.as_bytes()) {
            if let Ok(val) = http::header::HeaderValue::from_str(&value) {
                self.0.insert(name, val);
            }
        }
    }
}

// Inject into outgoing request
fn inject_context(headers: &mut http::HeaderMap) {
    let propagator = TraceContextPropagator::new();
    propagator.inject_context(&opentelemetry::Context::current(), &mut HeaderInjector(headers));
}

// Extract from incoming request
fn extract_context(headers: &http::HeaderMap) -> opentelemetry::Context {
    let propagator = TraceContextPropagator::new();
    propagator.extract(&HeaderExtractor(headers))
}
```

---

## 9. Common Mistakes to Avoid

| Mistake | Problem | Fix |
|---------|---------|-----|
| Instrumenting hot paths | Thousands of spans/sec, high overhead | Remove `#[instrument]` from frequently-called functions |
| Capturing large data | Debug-formatting large structs in fields | Use `skip(data)` and `fields(size = data.len())` |
| Missing error context | Logs show "failed" with no details | Include `err = ?e` and relevant identifiers |
| Inconsistent naming | Mix of snake_case, PascalCase, kebab-case | Standardize on snake_case for fields |
| No graceful shutdown | Spans lost on exit | Call `provider.shutdown()` before exit |
| Global meter without init | Metrics silently dropped | Initialize MeterProvider before using global meter |

---

## 10. System Metrics Semantic Conventions

OpenTelemetry defines standard metrics for host-level monitoring under the `system.*` namespace. Use these when instrumenting infrastructure metrics.

### CPU Metrics (`system.cpu.*`)

| Metric | Type | Unit | Description |
|--------|------|------|-------------|
| `system.cpu.time` | Counter | seconds | CPU time by mode |
| `system.cpu.utilization` | Gauge | 1 (ratio) | CPU utilization 0.0-1.0 |
| `system.cpu.physical.count` | UpDownCounter | {cpu} | Physical processor cores |
| `system.cpu.logical.count` | UpDownCounter | {cpu} | Logical processor cores |

**Key attribute:** `cpu.mode` - One of: `user`, `system`, `idle`, `interrupt`, `nice`, `softirq`, `steal`, `wait`

```rust
// Example: Recording CPU utilization
let cpu_utilization = meter
    .f64_gauge("system.cpu.utilization")
    .with_description("CPU utilization as ratio 0.0-1.0")
    .with_unit("1")
    .build();

cpu_utilization.record(0.75, &[KeyValue::new("cpu.mode", "user")]);
```

### Memory Metrics (`system.memory.*`)

| Metric | Type | Unit | Description |
|--------|------|------|-------------|
| `system.memory.usage` | UpDownCounter | By | Memory usage by state |
| `system.memory.limit` | UpDownCounter | By | Total available memory |
| `system.memory.utilization` | Gauge | 1 | Memory utilization ratio |

**Key attribute:** `system.memory.state` - One of: `used`, `free`, `cached`, `buffers`

```rust
let memory_usage = meter
    .i64_up_down_counter("system.memory.usage")
    .with_description("Memory usage in bytes")
    .with_unit("By")
    .build();

memory_usage.add(1024 * 1024 * 512, &[KeyValue::new("system.memory.state", "used")]);
```

### Disk Metrics (`system.disk.*`)

| Metric | Type | Unit | Description |
|--------|------|------|-------------|
| `system.disk.io` | Counter | By | Bytes read/written |
| `system.disk.operations` | Counter | {operation} | Read/write operation count |
| `system.disk.io_time` | Counter | s | Time disk was active |

**Key attributes:**
- `system.device` - Device identifier (e.g., `sda`, `nvme0n1`)
- `disk.io.direction` - `read` or `write`

### Network Metrics (`system.network.*`)

| Metric | Type | Unit | Description |
|--------|------|------|-------------|
| `system.network.io` | Counter | By | Bytes transmitted/received |
| `system.network.packets` | Counter | {packet} | Packets sent/received |
| `system.network.errors` | Counter | {error} | Network errors |
| `system.network.connections` | UpDownCounter | {connection} | Active connections |

**Key attributes:**
- `network.interface.name` - Interface name (e.g., `eth0`, `lo`)
- `network.io.direction` - `transmit` or `receive`
- `network.connection.state` - `established`, `time_wait`, `close_wait`, etc.

---

## 11. FaaS Spans Semantic Conventions

FaaS (Function as a Service) conventions apply to serverless-style invocations. In wasmCloud, component invocations are analogous to FaaS functions.

### General Span Attributes

| Attribute | Type | Required | Description |
|-----------|------|----------|-------------|
| `faas.trigger` | string | Yes (incoming) | What triggered the invocation |
| `faas.invocation_id` | string | No | Unique invocation identifier |
| `faas.coldstart` | boolean | No | True if first invocation (cold start) |
| `cloud.resource_id` | string | No | Cloud provider resource ID |

**Trigger types:** `http`, `pubsub`, `datasource`, `timer`, `other`

### Incoming Invocation (SERVER spans)

```rust
// Component receiving an invocation
let span = tracing::info_span!(
    "ComponentInvocation",
    faas.trigger = "http",
    faas.invocation_id = %invocation_id,
    faas.coldstart = is_first_invocation,
    otel.kind = "server"
);
```

### Outgoing Invocation (CLIENT spans)

| Attribute | Type | Description |
|-----------|------|-------------|
| `faas.invoked_name` | string | Name of the function being invoked |
| `faas.invoked_provider` | string | Cloud provider (`aws`, `azure`, `gcp`) |
| `faas.invoked_region` | string | Region of invoked function |

```rust
// Calling another component/function
let span = tracing::info_span!(
    "InvokeComponent",
    faas.invoked_name = %target_component,
    faas.invoked_provider = "wasmcloud",
    otel.kind = "client"
);
```

### Trigger-Specific Attributes

**Datasource triggers** (database, storage events):

| Attribute | Description |
|-----------|-------------|
| `faas.document.collection` | Source collection/bucket name |
| `faas.document.operation` | `insert`, `edit`, or `delete` |
| `faas.document.name` | Document/object identifier |

**Timer triggers** (scheduled invocations):

| Attribute | Description |
|-----------|-------------|
| `faas.cron` | Cron expression for schedule |
| `faas.time` | Scheduled invocation time (ISO 8601) |

### wasmCloud Application

Map FaaS concepts to wasmCloud:

| FaaS Concept | wasmCloud Equivalent |
|--------------|---------------------|
| Function | Component |
| Cold start | First component instantiation |
| Trigger | Capability provider event source |
| Invocation ID | Request/correlation ID |

```rust
// wasmCloud component invocation span
pub fn component_invocation_span(
    component_id: &str,
    operation: &str,
    trigger: &str,
    is_coldstart: bool,
) -> tracing::Span {
    tracing::span!(
        tracing::Level::INFO,
        "ComponentInvocation",
        component_id = component_id,
        faas.trigger = trigger,
        faas.coldstart = is_coldstart,
        otel.name = %format!("{component_id}/{operation}"),
    )
}
```

---

## 12. Metrics vs Tracing Decision Matrix

| Scenario | Tracing | Metrics | Both |
|----------|---------|---------|------|
| Request latency debugging | Yes | | |
| SLA monitoring (p99 latency) | | Yes | |
| Error rate alerting | | Yes | |
| Root cause analysis | Yes | | |
| Capacity planning | | Yes | |
| Performance regression detection | | | Yes |
| Distributed request flow | Yes | | |
| Dashboard counts/rates | | Yes | |

---

## 13. Wasmtime Runtime Metrics

Wasmtime provides runtime instrumentation capabilities for WebAssembly component execution. These metrics enable observability into fuel consumption, memory usage, execution timing, and instance lifecycle.

### Fuel Consumption Metrics

Fuel metering tracks instruction-level execution budgets for components.

| Metric | Type | Unit | Description |
|--------|------|------|-------------|
| `wasmtime.fuel.consumed` | Counter | {instruction} | Total fuel consumed by component |
| `wasmtime.fuel.remaining` | Gauge | {instruction} | Remaining fuel budget |

### Memory Metrics

Track WebAssembly linear memory usage per component.

| Metric | Type | Unit | Description |
|--------|------|------|-------------|
| `wasmtime.memory.usage` | Gauge | By | Current linear memory usage |
| `wasmtime.memory.peak` | Gauge | By | Peak memory during execution |
| `wasmtime.memory.limit` | Gauge | By | Configured memory limit |

### Execution Metrics

Measure component execution and instantiation performance.

| Metric | Type | Unit | Description |
|--------|------|------|-------------|
| `wasmtime.execution.duration` | Histogram | s | Component execution time |
| `wasmtime.instantiation.duration` | Histogram | s | Time to instantiate component |

### Instance Lifecycle Metrics

Track component instance creation and active counts.

| Metric | Type | Unit | Description |
|--------|------|------|-------------|
| `wasmtime.instances.active` | UpDownCounter | {instance} | Currently active instances |
| `wasmtime.instances.created` | Counter | {instance} | Total instances created |

### Key Attributes

Use these attributes to identify and correlate metrics:

| Attribute | Type | Description |
|-----------|------|-------------|
| `wasmtime.component.id` | string | Component identifier (e.g., hash or reference) |
| `wasmtime.component.name` | string | Human-readable component name |
| `wasmtime.store.id` | string | Wasmtime Store identifier |
| `wasmtime.epoch` | integer | Current epoch (if epoch interruption enabled) |

### 13.1 Enabling Fuel Metering

Configure wasmtime to track fuel consumption:

```rust
use wasmtime::{Config, Engine, Store};

fn create_fuel_enabled_engine() -> Engine {
    let mut config = Config::new();
    config.consume_fuel(true);
    Engine::new(&config).expect("failed to create engine")
}

fn create_store_with_fuel<T>(engine: &Engine, data: T, fuel_budget: u64) -> Store<T> {
    let mut store = Store::new(engine, data);
    store.set_fuel(fuel_budget).expect("failed to set fuel");
    store
}
```

### 13.2 Recording Fuel Metrics

Track fuel consumption after component execution:

```rust
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Gauge, Meter};
use wasmtime::Store;

struct WasmtimeMetrics {
    fuel_consumed: Counter<u64>,
    fuel_remaining: Gauge<u64>,
}

impl WasmtimeMetrics {
    fn new(meter: &Meter) -> Self {
        Self {
            fuel_consumed: meter
                .u64_counter("wasmtime.fuel.consumed")
                .with_description("Total fuel consumed by component")
                .with_unit("{instruction}")
                .build(),
            fuel_remaining: meter
                .u64_gauge("wasmtime.fuel.remaining")
                .with_description("Remaining fuel budget")
                .with_unit("{instruction}")
                .build(),
        }
    }

    fn record_fuel<T>(&self, store: &Store<T>, component_id: &str, initial_fuel: u64) {
        let remaining = store.get_fuel().unwrap_or(0);
        let consumed = initial_fuel.saturating_sub(remaining);

        let attrs = [
            KeyValue::new("wasmtime.component.id", component_id.to_string()),
        ];

        self.fuel_consumed.add(consumed, &attrs);
        self.fuel_remaining.record(remaining, &attrs);
    }
}
```

### 13.3 Memory Tracking

Monitor linear memory usage via Store methods:

```rust
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Gauge, Meter};
use wasmtime::{Memory, Store};

struct MemoryMetrics {
    memory_usage: Gauge<u64>,
    memory_peak: Gauge<u64>,
    memory_limit: Gauge<u64>,
}

impl MemoryMetrics {
    fn new(meter: &Meter) -> Self {
        Self {
            memory_usage: meter
                .u64_gauge("wasmtime.memory.usage")
                .with_description("Current linear memory usage")
                .with_unit("By")
                .build(),
            memory_peak: meter
                .u64_gauge("wasmtime.memory.peak")
                .with_description("Peak memory during execution")
                .with_unit("By")
                .build(),
            memory_limit: meter
                .u64_gauge("wasmtime.memory.limit")
                .with_description("Configured memory limit")
                .with_unit("By")
                .build(),
        }
    }

    fn record_memory<T>(&self, store: &Store<T>, memory: &Memory, component_id: &str) {
        let current_pages = memory.size(&store);
        let current_bytes = current_pages * wasmtime::WASM_PAGE_SIZE as u64;

        // Memory type contains max pages if configured
        let max_bytes = memory
            .ty(&store)
            .maximum()
            .map(|pages| pages * wasmtime::WASM_PAGE_SIZE as u64);

        let attrs = [
            KeyValue::new("wasmtime.component.id", component_id.to_string()),
        ];

        self.memory_usage.record(current_bytes, &attrs);
        if let Some(limit) = max_bytes {
            self.memory_limit.record(limit, &attrs);
        }
    }
}
```

### 13.4 Instrumenting Instantiation

Track component instantiation with spans and histograms:

```rust
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Histogram, Meter};
use std::time::Instant;
use tracing::{instrument, Instrument};
use wasmtime::{Component, Engine, Linker, Store};

struct InstantiationMetrics {
    instantiation_duration: Histogram<f64>,
}

impl InstantiationMetrics {
    fn new(meter: &Meter) -> Self {
        Self {
            instantiation_duration: meter
                .f64_histogram("wasmtime.instantiation.duration")
                .with_description("Time to instantiate component")
                .with_unit("s")
                .build(),
        }
    }
}

#[instrument(level = "debug", skip_all, fields(
    wasmtime.component.id = %component_id,
    wasmtime.component.name = %component_name
))]
async fn instantiate_component<T: Send>(
    engine: &Engine,
    linker: &Linker<T>,
    store: &mut Store<T>,
    component: &Component,
    component_id: &str,
    component_name: &str,
    metrics: &InstantiationMetrics,
) -> anyhow::Result<wasmtime::component::Instance> {
    let start = Instant::now();

    let instance = linker.instantiate(store, component)?;

    let duration = start.elapsed();
    let attrs = [
        KeyValue::new("wasmtime.component.id", component_id.to_string()),
        KeyValue::new("wasmtime.component.name", component_name.to_string()),
    ];
    metrics.instantiation_duration.record(duration.as_secs_f64(), &attrs);

    tracing::debug!(
        duration_ms = duration.as_millis(),
        "component instantiated"
    );

    Ok(instance)
}
```

### 13.5 Instance Lifecycle Tracking

Track active instances with UpDownCounter:

```rust
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter, UpDownCounter};
use std::sync::Arc;

struct InstanceMetrics {
    instances_active: UpDownCounter<i64>,
    instances_created: Counter<u64>,
}

impl InstanceMetrics {
    fn new(meter: &Meter) -> Self {
        Self {
            instances_active: meter
                .i64_up_down_counter("wasmtime.instances.active")
                .with_description("Currently active instances")
                .with_unit("{instance}")
                .build(),
            instances_created: meter
                .u64_counter("wasmtime.instances.created")
                .with_description("Total instances created")
                .with_unit("{instance}")
                .build(),
        }
    }

    fn on_instance_created(&self, component_id: &str) {
        let attrs = [KeyValue::new("wasmtime.component.id", component_id.to_string())];
        self.instances_created.add(1, &attrs);
        self.instances_active.add(1, &attrs);
    }

    fn on_instance_dropped(&self, component_id: &str) {
        let attrs = [KeyValue::new("wasmtime.component.id", component_id.to_string())];
        self.instances_active.add(-1, &attrs);
    }
}

/// RAII guard for tracking instance lifecycle
struct InstanceGuard {
    metrics: Arc<InstanceMetrics>,
    component_id: String,
}

impl InstanceGuard {
    fn new(metrics: Arc<InstanceMetrics>, component_id: String) -> Self {
        metrics.on_instance_created(&component_id);
        Self { metrics, component_id }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        self.metrics.on_instance_dropped(&self.component_id);
    }
}
```

### 13.6 Execution Duration Tracking

Record component execution time with histograms and spans:

```rust
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Histogram, Meter};
use std::time::Instant;
use tracing::instrument;

struct ExecutionMetrics {
    execution_duration: Histogram<f64>,
}

impl ExecutionMetrics {
    fn new(meter: &Meter) -> Self {
        Self {
            execution_duration: meter
                .f64_histogram("wasmtime.execution.duration")
                .with_description("Component execution time")
                .with_unit("s")
                .build(),
        }
    }
}

#[instrument(level = "debug", skip_all, fields(
    wasmtime.component.id = %component_id,
    operation = %operation
))]
fn execute_component_function<T, R>(
    store: &mut wasmtime::Store<T>,
    func: wasmtime::TypedFunc<(), R>,
    component_id: &str,
    operation: &str,
    metrics: &ExecutionMetrics,
) -> anyhow::Result<R>
where
    R: wasmtime::WasmResults,
{
    let start = Instant::now();

    let result = func.call(store, ())?;

    let duration = start.elapsed();
    let attrs = [
        KeyValue::new("wasmtime.component.id", component_id.to_string()),
        KeyValue::new("operation", operation.to_string()),
    ];
    metrics.execution_duration.record(duration.as_secs_f64(), &attrs);

    tracing::debug!(
        duration_ms = duration.as_millis(),
        "component function executed"
    );

    Ok(result)
}
```

### 13.7 Complete Wasmtime Metrics Integration

Combine all metrics into a unified struct:

```rust
use opentelemetry::metrics::Meter;
use std::sync::Arc;

/// Combined wasmtime runtime metrics
pub struct WasmtimeRuntimeMetrics {
    pub fuel: WasmtimeMetrics,
    pub memory: MemoryMetrics,
    pub instantiation: InstantiationMetrics,
    pub instances: Arc<InstanceMetrics>,
    pub execution: ExecutionMetrics,
}

impl WasmtimeRuntimeMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            fuel: WasmtimeMetrics::new(meter),
            memory: MemoryMetrics::new(meter),
            instantiation: InstantiationMetrics::new(meter),
            instances: Arc::new(InstanceMetrics::new(meter)),
            execution: ExecutionMetrics::new(meter),
        }
    }
}

// Usage
fn setup_metrics() -> WasmtimeRuntimeMetrics {
    let meter = opentelemetry::global::meter("wasmtime-runtime");
    WasmtimeRuntimeMetrics::new(&meter)
}
```

---

## 14. wash Runtime Observability Conventions

This section documents the standardized span names, metrics, and event conventions for the wash runtime. These conventions ensure consistent observability across all wash components.

### 14.1 Traces (Spans)

Use PascalCase for span names. Each category has defined span types:

#### Workload Lifecycle

| Span Name | Description | Key Attributes |
|-----------|-------------|----------------|
| `WorkloadStart` | Workload initialization and startup | `workload_id`, `trigger_type` |
| `WorkloadStop` | Graceful workload shutdown | `workload_id`, `shutdown_reason` |
| `WorkloadStateChange` | State transition events | `workload_id`, `previous_state`, `new_state` |

#### Component Execution

| Span Name | Description | Key Attributes |
|-----------|-------------|----------------|
| `ComponentInvocation` | Function call into a component | `component_id`, `function_name`, `caller_id` |
| `ComponentInstantiation` | Creating a new component instance | `component_id`, `is_coldstart` |

#### HTTP Handling

| Span Name | Description | Key Attributes |
|-----------|-------------|----------------|
| `HttpRequest` | Incoming HTTP request processing | `http.method`, `http.route`, `http.host` |
| `HttpResponse` | HTTP response completion | `http.status_code`, `http.response_size` |

#### Plugin Operations

| Span Name | Description | Key Attributes |
|-----------|-------------|----------------|
| `PluginBind` | Binding a plugin to a workload | `plugin_type`, `interface`, `workload_id` |
| `PluginOperation` | Plugin operation execution | `plugin_type`, `operation`, `success` |

### 14.2 Metrics

#### Counters

| Metric Name | Unit | Description |
|-------------|------|-------------|
| `workload.state_transitions` | `{transition}` | Total workload state changes |
| `component.invocations` | `{invocation}` | Total component function calls |
| `http.requests` | `{request}` | Total HTTP requests received |
| `plugin.operations` | `{operation}` | Total plugin operations executed |

#### Histograms

| Metric Name | Unit | Description |
|-------------|------|-------------|
| `component.invocation.duration` | `s` | Component function execution time |
| `http.request.duration` | `s` | HTTP request processing time |
| `component.instantiation.duration` | `s` | Time to instantiate a component |

#### Gauges

| Metric Name | Unit | Description |
|-------------|------|-------------|
| `workloads.active` | `{workload}` | Currently running workloads |
| `components.active` | `{component}` | Currently active component instances |
| `host.memory.used` | `By` | Host memory usage |
| `host.cpu.utilization` | `1` | Host CPU utilization (0.0-1.0) |

### 14.3 Events (within spans)

Events are recorded within spans to capture discrete occurrences. Use these conventions:

#### Error Events

Record errors with structured context:

```rust
tracing::error!(
    error.type = "ComponentTrap",
    error.message = %err,
    component_id = %id,
    function_name = %func,
    "component execution trapped"
);
```

Standard error attributes:
- `error.type` - Classification of the error (e.g., `ComponentTrap`, `PluginBindFailure`, `ValidationError`)
- `error.message` - Human-readable error description
- Context IDs (`workload_id`, `component_id`, `request_id`)

#### Warning Events

Emit warnings for recoverable issues:

```rust
tracing::warn!(
    workload_id = %id,
    current_memory = memory_bytes,
    memory_limit = limit_bytes,
    utilization_pct = (memory_bytes as f64 / limit_bytes as f64) * 100.0,
    "component approaching memory limit"
);
```

Common warning scenarios:
- Resource limit approaching (memory, fuel)
- Retry attempts with backoff info
- Deprecated configuration usage

#### Info Events

Record significant state changes and completions:

```rust
tracing::info!(
    workload_id = %id,
    previous_state = %old_state,
    new_state = %new_state,
    trigger = %trigger,
    "workload state transition"
);
```

Common info scenarios:
- State transitions (with before/after states)
- Successful initialization completions
- Plugin binding success

### 14.4 Attribute Naming Conventions

Follow these conventions for consistent attribute naming:

```rust
// Identifiers: use *_id suffix
workload_id, component_id, request_id, plugin_id

// Counts: use *_count suffix
retry_count, instance_count, error_count

// Durations: use *_ms suffix for milliseconds
latency_ms, timeout_ms, backoff_ms

// Sizes: use *_bytes suffix
memory_bytes, response_bytes, payload_bytes

// States: use descriptive names
previous_state, new_state, current_state

// Booleans: use is_* or has_* prefix
is_coldstart, is_success, has_error
```

### 14.5 Example: Instrumented Workload Lifecycle

```rust
use tracing::{instrument, info, error, warn};

#[instrument(
    level = "info",
    name = "WorkloadStart",
    skip_all,
    fields(workload_id = %id, trigger_type = %trigger)
)]
pub async fn start_workload(id: &str, trigger: &str) -> Result<()> {
    info!("initializing workload");

    // Initialization phases...

    info!(
        workload_id = %id,
        previous_state = "pending",
        new_state = "running",
        "workload state transition"
    );

    Ok(())
}

#[instrument(
    level = "info",
    name = "WorkloadStop",
    skip_all,
    fields(workload_id = %id, shutdown_reason = %reason)
)]
pub async fn stop_workload(id: &str, reason: &str) -> Result<()> {
    info!("initiating graceful shutdown");

    // Cleanup...

    info!(
        workload_id = %id,
        previous_state = "running",
        new_state = "stopped",
        shutdown_duration_ms = elapsed.as_millis(),
        "workload stopped"
    );

    Ok(())
}
```

---

## References

- [tracing-opentelemetry crate](https://crates.io/crates/tracing-opentelemetry)
- [OpenTelemetry Rust docs](https://opentelemetry.io/docs/languages/rust/)
- [System Metrics Semantic Conventions](https://opentelemetry.io/docs/specs/semconv/system/system-metrics/)
- [FaaS Spans Semantic Conventions](https://opentelemetry.io/docs/specs/semconv/faas/faas-spans/)
- [Datadog: Monitor Rust with OpenTelemetry](https://www.datadoghq.com/blog/monitor-rust-otel/)
- [opentelemetry-rust GitHub](https://github.com/open-telemetry/opentelemetry-rust)
- [Wasmtime Fuel Metering](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.consume_fuel)
- [Wasmtime Epoch Interruption](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.epoch_interruption)
- [Wasmtime Memory API](https://docs.wasmtime.dev/api/wasmtime/struct.Memory.html)
