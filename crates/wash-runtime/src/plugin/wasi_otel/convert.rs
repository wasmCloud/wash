//! Conversion utilities for WASI OpenTelemetry types

use opentelemetry::logs::{AnyValue, LogRecord as OtelLogRecord};
use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId};
use opentelemetry::Key;
use std::time::{Duration, UNIX_EPOCH};

use super::bindings::wasi::otel::logs::LogRecord as WasiLogRecord;
use super::bindings::wasi::otel::tracing::{SpanContext as WitSpanContext, TraceFlags as WitTraceFlags};

/// Convert OTel span context to WIT
pub fn otel_span_context_to_wit(ctx: &SpanContext) -> WitSpanContext {
    WitSpanContext {
        trace_id: format!("{:032x}", ctx.trace_id()),
        span_id: format!("{:016x}", ctx.span_id()),
        trace_flags: if ctx.is_sampled() {
            WitTraceFlags::SAMPLED
        } else {
            WitTraceFlags::empty()
        },
        is_remote: ctx.is_remote(),
        trace_state: vec![],
    }
}

/// Converts a WASI OTEL LogRecord to populate an OpenTelemetry LogRecord
pub fn convert_wasi_log_record<R: OtelLogRecord>(wasi_record: WasiLogRecord, otel_record: &mut R) {
    use opentelemetry::logs::Severity;

    otel_record.add_attribute(
        Key::new("resource.service.name"),
        AnyValue::String("wasi-otel".into()),
    );

    // Set timestamp
    if let Some(ts) = wasi_record.timestamp {
        let duration = Duration::new(ts.seconds, ts.nanoseconds);
        if let Some(time) = UNIX_EPOCH.checked_add(duration) {
            otel_record.set_timestamp(time);
        }
    }

    // Set observed timestamp
    if let Some(ts) = wasi_record.observed_timestamp {
        let duration = Duration::new(ts.seconds, ts.nanoseconds);
        if let Some(time) = UNIX_EPOCH.checked_add(duration) {
            otel_record.set_observed_timestamp(time);
        }
    }

    // Set severity number (map u8 to Severity enum)
    if let Some(severity_num) = wasi_record.severity_number {
        let severity = match severity_num {
            1..=4 => Severity::Trace,
            5..=8 => Severity::Debug,
            9..=12 => Severity::Info,
            13..=16 => Severity::Warn,
            17..=20 => Severity::Error,
            21..=24 => Severity::Fatal,
            _ => Severity::Info, // Default fallback
        };
        otel_record.set_severity_number(severity);
    }

    // Set body (value is a JSON-encoded string in WASI OTEL)
    if let Some(ref body) = wasi_record.body {
        otel_record.set_body(AnyValue::String(body.clone().into()));
    }

    // Set attributes
    if let Some(ref attributes) = wasi_record.attributes {
        for kv in attributes {
            // The value in WASI OTEL is a JSON-encoded string
            otel_record.add_attribute(
                Key::new(kv.key.clone()),
                AnyValue::String(kv.value.clone().into()),
            );
        }
    }

    // Set trace context (trace_id, span_id, trace_flags)
    if wasi_record.trace_id.is_some() || wasi_record.span_id.is_some() {
        let trace_id = wasi_record
            .trace_id
            .as_ref()
            .and_then(|id| TraceId::from_hex(id).ok())
            .unwrap_or(TraceId::INVALID);

        let span_id = wasi_record
            .span_id
            .as_ref()
            .and_then(|id| SpanId::from_hex(id).ok())
            .unwrap_or(SpanId::INVALID);

        let flags = wasi_record
            .trace_flags
            .map(|f| {
                if f.contains(super::bindings::wasi::otel::tracing::TraceFlags::SAMPLED) {
                    TraceFlags::SAMPLED
                } else {
                    TraceFlags::default()
                }
            })
            .unwrap_or_default();

        otel_record.set_trace_context(trace_id, span_id, Some(flags));
    }

    // Note: resource and instrumentation_scope from the WASI record are typically
    // handled at the Logger/LoggerProvider level in OpenTelemetry, not on individual records.
    // If needed, they can be added as attributes:
    if let Some(ref resource) = wasi_record.resource {
        for kv in &resource.attributes {
            otel_record.add_attribute(
                Key::new(format!("resource.{}", kv.key)),
                AnyValue::String(kv.value.clone().into()),
            );
        }
    }

    if let Some(ref scope) = wasi_record.instrumentation_scope {
        otel_record.add_attribute(
            Key::new("instrumentation_scope.name"),
            AnyValue::String(scope.name.clone().into()),
        );
        if let Some(ref version) = scope.version {
            otel_record.add_attribute(
                Key::new("instrumentation_scope.version"),
                AnyValue::String(version.clone().into()),
            );
        }
    }
}
