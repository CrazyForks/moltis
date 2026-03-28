use std::sync::atomic::Ordering;

use {
    axum::{
        Json,
        extract::{Path, State},
        http::StatusCode,
        response::{IntoResponse, Response},
    },
    serde::Serialize,
    tokio::process::Command,
};

use moltis_gateway::{
    auth::{SshAuthMode, SshKeyEntry, SshResolvedTarget, SshTargetEntry},
    node_exec::exec_resolved_ssh_target,
};

const SSH_STORE_UNAVAILABLE: &str = "SSH_STORE_UNAVAILABLE";
const SSH_KEY_NAME_REQUIRED: &str = "SSH_KEY_NAME_REQUIRED";
const SSH_PRIVATE_KEY_REQUIRED: &str = "SSH_PRIVATE_KEY_REQUIRED";
const SSH_TARGET_LABEL_REQUIRED: &str = "SSH_TARGET_LABEL_REQUIRED";
const SSH_TARGET_REQUIRED: &str = "SSH_TARGET_REQUIRED";
const SSH_LIST_FAILED: &str = "SSH_LIST_FAILED";
const SSH_KEY_GENERATE_FAILED: &str = "SSH_KEY_GENERATE_FAILED";
const SSH_KEY_IMPORT_FAILED: &str = "SSH_KEY_IMPORT_FAILED";
const SSH_KEY_DELETE_FAILED: &str = "SSH_KEY_DELETE_FAILED";
const SSH_TARGET_CREATE_FAILED: &str = "SSH_TARGET_CREATE_FAILED";
const SSH_TARGET_DELETE_FAILED: &str = "SSH_TARGET_DELETE_FAILED";
const SSH_TARGET_DEFAULT_FAILED: &str = "SSH_TARGET_DEFAULT_FAILED";
const SSH_TARGET_TEST_FAILED: &str = "SSH_TARGET_TEST_FAILED";

#[derive(Serialize)]
pub struct SshStatusResponse {
    keys: Vec<SshKeyEntry>,
    targets: Vec<SshTargetEntry>,
}

impl IntoResponse for SshStatusResponse {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

#[derive(Serialize)]
pub struct SshMutationResponse {
    ok: bool,
    id: Option<i64>,
}

impl SshMutationResponse {
    fn success(id: Option<i64>) -> Self {
        Self { ok: true, id }
    }
}

impl IntoResponse for SshMutationResponse {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

#[derive(Serialize)]
pub struct SshTestResponse {
    ok: bool,
    reachable: bool,
    stdout: String,
    stderr: String,
    exit_code: i32,
    route_label: Option<String>,
}

impl IntoResponse for SshTestResponse {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

#[derive(Clone, Serialize)]
pub struct SshDoctorCheck {
    id: &'static str,
    level: &'static str,
    title: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct SshDoctorRoute {
    label: String,
    target: String,
    auth_mode: &'static str,
    source: &'static str,
}

#[derive(Serialize)]
pub struct SshDoctorResponse {
    ok: bool,
    exec_host: String,
    ssh_binary_available: bool,
    ssh_binary_version: Option<String>,
    paired_node_count: usize,
    managed_key_count: usize,
    encrypted_key_count: usize,
    managed_target_count: usize,
    configured_node: Option<String>,
    legacy_target: Option<String>,
    active_route: Option<SshDoctorRoute>,
    checks: Vec<SshDoctorCheck>,
}

impl IntoResponse for SshDoctorResponse {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn service_unavailable(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code,
            message: message.into(),
        }
    }

    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    fn internal(code: &'static str, err: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            code: &'static str,
            error: String,
        }

