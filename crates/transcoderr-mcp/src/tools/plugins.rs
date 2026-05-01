use crate::Server;
use rmcp::{
    handler::server::{tool::Parameters, wrapper::Json},
    model::{ErrorCode, ErrorData},
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future; // pulled in by #[tool_router] macro expansion

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PluginIdArgs {
    pub id: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct InstallPluginArgs {
    /// Catalog the entry comes from (see `browse_plugins.entries[].catalog_id`).
    pub catalog_id: i64,
    /// Plugin name (see `browse_plugins.entries[].name`).
    pub name: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UninstallPluginArgs {
    pub id: i64,
    /// Required confirmation. Reject the call by setting this to false.
    pub confirm: bool,
}

#[tool_router(router = plugins_router, vis = "pub")]
impl Server {
    #[tool(
        name = "list_plugins",
        description = "List installed plugins. Each row carries `id`, `name`, `version`, `kind`, the `provides_steps` the plugin registers, and `catalog_id`/`tarball_sha256` (NULL if installed manually rather than from a catalog)."
    )]
    pub async fn list_plugins(
        &self,
        _: Parameters<super::NoArgs>,
    ) -> Result<Json<Vec<serde_json::Value>>, ErrorData> {
        self.api
            .get::<Vec<serde_json::Value>>("/api/plugins")
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "get_plugin",
        description = "Get full detail for one installed plugin: manifest fields (provides_steps, capabilities, requires, summary, min_transcoderr_version), schema, on-disk path, and the verbatim README.md if present."
    )]
    pub async fn get_plugin(
        &self,
        Parameters(a): Parameters<PluginIdArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api
            .get::<serde_json::Value>(&format!("/api/plugins/{}", a.id))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "browse_plugins",
        description = "List every plugin available across all configured catalogs. Returns `{entries, errors}` where `entries[]` describes installable plugins (with `catalog_id`, `name`, `version`, `summary`, `provides_steps`, etc.) and `errors[]` names any catalogs that failed to fetch. Use the `catalog_id` + `name` from an entry to call `install_plugin`."
    )]
    pub async fn browse_plugins(
        &self,
        _: Parameters<super::NoArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api
            .get::<serde_json::Value>("/api/plugin-catalog-entries")
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "install_plugin",
        description = "Install a plugin from a catalog. Side effect: downloads the tarball, sha256-verifies it, atomically swaps it into `{data_dir}/plugins/<name>/`, and live-reloads the step registry so the plugin's steps become dispatchable without a server restart. Use `browse_plugins` first to discover available `(catalog_id, name)` pairs."
    )]
    pub async fn install_plugin(
        &self,
        Parameters(a): Parameters<InstallPluginArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        // Plugin names from a catalog are kebab-case alphanum (validated
        // by publish.py), so we don't URL-encode here. A name with
        // path-separator chars would fail at the server's route match
        // with a clear 404, not silently install the wrong plugin.
        let path = format!(
            "/api/plugin-catalog-entries/{}/{}/install",
            a.catalog_id, a.name
        );
        self.api
            .post::<serde_json::Value, _>(&path, &serde_json::Value::Null)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "uninstall_plugin",
        description = "Destructive: permanently delete an installed plugin -- removes its directory from `{data_dir}/plugins/` and drops its DB row. Flows that reference the plugin's steps will fail to dispatch until the plugin is reinstalled. Cannot be undone. Requires `confirm: true`."
    )]
    pub async fn uninstall_plugin(
        &self,
        Parameters(a): Parameters<UninstallPluginArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        if !a.confirm {
            return Err(ErrorData {
                code: ErrorCode::INVALID_PARAMS,
                message: "uninstall_plugin requires `confirm: true`".into(),
                data: None,
            });
        }
        self.api
            .delete::<serde_json::Value>(&format!("/api/plugins/{}", a.id))
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"uninstalled": true, "id": a.id})))
    }
}
