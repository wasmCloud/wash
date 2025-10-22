use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tracing::debug;

/// A wrapper around [`System`] to provide a higher-level API for monitoring system resources.
#[derive(Debug, Default)]
pub struct SystemMonitor {
    system: System,
}

impl SystemMonitor {
    /// Creates a new [`SystemMonitor`] instance.
    pub fn new() -> Self {
        let system = System::new_with_specifics(
            RefreshKind::new()
                .with_memory(MemoryRefreshKind::everything())
                .with_cpu(CpuRefreshKind::everything()),
        );
        Self { system }
    }

    /// Refreshes the system information.
    pub fn refresh(&mut self) {
        self.system.refresh_memory();
        self.system.refresh_cpu_all();
    }

    /// Returns the current memory usage statistics as [`MemoryUsage`]
    pub fn memory_usage(&self) -> MemoryUsage {
        MemoryUsage {
            total_memory: self.system.total_memory(),
            used_memory: self.system.used_memory(),
            available_memory: self.system.available_memory(),
            free_memory: self.system.free_memory(),
        }
    }

    /// Returns the current CPU usage statistics as [`CpuUsage`]
    pub fn cpu_usage(&self) -> CpuUsage {
        let global_cpu = self.system.global_cpu_usage();

        CpuUsage {
            global_usage: global_cpu,
            cpu_count: self.system.cpus().len(),
        }
    }

    /// Reports the current system resource usage as a debug level trace event.
    pub fn report_usage(&self) {
        let mem = self.memory_usage();
        let cpu = self.cpu_usage();

        debug!(
            total_memory_mb = mem.total_memory / 1_048_576,
            used_memory_mb = mem.used_memory / 1_048_576,
            available_memory_mb = mem.available_memory / 1_048_576,
            memory_usage_percent = (mem.used_memory as f64 / mem.total_memory as f64 * 100.0),
            "Memory usage"
        );

        debug!(
            global_cpu_usage = cpu.global_usage,
            cpu_count = cpu.cpu_count,
            "CPU usage"
        );
    }
}

/// Represents the current memory usage statistics.
#[derive(Debug, Clone)]
pub struct MemoryUsage {
    pub total_memory: u64,
    pub used_memory: u64,
    pub available_memory: u64,
    pub free_memory: u64,
}

/// Represents the current CPU usage statistics.
#[derive(Debug, Clone)]
pub struct CpuUsage {
    pub global_usage: f32,
    pub cpu_count: usize,
}