        (
            self.status,
            Json(Body {
                code: self.code,
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[derive(serde::Deserialize)]
pub struct GenerateKeyRequest {
    name: String,
}

#[derive(serde::Deserialize)]
pub struct ImportKeyRequest {
    name: String,
    private_key: String,
}

#[derive(serde::Deserialize)]
pub struct CreateTargetRequest {
    label: String,
    target: String,
    port: Option<u16>,
    auth_mode: SshAuthMode,
    key_id: Option<i64>,
    #[serde(default)]
    is_default: bool,
}

pub async fn ssh_status(
    State(state): State<crate::server::AppState>,
) -> Result<SshStatusResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    let keys = store
        .list_ssh_keys()
        .await
        .map_err(|err| ApiError::internal(SSH_LIST_FAILED, err))?;
    let targets = store
        .list_ssh_targets()
        .await
        .map_err(|err| ApiError::internal(SSH_LIST_FAILED, err))?;
    Ok(SshStatusResponse { keys, targets })
}

pub async fn ssh_generate_key(
    State(state): State<crate::server::AppState>,
    Json(body): Json<GenerateKeyRequest>,
) -> Result<SshMutationResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request(
            SSH_KEY_NAME_REQUIRED,
            "ssh key name is required",
        ));
    }

    let (private_key, public_key, fingerprint) = generate_ssh_key_material(name)
        .await
        .map_err(|err| ApiError::internal(SSH_KEY_GENERATE_FAILED, err))?;
    let id = store
        .create_ssh_key(name, &private_key, &public_key, &fingerprint)
        .await
        .map_err(|err| ApiError::internal(SSH_KEY_GENERATE_FAILED, err))?;

    Ok(SshMutationResponse::success(Some(id)))
}

pub async fn ssh_import_key(
    State(state): State<crate::server::AppState>,
    Json(body): Json<ImportKeyRequest>,
) -> Result<SshMutationResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request(
            SSH_KEY_NAME_REQUIRED,
            "ssh key name is required",
        ));
    }
    if body.private_key.trim().is_empty() {
        return Err(ApiError::bad_request(
            SSH_PRIVATE_KEY_REQUIRED,
            "private key is required",
        ));
    }

    let (public_key, fingerprint) = inspect_imported_private_key(&body.private_key)
        .await
        .map_err(|err| ApiError::bad_request(SSH_KEY_IMPORT_FAILED, err.to_string()))?;
    let id = store
        .create_ssh_key(name, &body.private_key, &public_key, &fingerprint)
        .await
        .map_err(|err| ApiError::internal(SSH_KEY_IMPORT_FAILED, err))?;

    Ok(SshMutationResponse::success(Some(id)))
}

pub async fn ssh_delete_key(
    State(state): State<crate::server::AppState>,
    Path(id): Path<i64>,
) -> Result<SshMutationResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    store
        .delete_ssh_key(id)
        .await
        .map_err(|err| ApiError::bad_request(SSH_KEY_DELETE_FAILED, err.to_string()))?;
    Ok(SshMutationResponse::success(None))
}

pub async fn ssh_create_target(
    State(state): State<crate::server::AppState>,
    Json(body): Json<CreateTargetRequest>,
) -> Result<SshMutationResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    if body.label.trim().is_empty() {
        return Err(ApiError::bad_request(
            SSH_TARGET_LABEL_REQUIRED,
            "target label is required",
        ));
    }
    if body.target.trim().is_empty() {
        return Err(ApiError::bad_request(
            SSH_TARGET_REQUIRED,
            "target is required",
        ));
    }

    let id = store
        .create_ssh_target(
            &body.label,
            &body.target,
            body.port,
            body.auth_mode,
            body.key_id,
            body.is_default,
        )
        .await
        .map_err(|err| ApiError::bad_request(SSH_TARGET_CREATE_FAILED, err.to_string()))?;
    refresh_ssh_target_count(&state).await;

    Ok(SshMutationResponse::success(Some(id)))
}

pub async fn ssh_delete_target(
    State(state): State<crate::server::AppState>,
    Path(id): Path<i64>,
) -> Result<SshMutationResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    store
        .delete_ssh_target(id)
        .await
        .map_err(|err| ApiError::internal(SSH_TARGET_DELETE_FAILED, err))?;
    refresh_ssh_target_count(&state).await;

    Ok(SshMutationResponse::success(None))
}

