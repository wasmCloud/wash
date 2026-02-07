mod conversions;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use futures::TryStreamExt;
use tokio::sync::RwLock;
use tokio_postgres::types::Type as PgType;
use tracing::instrument;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};

use conversions::into_result_row;

const PLUGIN_POSTGRES_ID: &str = "wasmcloud-postgres";
const DEFAULT_POOL_SIZE: usize = 10;

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "postgres",
        imports: { default: async | trappable | tracing },
    });
}

use bindings::wasmcloud::postgres::prepared;
use bindings::wasmcloud::postgres::query;
use bindings::wasmcloud::postgres::types;

use query::{PgValue, QueryError};

use prepared::{PreparedStatementExecError, StatementPrepareError};

/// A prepared statement entry stored by the plugin.
/// Contains the original SQL, the inferred parameter types, and the database name.
struct PreparedEntry {
    sql: String,
    param_types: Vec<PgType>,
    database: String,
}

/// wasmcloud:postgres host plugin.
///
/// Manages postgres connection pools at the host level and routes workload
/// queries to the appropriate database via a connection bouncer pattern.
#[derive(Clone)]
pub struct WasmcloudPostgres {
    /// Base config parsed from URL (no dbname set)
    base_config: tokio_postgres::Config,
    /// Max pool size per database
    pool_size: usize,
    /// Whether TLS should be used for connections
    tls: bool,
    /// database_name -> Pool
    pools: Arc<RwLock<HashMap<String, Pool>>>,
    /// prepared_statement_token -> PreparedEntry
    prepared_statements: Arc<RwLock<HashMap<String, PreparedEntry>>>,
    /// component_id -> database_name
    component_databases: Arc<RwLock<HashMap<String, String>>>,
}

impl WasmcloudPostgres {
    /// Create a new WasmcloudPostgres plugin from a postgres URL.
    ///
    /// The URL should contain credentials, host/port, sslmode, and optionally pool_size.
    /// The database name should NOT be included - workloads provide it via config.
    ///
    /// Example: `postgres://user:pass@bouncer:6432?sslmode=require&pool_size=10`
    pub fn new(url: &str) -> anyhow::Result<Self> {
        let mut config: tokio_postgres::Config =
            url.parse().context("failed to parse postgres URL")?;

        // Extract pool_size from the URL (not a standard postgres param, we parse it ourselves)
        let pool_size = extract_pool_size(url);

        // Determine TLS from sslmode
        let tls = extract_tls_requirement(url);

        // Strip dbname from the base config - workloads set this via their config
        config.dbname("");

        Ok(Self {
            base_config: config,
            pool_size,
            tls,
            pools: Arc::new(RwLock::new(HashMap::new())),
            prepared_statements: Arc::new(RwLock::new(HashMap::new())),
            component_databases: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Get or lazily create a connection pool for the given database name.
    async fn get_or_create_pool(&self, database: &str) -> anyhow::Result<Pool> {
        // Fast path: pool already exists
        {
            let pools = self.pools.read().await;
            if let Some(pool) = pools.get(database) {
                return Ok(pool.clone());
            }
        }

        // Slow path: create pool
        let mut pools = self.pools.write().await;
        // Double-check after acquiring write lock
        if let Some(pool) = pools.get(database) {
            return Ok(pool.clone());
        }

        let mut pg_config = self.base_config.clone();
        pg_config.dbname(database);

        let mgr_config = ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        };

        let pool = if self.tls {
            let tls_config = rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore {
                    roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
                })
                .with_no_client_auth();
            let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
            let mgr = Manager::from_config(pg_config, tls, mgr_config);
            Pool::builder(mgr)
                .max_size(self.pool_size)
                .build()
                .context("failed to build TLS connection pool")?
        } else {
            let mgr = Manager::from_config(pg_config, tokio_postgres::NoTls, mgr_config);
            Pool::builder(mgr)
                .max_size(self.pool_size)
                .build()
                .context("failed to build connection pool")?
        };

        pools.insert(database.to_string(), pool.clone());
        Ok(pool)
    }

    /// Look up the database name for a component.
    async fn database_for_component(&self, component_id: &str) -> Option<String> {
        self.component_databases
            .read()
            .await
            .get(component_id)
            .cloned()
    }
}

/// Extract a query parameter value from a URL string.
fn extract_query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let query_start = url.find('?')?;
    let query = &url[query_start + 1..];
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(v);
        }
    }
    None
}

