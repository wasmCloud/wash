//! # WASI OpenTelemetry Plugin
//! This module implements an OpenTelemetry plugin for the wasmCloud runtime,
//! providing the `wasi:otel@0.0.1` interfaces.

mod convert;

pub use convert::otel_span_context_to_wit;
use convert::convert_wasi_log_record;

use anyhow::bail;
use opentelemetry::logs::{Logger, LoggerProvider};

use std::sync::Arc;
use std::collections::HashSet;
use tokio::sync::RwLock;
use opentelemetry::trace::SpanContext;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLoggerProvider};

use opentelemetry_otlp::{LogExporter, MetricExporter}; //, WithExportConfig};

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::plugin::{HostPlugin,WorkloadItem,WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};



const WASI_OTEL_ID: &str = "wasi-otel";

mod bindings {
    wasmtime::component::bindgen!({
        world: "otel",
        imports: { default: async | trappable },
    });
}

use bindings::wasi::otel::tracing::{SpanContext as WitSpanContext, TraceFlags as WitTraceFlags};

/// Plugin configuration
#[derive(Clone, Debug)]
pub struct WasiOtelConfig {
    pub endpoint: String,
    pub protocol: String,
    pub service_name: String,
    pub propagate_context: bool,
    pub batch_timeout_ms: u64,
}

impl Default for WasiOtelConfig {
    fn default() -> Self {
        Self {
            endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:5318".to_string()),
            protocol: std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
                .unwrap_or_else(|_| "grpc".to_string()),
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "wash-component".to_string()),
            propagate_context: true,
            batch_timeout_ms: 5000,
        }
    }
}

/// Per-component context tracking
#[allow(dead_code)]
struct ComponentContext {
    component_id: String,
    workload_name: String,
    /// Current span context for this component's execution
    current_span_context: Option<SpanContext>,
}

/// WASI OpenTelemetry Plugin
#[derive(Default)]
pub struct WasiOtel {
    config: WasiOtelConfig,
    tracker: Arc<RwLock<WorkloadTracker<(),ComponentContext>>>,
    /// Meter provider for metrics export
    meter_provider: Arc<RwLock<Option<SdkMeterProvider>>>,
    tracer_provider: Arc<RwLock<Option<SdkTracerProvider>>>,
    logger_provider: Arc<RwLock<Option<SdkLoggerProvider>>>,
}

#[async_trait::async_trait]
impl HostPlugin for WasiOtel {
    fn id(&self) -> &'static str {
        WASI_OTEL_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:otel/types,tracing,metrics,logs@0.2.0-rc.1",
            )]),
            ..Default::default()
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        tracing::info!(
            endpoint = %self.config.endpoint,
            protocol = %self.config.protocol,
            "Starting WASI OTel plugin"
        );

        // TODO: Add configurable endpoints/protocols to use. This would be beneficial for when you want to have Host otel go to Platform engineering teams,
        // And Workload otel go to a different backend for application monitoring.

        // TODO: tracer exporter

        // set up the grpc log exporter
        let log_exporter = LogExporter::builder()
            .with_tonic()
            //.with_endpoint("http://localhost:5318")
            //.with_protocol(opentelemetry_otlp::Protocol::Grpc)
            .build()?;

        // set up metric exporter
        let metric_exporter = MetricExporter::builder()
            .with_tonic()
            //.with_endpoint("http://localhost:5318")
            //.with_protocol(opentelemetry_otlp::Protocol::Grpc)
            .build()
            .expect("Failed to create metric exporter");

        // processor
        let processor = BatchLogProcessor::builder(log_exporter)
            .build();


        // Initialize all providers
        let tracer_provider = opentelemetry_sdk::trace::TracerProviderBuilder::default()
            .build();
        let logger_provider = opentelemetry_sdk::logs::LoggerProviderBuilder::default()
            .with_log_processor(processor)
            .with_resource(
                opentelemetry_sdk::Resource::builder_empty()
                    .with_attributes([KeyValue::new("service.name","wasi-otel")])
                    .build(),
            )
            .build();
        let meter_provider = SdkMeterProvider::builder()
            .with_periodic_exporter(metric_exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder_empty()
                    .with_attributes([KeyValue::new("service.name","wasi-otel")])
                    .build(),
            )
            .build();

        *self.tracer_provider.write().await = Some(tracer_provider);
        *self.logger_provider.write().await = Some(logger_provider);
        *self.meter_provider.write().await = Some(meter_provider);

        tracing::info!("WASI OTel plugin started");
        Ok(())
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        _interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        // Add all wasi:otel interfaces to linker
        bindings::wasi::otel::types::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasi::otel::tracing::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasi::otel::metrics::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasi::otel::logs::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;

        // Register component context for tracking
        let ctx = ComponentContext {
            component_id: component_handle.id().to_string(),
            workload_name: component_handle.workload_name().to_string(),
            current_span_context: None,
        };

        let WorkloadItem::Component(component_handle) = component_handle else {
            bail!("Service can not be tracked");
        };

        self.tracker.write().await
            .add_component(component_handle, ctx);

        tracing::debug!(
            component_id = component_handle.id(),
            "WASI OTel interfaces bound to component"
        );
        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        self.tracker.write().await
            .remove_workload(workload_id)
            .await;
        tracing::debug!(workload_id, "WASI OTel unbound from workload");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        tracing::info!("Stopping WASI OTel plugin");

        // Flush and shutdown all providers
        if let Some(provider) = self.tracer_provider.write().await.take() {
            let _ = provider.force_flush();
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.logger_provider.write().await.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.meter_provider.write().await.take() {
            let _ = provider.shutdown();
        }

        tracing::info!("WASI OTel plugin stopped");
        Ok(())
    }
}