pub async fn ssh_set_default_target(
    State(state): State<crate::server::AppState>,
    Path(id): Path<i64>,
) -> Result<SshMutationResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    store
        .set_default_ssh_target(id)
        .await
        .map_err(|err| ApiError::bad_request(SSH_TARGET_DEFAULT_FAILED, err.to_string()))?;
    Ok(SshMutationResponse::success(Some(id)))
}

pub async fn ssh_test_target(
    State(state): State<crate::server::AppState>,
    Path(id): Path<i64>,
) -> Result<SshTestResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    let target = store
        .resolve_ssh_target_by_id(id)
        .await
        .map_err(|err| ApiError::internal(SSH_TARGET_TEST_FAILED, err))?
        .ok_or_else(|| ApiError::bad_request(SSH_TARGET_TEST_FAILED, "ssh target not found"))?;

    let probe = "__moltis_ssh_probe__";
    let result = exec_resolved_ssh_target(
        store,
        &target,
        &format!("printf {probe}"),
        10,
        None,
        None,
        8 * 1024,
    )
    .await
    .map_err(|err| ApiError::bad_request(SSH_TARGET_TEST_FAILED, err.to_string()))?;

    Ok(SshTestResponse {
        ok: true,
        reachable: result.exit_code == 0 && result.stdout.contains(probe),
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        route_label: Some(target.label),
    })
}

pub async fn ssh_doctor(
    State(state): State<crate::server::AppState>,
) -> Result<SshDoctorResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    let keys = store
        .list_ssh_keys()
        .await
        .map_err(|err| ApiError::internal(SSH_LIST_FAILED, err))?;
    let targets = store
        .list_ssh_targets()
        .await
        .map_err(|err| ApiError::internal(SSH_LIST_FAILED, err))?;

    let config = moltis_config::discover_and_load();
    let exec_host = config.tools.exec.host.trim().to_string();
    let configured_node = config
        .tools
        .exec
        .node
        .clone()
        .filter(|value: &String| !value.trim().is_empty());
    let legacy_target = config
        .tools
        .exec
        .ssh_target
        .clone()
        .filter(|value: &String| !value.trim().is_empty());
    let default_target = targets.iter().find(|target| target.is_default).cloned();
    let (ssh_binary_available, ssh_binary_version) = detect_ssh_binary().await;
    let paired_node_count = {
        let inner = state.gateway.inner.read().await;
        inner.nodes.list().len()
    };
    let encrypted_key_count = keys.iter().filter(|entry| entry.encrypted).count();
    let vault_is_unsealed = match state.gateway.vault.as_ref() {
        Some(vault) => vault.is_unsealed().await,
        None => false,
    };

    let active_route = if exec_host == "ssh" {
        default_target
            .as_ref()
            .map(|target| SshDoctorRoute {
                label: format!("SSH: {}", target.label),
                target: target.target.clone(),
                auth_mode: match target.auth_mode {
                    SshAuthMode::Managed => "managed",
                    SshAuthMode::System => "system",
                },
                source: "managed",
            })
            .or_else(|| {
                legacy_target
                    .as_ref()
                    .map(|target: &String| SshDoctorRoute {
                        label: format!("SSH: {target}"),
                        target: target.clone(),
                        auth_mode: "system",
                        source: "legacy_config",
                    })
            })
    } else {
        None
    };

    let checks = build_doctor_checks(DoctorInputs {
        exec_host: &exec_host,
        ssh_binary_available,
        paired_node_count,
        managed_target_count: targets.len(),
        managed_key_count: keys.len(),
        encrypted_key_count,
        configured_node: configured_node.as_deref(),
        legacy_target: legacy_target.as_deref(),
        default_target: default_target.as_ref(),
        vault_is_unsealed,
    });

    Ok(SshDoctorResponse {
        ok: true,
        exec_host,
        ssh_binary_available,
        ssh_binary_version,
        paired_node_count,
        managed_key_count: keys.len(),
        encrypted_key_count,
        managed_target_count: targets.len(),
        configured_node,
        legacy_target,
        active_route,
        checks,
    })
}