/// Extract pool_size from URL query params. Returns default if not found.
fn extract_pool_size(url: &str) -> usize {
    extract_query_param(url, "pool_size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_POOL_SIZE)
}

/// Determine whether TLS is required based on the sslmode URL parameter.
fn extract_tls_requirement(url: &str) -> bool {
    extract_query_param(url, "sslmode")
        .map(|v| matches!(v, "require" | "verify-ca" | "verify-full"))
        .unwrap_or(false)
}

// ── Host trait implementations ──────────────────────────────────────────────

impl<'a> types::Host for ActiveCtx<'a> {}

impl<'a> query::Host for ActiveCtx<'a> {
    #[instrument(skip_all, fields(query = %q))]
    async fn query(
        &mut self,
        q: String,
        params: Vec<PgValue>,
    ) -> anyhow::Result<Result<Vec<query::ResultRow>, QueryError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID) else {
            return Ok(Err(QueryError::Unexpected(
                "postgres plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let database = match plugin.database_for_component(&component_id).await {
            Some(db) => db,
            None => {
                return Ok(Err(QueryError::Unexpected(
                    "no database configured for this component".to_string(),
                )));
            }
        };

        let pool = match plugin.get_or_create_pool(&database).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to get connection pool: {e}"
                ))));
            }
        };

        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to get connection: {e}"
                ))));
            }
        };

        let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();

        let rows = match client.query_raw(&q, param_refs).await {
            Ok(stream) => match stream.try_collect::<Vec<_>>().await {
                Ok(rows) => rows,
                Err(e) => {
                    return Ok(Err(QueryError::Unexpected(format!(
                        "failed to collect rows: {e}"
                    ))));
                }
            },
            Err(e) => {
                return Ok(Err(QueryError::InvalidQuery(format!(
                    "query execution failed: {e}"
                ))));
            }
        };

        let result: Vec<query::ResultRow> = rows.into_iter().map(into_result_row).collect();
        Ok(Ok(result))
    }

    #[instrument(skip_all, fields(query = %q))]
    async fn query_batch(&mut self, q: String) -> anyhow::Result<Result<(), QueryError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID) else {
            return Ok(Err(QueryError::Unexpected(
                "postgres plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let database = match plugin.database_for_component(&component_id).await {
            Some(db) => db,
            None => {
                return Ok(Err(QueryError::Unexpected(
                    "no database configured for this component".to_string(),
                )));
            }
        };

        let pool = match plugin.get_or_create_pool(&database).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to get connection pool: {e}"
                ))));
            }
        };

        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to get connection: {e}"
                ))));
            }
        };

        match client.batch_execute(&q).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(QueryError::InvalidQuery(format!(
                "batch execution failed: {e}"
            )))),
        }
    }
}