// OTel Logs
impl<'a> bindings::wasi::otel::logs::Host for ActiveCtx<'a> {
    async fn on_emit(&mut self, data: bindings::wasi::otel::logs::LogRecord) -> wasmtime::Result<()> {
        tracing::debug!(?data, "emitting log record");
        if let Some(plugin) = self.ctx.get_plugin::<WasiOtel>(WASI_OTEL_ID) {
            let provider = plugin.logger_provider.read().await;

            if let Some(ref provider) = *provider {
                let logger = provider.logger("wasi-otel");
                let mut otel_record = logger.create_log_record();
                convert_wasi_log_record(data, &mut otel_record);
                logger.emit(otel_record);
            }
        }
        Ok(())
    }
}

// OTel Metrics
impl < 'a> bindings::wasi::otel::metrics::Host for ActiveCtx<'a> {
    async  fn export(&mut self,resource_log:bindings::wasi::otel::metrics::ResourceMetrics,) -> wasmtime::Result<Result<(),bindings::wasi::otel::metrics::Error>> {
        if let Some(plugin) = self.ctx.get_plugin::<WasiOtel>(WASI_OTEL_ID) {
            tracing::debug!("Passing resource metrics to meter provider: {:?}", resource_log);

            if let Some(meter_provider) = plugin.meter_provider.write().await.as_ref() {
                let _ = meter_provider.force_flush();
            }
        }

        Ok(Ok(()))
    }
}

// OTel Tracing
impl<'a> bindings::wasi::otel::tracing::Host for ActiveCtx<'a> {
    async fn on_start(&mut self, span_context: bindings::wasi::otel::tracing::SpanContext) -> wasmtime::Result<()> {
        tracing::debug!(?span_context, "starting span");
        Ok(())
    }

    async fn on_end(&mut self, span_data: bindings::wasi::otel::tracing::SpanData) -> wasmtime::Result<()> {
        tracing::debug!(?span_data, "ending span");
        Ok(())
    }

    async fn outer_span_context(&mut self) -> wasmtime::Result<WitSpanContext> {
        tracing::debug!("retrieving outer span context");
        Ok(WitSpanContext {
            trace_id: String::new(),
            span_id: String::new(),
            trace_flags: WitTraceFlags::empty(),
            is_remote: false,
            trace_state: vec![],
        })
    }
}

impl<'a> bindings::wasi::otel::types::Host for ActiveCtx<'a> {

}