pub async fn ssh_doctor_test_active(
    State(state): State<crate::server::AppState>,
) -> Result<SshTestResponse, ApiError> {
    let store = state.gateway.credential_store.as_ref().ok_or_else(|| {
        ApiError::service_unavailable(SSH_STORE_UNAVAILABLE, "no credential store")
    })?;

    let config = moltis_config::discover_and_load();
    if config.tools.exec.host.trim() != "ssh" {
        return Err(ApiError::bad_request(
            SSH_TARGET_TEST_FAILED,
            "remote exec is not configured to use ssh",
        ));
    }

    let route = if let Some(target) = store
        .get_default_ssh_target()
        .await
        .map_err(|err| ApiError::internal(SSH_TARGET_TEST_FAILED, err))?
    {
        target
    } else if let Some(target) = config
        .tools
        .exec
        .ssh_target
        .clone()
        .filter(|value: &String| !value.trim().is_empty())
    {
        SshResolvedTarget {
            id: 0,
            node_id: format!("ssh:{target}"),
            label: target.clone(),
            target,
            port: None,
            auth_mode: SshAuthMode::System,
            key_id: None,
            key_name: None,
        }
    } else {
        return Err(ApiError::bad_request(
            SSH_TARGET_TEST_FAILED,
            "no active ssh route is configured",
        ));
    };

    let probe = "__moltis_ssh_probe__";
    let result = exec_resolved_ssh_target(
        store,
        &route,
        &format!("printf {probe}"),
        10,
        None,
        None,
        8 * 1024,
    )
    .await
    .map_err(|err| ApiError::bad_request(SSH_TARGET_TEST_FAILED, err.to_string()))?;

    Ok(SshTestResponse {
        ok: true,
        reachable: result.exit_code == 0 && result.stdout.contains(probe),
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        route_label: Some(route.label),
    })
}

async fn refresh_ssh_target_count(state: &crate::server::AppState) {
    let Some(store) = state.gateway.credential_store.as_ref() else {
        return;
    };
    match store.ssh_target_count().await {
        Ok(count) => state
            .gateway
            .ssh_target_count
            .store(count, Ordering::Relaxed),
        Err(error) => tracing::warn!(%error, "failed to refresh ssh target count"),
    }
}

async fn generate_ssh_key_material(name: &str) -> anyhow::Result<(String, String, String)> {
    let dir = tempfile::tempdir()?;
    let key_path = dir.path().join("moltis_deploy_key");
    let output = Command::new("ssh-keygen")
        .arg("-t")
        .arg("ed25519")
        .arg("-N")
        .arg("")
        .arg("-C")
        .arg(format!("moltis:{name}"))
        .arg("-f")
        .arg(&key_path)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        );
    }

    let private_key: String = tokio::fs::read_to_string(&key_path).await?;
    let public_key: String = tokio::fs::read_to_string(key_path.with_extension("pub")).await?;
    let fingerprint = ssh_keygen_fingerprint(&key_path).await?;
    Ok((private_key, public_key.trim().to_string(), fingerprint))
}

async fn inspect_imported_private_key(private_key: &str) -> anyhow::Result<(String, String)> {
    let dir = tempfile::tempdir()?;
    let key_path = dir.path().join("imported_key");
    tokio::fs::write(&key_path, private_key).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    let public_output = Command::new("ssh-keygen")
        .arg("-y")
        .arg("-f")
        .arg(&key_path)
        .output()
        .await?;
    if !public_output.status.success() {
        let stderr = String::from_utf8_lossy(&public_output.stderr)
            .trim()
            .to_string();
        anyhow::bail!(if stderr.to_lowercase().contains("passphrase") {
            "passphrase-protected private keys are not supported yet".to_string()
        } else {
            stderr
        });
    }

    let fingerprint = ssh_keygen_fingerprint(&key_path).await?;
    let public_key = String::from_utf8(public_output.stdout)?.trim().to_string();
    Ok((public_key, fingerprint))
}