impl<'a> prepared::Host for ActiveCtx<'a> {
    #[instrument(skip_all)]
    async fn prepare(
        &mut self,
        statement: String,
    ) -> anyhow::Result<Result<String, StatementPrepareError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID) else {
            return Ok(Err(StatementPrepareError::Unexpected(
                "postgres plugin not available".to_string(),
            )));
        };

        let component_id = self.component_id.to_string();
        let database = match plugin.database_for_component(&component_id).await {
            Some(db) => db,
            None => {
                return Ok(Err(StatementPrepareError::Unexpected(
                    "no database configured for this component".to_string(),
                )));
            }
        };

        let pool = match plugin.get_or_create_pool(&database).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(StatementPrepareError::Unexpected(format!(
                    "failed to get connection pool: {e}"
                ))));
            }
        };

        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(StatementPrepareError::Unexpected(format!(
                    "failed to get connection: {e}"
                ))));
            }
        };

        let stmt = match client.prepare(&statement).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(StatementPrepareError::Unexpected(format!(
                    "prepare failed: {e}"
                ))));
            }
        };

        let token = ulid::Ulid::new().to_string();
        let param_types = stmt.params().to_vec();

        plugin.prepared_statements.write().await.insert(
            token.clone(),
            PreparedEntry {
                sql: statement,
                param_types,
                database,
            },
        );

        Ok(Ok(token))
    }

    #[instrument(skip_all, fields(stmt_token = %stmt_token))]
    async fn exec(
        &mut self,
        stmt_token: String,
        params: Vec<PgValue>,
    ) -> anyhow::Result<Result<u64, PreparedStatementExecError>> {
        let Some(plugin) = self.get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID) else {
            return Ok(Err(PreparedStatementExecError::Unexpected(
                "postgres plugin not available".to_string(),
            )));
        };

        let entry = {
            let stmts = plugin.prepared_statements.read().await;
            match stmts.get(&stmt_token) {
                Some(entry) => PreparedEntry {
                    sql: entry.sql.clone(),
                    param_types: entry.param_types.clone(),
                    database: entry.database.clone(),
                },
                None => return Ok(Err(PreparedStatementExecError::UnknownPreparedQuery)),
            }
        };

        let pool = match plugin.get_or_create_pool(&entry.database).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(PreparedStatementExecError::Unexpected(format!(
                    "failed to get connection pool: {e}"
                ))));
            }
        };

        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(PreparedStatementExecError::Unexpected(format!(
                    "failed to get connection: {e}"
                ))));
            }
        };

        // Re-prepare via statement cache (deadpool-postgres caches these per connection)
        let stmt = match client.prepare_typed(&entry.sql, &entry.param_types).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(PreparedStatementExecError::Unexpected(format!(
                    "re-prepare failed: {e}"
                ))));
            }
        };

        let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();

        match client.execute_raw(&stmt, param_refs).await {
            Ok(n) => Ok(Ok(n)),
            Err(e) => Ok(Err(PreparedStatementExecError::QueryError(
                QueryError::Unexpected(format!("execute failed: {e}")),
            ))),
        }
    }
}

// ── HostPlugin implementation ───────────────────────────────────────────────

#[async_trait::async_trait]
impl HostPlugin for WasmcloudPostgres {
    fn id(&self) -> &'static str {
        PLUGIN_POSTGRES_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:postgres/types,query,prepared@0.1.1-draft",
            )]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        let has_postgres = interfaces
            .iter()
            .any(|i| i.namespace == "wasmcloud" && i.package == "postgres");

        if !has_postgres {
            return Ok(());
        }

        // Extract database name from interface config
        let database = interfaces
            .iter()
            .find(|i| i.namespace == "wasmcloud" && i.package == "postgres")
            .and_then(|i| i.config.get("database").cloned());

        let database = match database {
            Some(db) => db,
            None => {
                bail!("wasmcloud:postgres requires a 'database' config parameter");
            }
        };

        let component_id = component_handle.id().to_string();

        tracing::debug!(
            component_id = %component_id,
            database = %database,
            "Binding postgres plugin to component"
        );

        // Store the component → database mapping
        self.component_databases
            .write()
            .await
            .insert(component_id, database);

        // Add linker functions
        let linker = component_handle.linker();
        bindings::wasmcloud::postgres::types::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasmcloud::postgres::query::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasmcloud::postgres::prepared::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        tracing::debug!(workload_id = %workload_id, "Unbinding postgres plugin from workload");

        // Remove component → database mappings for this workload
        // and clean up prepared statements that belong to those components
        let mut component_databases = self.component_databases.write().await;
        let removed_components: Vec<String> = component_databases
            .iter()
            .filter(|(k, _)| k.starts_with(workload_id))
            .map(|(k, _)| k.clone())
            .collect();

        let removed_databases: HashSet<String> = removed_components
            .iter()
            .filter_map(|c| component_databases.remove(c))
            .collect();

        // Clean up prepared statements for removed databases
        if !removed_databases.is_empty() {
            let mut prepared = self.prepared_statements.write().await;
            prepared.retain(|_, entry| !removed_databases.contains(&entry.database));
        }

        Ok(())
    }
}
