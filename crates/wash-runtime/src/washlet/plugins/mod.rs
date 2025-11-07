pub mod wasi_blobstore;
pub mod wasi_config;
pub mod wasi_http;
pub mod wasi_keyvalue;
pub mod wasi_logging;
pub mod wasmcloud_messaging;

use std::collections::HashMap;
use std::future::Future;

use crate::engine::workload::{UnresolvedWorkload, WorkloadComponent};

/// A tracker for workloads and their components, allowing storage of associated
/// data.
/// The tracker maintains a mapping of workload IDs to their data and
/// components, as well as a mapping of component IDs to their parent workload
/// IDs.
pub struct WorkloadTracker<T, Y> {
    pub workloads: HashMap<String, WorkloadTrackerItem<T, Y>>,
    pub components: HashMap<String, String>,
}

#[derive(Default)]
pub struct WorkloadTrackerItem<T, Y> {
    pub workload_data: Option<T>,
    pub components: HashMap<String, Y>,
}

impl<T, Y> Default for WorkloadTracker<T, Y> {
    fn default() -> Self {
        Self {
            workloads: HashMap::new(),
            components: HashMap::new(),
        }
    }
}

// TODO(lxf): remove once plugins have migrated to use this.
#[allow(dead_code)]
impl<T, Y> WorkloadTracker<T, Y> {
    pub fn add_unresolved_workload(&mut self, workload: &UnresolvedWorkload, data: T) {
        self.workloads.insert(
            workload.id().to_string(),
            WorkloadTrackerItem {
                workload_data: Some(data),
                components: HashMap::new(),
            },
        );
    }

    pub async fn remove_workload(&mut self, workload_id: &str) {
        if let Some(item) = self.workloads.remove(workload_id) {
            for component_id in item.components.keys() {
                self.components.remove(component_id);
            }
        }
    }

    pub async fn remove_workload_with_cleanup<
        FutW: Future<Output = ()>,
        FutC: Future<Output = ()>,
    >(
        &mut self,
        workload_id: &str,
        workload_cleanup: impl FnOnce(Option<T>) -> FutW,
        component_cleanup: impl Fn(Y) -> FutC,
    ) {
        if let Some(item) = self.workloads.remove(workload_id) {
            for (component_id, component_data) in item.components {
                component_cleanup(component_data).await;
                self.components.remove(&component_id);
            }
            workload_cleanup(item.workload_data).await;
        }
    }

    pub fn add_component(&mut self, workload_component: &WorkloadComponent, data: Y) {
        let component_id = workload_component.id();
        let workload_id = workload_component.workload_id();
        let item = self
            .workloads
            .entry(workload_id.to_string())
            .or_insert_with(|| WorkloadTrackerItem {
                workload_data: None,
                components: HashMap::new(),
            });
        item.components.insert(component_id.to_string(), data);
        self.components
            .insert(component_id.to_string(), workload_id.to_string());
    }

    pub fn get_workload_data(&self, workload_id: &str) -> Option<&T> {
        let item = self.workloads.get(workload_id)?;
        item.workload_data.as_ref()
    }

    pub fn get_component_data(&self, component_id: &str) -> Option<&Y> {
        let workload_id = self.components.get(component_id)?;
        let item = self.workloads.get(workload_id)?;
        item.components.get(component_id)
    }
}