async fn ssh_keygen_fingerprint(path: &std::path::Path) -> anyhow::Result<String> {
    let output = Command::new("ssh-keygen")
        .arg("-lf")
        .arg(path)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

struct DoctorInputs<'a> {
    exec_host: &'a str,
    ssh_binary_available: bool,
    paired_node_count: usize,
    managed_target_count: usize,
    managed_key_count: usize,
    encrypted_key_count: usize,
    configured_node: Option<&'a str>,
    legacy_target: Option<&'a str>,
    default_target: Option<&'a SshTargetEntry>,
    vault_is_unsealed: bool,
}

fn build_doctor_checks(input: DoctorInputs<'_>) -> Vec<SshDoctorCheck> {
    let mut checks = Vec::new();

    checks.push(SshDoctorCheck {
        id: "exec-host",
        level: "ok",
        title: "Execution backend",
        message: match input.exec_host {
            "ssh" => "Remote exec is currently routed through SSH.".to_string(),
            "node" => "Remote exec is currently routed through paired nodes.".to_string(),
            _ => "Remote exec is currently running locally.".to_string(),
        },
        hint: Some("Change this in tools.exec.host or from the chat node picker.".to_string()),
    });

    if input.ssh_binary_available {
        checks.push(SshDoctorCheck {
            id: "ssh-binary",
            level: "ok",
            title: "SSH client",
            message: "System ssh client is available.".to_string(),
            hint: None,
        });
    } else {
        checks.push(SshDoctorCheck {
            id: "ssh-binary",
            level: "error",
            title: "SSH client",
            message: "System ssh client is not available in PATH.".to_string(),
            hint: Some(
                "Install OpenSSH or fix PATH before using SSH execution targets.".to_string(),
            ),
        });
    }

    match input.exec_host {
        "ssh" => {
            if let Some(target) = input.default_target {
                checks.push(SshDoctorCheck {
                    id: "ssh-route",
                    level: "ok",
                    title: "Active SSH route",
                    message: format!(
                        "Using managed target '{}' ({})",
                        target.label, target.target
                    ),
                    hint: None,
                });
                if target.auth_mode == SshAuthMode::Managed
                    && input.encrypted_key_count > 0
                    && !input.vault_is_unsealed
                {
                    checks.push(SshDoctorCheck {
                        id: "managed-key-vault",
                        level: "error",
                        title: "Managed key access",
                        message: "The active SSH route uses a managed key, but the vault is locked.".to_string(),
                        hint: Some("Unlock the vault in Settings → Encryption before testing or using this target.".to_string()),
                    });
                }
            } else if let Some(target) = input.legacy_target {
                checks.push(SshDoctorCheck {
                    id: "ssh-route",
                    level: "warn",
                    title: "Active SSH route",
                    message: format!("Using legacy config target '{target}'."),
                    hint: Some("Move this into Settings → SSH if you want named targets, testing, and managed deploy keys.".to_string()),
                });
            } else {
                checks.push(SshDoctorCheck {
                    id: "ssh-route",
                    level: "error",
                    title: "Active SSH route",
                    message: "SSH execution is enabled, but no target is configured.".to_string(),
                    hint: Some(
                        "Add a target in Settings → SSH or set tools.exec.ssh_target.".to_string(),
                    ),
                });
            }
        },
        "node" => {
            if input.paired_node_count == 0 {
                checks.push(SshDoctorCheck {
                    id: "paired-node-route",
                    level: "error",
                    title: "Paired node route",
                    message: "Remote exec is set to use paired nodes, but none are connected.".to_string(),
                    hint: Some("Generate a connection token from the Nodes page or switch tools.exec.host back to local.".to_string()),
                });
            } else if let Some(node) = input.configured_node {
                checks.push(SshDoctorCheck {
                    id: "paired-node-route",
                    level: "ok",
                    title: "Paired node route",
                    message: format!("Default node preference is '{node}'."),
                    hint: None,
                });
            } else {
                checks.push(SshDoctorCheck {
                    id: "paired-node-route",
                    level: "warn",
                    title: "Paired node route",
                    message: "Paired nodes are available, but no default node is configured.".to_string(),
                    hint: Some("Select a node from chat or set tools.exec.node if you want a fixed default.".to_string()),
                });
            }
        },
        _ => {
            checks.push(SshDoctorCheck {
                id: "local-route",
                level: "warn",
                title: "Remote exec route",
                message: "The current backend is local, so SSH and node targets are only available when selected explicitly.".to_string(),
                hint: Some("Switch tools.exec.host if you want remote execution by default.".to_string()),
            });
        },
    }

    if input.managed_key_count == 0
        && input.managed_target_count == 0
        && input.legacy_target.is_none()
    {
        checks.push(SshDoctorCheck {
            id: "ssh-onboarding",
            level: "warn",
            title: "SSH onboarding",
            message: "No SSH targets are configured yet.".to_string(),
            hint: Some("Generate a deploy key in Settings → SSH, copy the public key to the remote host, then add a named target.".to_string()),
        });
    } else if input.managed_target_count > 0 {
        checks.push(SshDoctorCheck {
            id: "ssh-inventory",
            level: "ok",
            title: "Managed SSH inventory",
            message: format!(
                "{} key(s), {} target(s), {} encrypted key(s).",
                input.managed_key_count, input.managed_target_count, input.encrypted_key_count
            ),
            hint: None,
        });
    }

    checks
}

async fn detect_ssh_binary() -> (bool, Option<String>) {
    match Command::new("ssh").arg("-V").output().await {
        Ok(output) => {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            } else {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            };
            (output.status.success(), (!text.is_empty()).then_some(text))
        },
        Err(_) => (false, None),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn generated_key_material_round_trips() {
        let (private_key, public_key, fingerprint) =
            generate_ssh_key_material("test-key").await.unwrap();
        assert!(private_key.contains("BEGIN OPENSSH PRIVATE KEY"));
        assert!(public_key.starts_with("ssh-ed25519 "));
        assert!(fingerprint.contains("SHA256:"));
    }

    #[tokio::test]
    async fn imported_key_is_validated() {
        let (private_key, ..) = generate_ssh_key_material("importable").await.unwrap();
        let (public_key, fingerprint) = inspect_imported_private_key(&private_key).await.unwrap();
        assert!(public_key.starts_with("ssh-ed25519 "));
        assert!(fingerprint.contains("SHA256:"));
    }

    #[test]
    fn doctor_checks_flag_missing_ssh_target() {
        let checks = build_doctor_checks(DoctorInputs {
            exec_host: "ssh",
            ssh_binary_available: true,
            paired_node_count: 0,
            managed_target_count: 0,
            managed_key_count: 0,
            encrypted_key_count: 0,
            configured_node: None,
            legacy_target: None,
            default_target: None,
            vault_is_unsealed: false,
        });

        assert!(
            checks
                .iter()
                .any(|check| check.id == "ssh-route" && check.level == "error")
        );
    }

    #[test]
    fn doctor_checks_flag_locked_vault_for_managed_route() {
        let default_target = SshTargetEntry {
            id: 1,
            label: "prod".to_string(),
            target: "deploy@example.com".to_string(),
            port: None,
            auth_mode: SshAuthMode::Managed,
            key_id: Some(1),
            key_name: Some("prod-key".to_string()),
            is_default: true,
            created_at: "2026-03-28T00:00:00Z".to_string(),
            updated_at: "2026-03-28T00:00:00Z".to_string(),
        };
        let checks = build_doctor_checks(DoctorInputs {
            exec_host: "ssh",
            ssh_binary_available: true,
            paired_node_count: 0,
            managed_target_count: 1,
            managed_key_count: 1,
            encrypted_key_count: 1,
            configured_node: None,
            legacy_target: None,
            default_target: Some(&default_target),
            vault_is_unsealed: false,
        });

        assert!(
            checks
                .iter()
                .any(|check| check.id == "managed-key-vault" && check.level == "error")
        );
    }
}
