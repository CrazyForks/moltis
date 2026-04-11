//! Sandbox routing for the native filesystem tools.
//!
//! Phase 2 of moltis-f8i (GH moltis-org/moltis#657). When a session is
//! sandboxed, Read/Write/Edit/MultiEdit/Glob/Grep dispatch through this
//! bridge instead of running against the gateway host.
//!
//! Each operation is a small shell script invoked through the existing
//! `Sandbox` trait's command-execution API, so the bridge works with
//! any backend (Docker, Apple container, none) without per-backend
//! plumbing.
//!
//! Data moves in and out via base64 round-trips to avoid quoting issues
//! with arbitrary binary / text content. Writes are capped at 512 KB
//! because the content is embedded in the command string.

use {
    base64::{Engine as _, engine::general_purpose::STANDARD as BASE64},
    serde_json::{Value, json},
    std::{path::PathBuf, sync::Arc, time::Duration},
    tracing::warn,
};

use crate::{
    Result,
    error::Error,
    exec::ExecOpts,
    file_io::shell_single_quote,
    sandbox::{Sandbox, SandboxId, SandboxRouter},
};

/// Maximum file size Write/Edit/MultiEdit can send into a sandbox in a
/// single call. Base64 expands by ~33%, so 512 KB raw becomes ~683 KB
/// of shell arg — comfortably under typical ARG_MAX limits.
pub const MAX_SANDBOX_WRITE_BYTES: usize = 512 * 1024;

const DEFAULT_SANDBOX_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SANDBOX_READ_OUTPUT: usize = 32 * 1024 * 1024;

// Exit codes used by the bridge scripts to encode typed errors.
const EXIT_NOT_FOUND: i32 = 10;
const EXIT_PERMISSION_DENIED: i32 = 11;
const EXIT_NOT_REGULAR_FILE: i32 = 12;
const EXIT_TOO_LARGE: i32 = 13;
const EXIT_SYMLINK: i32 = 14;
const EXIT_PARENT_MISSING: i32 = 20;

fn default_opts() -> ExecOpts {
    ExecOpts {
        timeout: DEFAULT_SANDBOX_TIMEOUT,
        max_output_bytes: DEFAULT_SANDBOX_READ_OUTPUT,
        working_dir: Some(PathBuf::from("/home/sandbox")),
        env: Vec::new(),
    }
}

/// Prepare the sandbox for a session and return the backend + id pair.
pub async fn ensure_sandbox(
    router: &SandboxRouter,
    session_key: &str,
) -> Result<(Arc<dyn Sandbox>, SandboxId)> {
    let id = router.sandbox_id_for(session_key);
    let image = router.resolve_image(session_key, None).await;
    let backend = Arc::clone(router.backend());
    backend.ensure_ready(&id, Some(&image)).await?;
    Ok((backend, id))
}

/// Read a file through the sandbox. Returns raw bytes on success or a
/// typed error variant the caller can surface to the LLM.
pub async fn sandbox_read(
    backend: &Arc<dyn Sandbox>,
    id: &SandboxId,
    file_path: &str,
    max_bytes: u64,
) -> Result<SandboxReadResult> {
    let quoted = shell_single_quote(file_path);
    let script = format!(
        "path={quoted}; max={max_bytes}; \
         if [ ! -e \"$path\" ]; then exit {EXIT_NOT_FOUND}; fi; \
         if [ ! -r \"$path\" ]; then exit {EXIT_PERMISSION_DENIED}; fi; \
         if [ ! -f \"$path\" ]; then exit {EXIT_NOT_REGULAR_FILE}; fi; \
         size=$(wc -c < \"$path\"); \
         if [ \"$size\" -gt \"$max\" ]; then echo \"$size\" >&2; exit {EXIT_TOO_LARGE}; fi; \
         base64 < \"$path\" | tr -d '\\n'"
    );

    let result = backend.exec(id, &script, &default_opts()).await?;
    match result.exit_code {
        0 => {
            let bytes = BASE64.decode(result.stdout.trim()).map_err(|e| {
                Error::message(format!(
                    "failed to decode sandbox read for '{file_path}': {e}"
                ))
            })?;
            Ok(SandboxReadResult::Ok(bytes))
        },
        EXIT_NOT_FOUND => Ok(SandboxReadResult::NotFound),
        EXIT_PERMISSION_DENIED => Ok(SandboxReadResult::PermissionDenied),
        EXIT_NOT_REGULAR_FILE => Ok(SandboxReadResult::NotRegularFile),
        EXIT_TOO_LARGE => {
            let size = result.stderr.trim().parse::<u64>().unwrap_or(0);
            Ok(SandboxReadResult::TooLarge(size))
        },
        other => {
            let detail = if result.stderr.trim().is_empty() {
                format!("sandbox read exited with code {other}")
            } else {
                result.stderr.trim().to_string()
            };
            Err(Error::message(format!(
                "sandbox read of '{file_path}' failed: {detail}"
            )))
        },
    }
}

/// Outcome of a sandbox read call.
#[derive(Debug)]
pub enum SandboxReadResult {
    Ok(Vec<u8>),
    NotFound,
    PermissionDenied,
    NotRegularFile,
    TooLarge(u64),
}

impl SandboxReadResult {
    /// Convert a non-`Ok` variant into the typed JSON payload the fs
    /// tools return. `None` for `Ok` (caller handles success).
    #[must_use]
    pub fn into_typed_payload(self, file_path: &str, max_bytes: u64) -> Option<Value> {
        match self {
            Self::Ok(_) => None,
            Self::NotFound => Some(json!({
                "kind": "not_found",
                "file_path": file_path,
                "error": "file does not exist",
                "detail": "",
            })),
            Self::PermissionDenied => Some(json!({
                "kind": "permission_denied",
                "file_path": file_path,
                "error": "insufficient permissions to access file",
                "detail": "",
            })),
            Self::NotRegularFile => Some(json!({
                "kind": "not_regular_file",
                "file_path": file_path,
                "error": "path is not a regular file",
                "detail": "",
            })),
            Self::TooLarge(size) => Some(json!({
                "kind": "too_large",
                "file_path": file_path,
                "error": format!(
                    "file is too large ({:.1} MB) — maximum is {:.0} MB",
                    size as f64 / (1024.0 * 1024.0),
                    max_bytes as f64 / (1024.0 * 1024.0),
                ),
                "bytes": size,
                "max_bytes": max_bytes,
            })),
        }
    }
}

/// Write a file through the sandbox. Returns `Ok(None)` on success or
/// a typed error payload the caller should surface to the LLM.
pub async fn sandbox_write(
    backend: &Arc<dyn Sandbox>,
    id: &SandboxId,
    file_path: &str,
    content: &[u8],
) -> Result<Option<Value>> {
    if content.len() > MAX_SANDBOX_WRITE_BYTES {
        return Err(Error::message(format!(
            "sandbox Write is limited to {} KB per call (got {:.1} KB); \
             larger writes will ship in a follow-up that chunks content",
            MAX_SANDBOX_WRITE_BYTES / 1024,
            content.len() as f64 / 1024.0,
        )));
    }

    let encoded = BASE64.encode(content);
    let quoted_path = shell_single_quote(file_path);
    let script = format!(
        "path={quoted_path}; \
         parent=$(dirname \"$path\"); \
         if [ ! -d \"$parent\" ]; then exit {EXIT_PARENT_MISSING}; fi; \
         if [ -L \"$path\" ]; then exit {EXIT_SYMLINK}; fi; \
         tmp=\"$path.moltis.$$\"; \
         printf '%s' '{encoded}' | base64 -d > \"$tmp\" && mv \"$tmp\" \"$path\""
    );

    let result = backend.exec(id, &script, &default_opts()).await?;
    match result.exit_code {
        0 => Ok(None),
        EXIT_PARENT_MISSING => Err(Error::message(format!(
            "cannot resolve parent of '{file_path}': directory does not exist in sandbox"
        ))),
        EXIT_SYMLINK => Ok(Some(json!({
            "kind": "path_denied",
            "file_path": file_path,
            "error": "target is a symbolic link; refusing to follow",
            "detail": "sandbox Write rejects symlinks",
        }))),
        other => {
            let detail = if result.stderr.trim().is_empty() {
                format!("sandbox write exited with code {other}")
            } else {
                result.stderr.trim().to_string()
            };
            Err(Error::message(format!(
                "sandbox write of '{file_path}' failed: {detail}"
            )))
        },
    }
}

/// List regular files under `root` inside the sandbox.
pub async fn sandbox_list_files(
    backend: &Arc<dyn Sandbox>,
    id: &SandboxId,
    root: &str,
) -> Result<Vec<String>> {
    let quoted = shell_single_quote(root);
    let script = format!("find {quoted} -type f 2>/dev/null");
    let result = backend.exec(id, &script, &default_opts()).await?;
    if result.exit_code != 0 && result.stdout.trim().is_empty() {
        let detail = if result.stderr.trim().is_empty() {
            format!("find exited with code {}", result.exit_code)
        } else {
            result.stderr.trim().to_string()
        };
        warn!(root, %detail, "sandbox list_files failed");
        return Err(Error::message(format!(
            "sandbox list_files '{root}' failed: {detail}"
        )));
    }
    Ok(result
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

/// Output-mode discriminator for [`sandbox_grep`].
///
/// Mirrors `grep::OutputMode` but kept in the bridge so the bridge
/// doesn't need a downward dependency on the tool module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxGrepMode {
    /// Return one row per matching line as `{path, line, match}`.
    Content,
    /// Return just the file paths that had at least one match.
    FilesWithMatches,
    /// Return `{path, count}` per file with a positive match count.
    Count,
}

/// Options for [`sandbox_grep`]. Kept minimal — context lines and
/// multiline mode are not wired because they require either complex
/// output parsing or post-filtering, and can land in a follow-up.
#[derive(Debug, Clone)]
pub struct SandboxGrepOptions<'a> {
    pub pattern: &'a str,
    pub path: &'a str,
    pub mode: SandboxGrepMode,
    pub case_insensitive: bool,
    /// Shell-level `--include=GLOB` filter. Caller is responsible for
    /// mapping `type` → glob before calling.
    pub include_glob: Option<&'a str>,
}

/// Run `grep` inside the sandbox and return a typed payload shaped
/// like the host `GrepTool` response.
pub async fn sandbox_grep(
    backend: &Arc<dyn Sandbox>,
    id: &SandboxId,
    opts: SandboxGrepOptions<'_>,
) -> Result<Value> {
    let pattern_q = shell_single_quote(opts.pattern);
    let path_q = shell_single_quote(opts.path);
    let mut flags: Vec<&str> = vec!["-r", "-E"];
    if opts.case_insensitive {
        flags.push("-i");
    }
    match opts.mode {
        SandboxGrepMode::Content => {
            flags.push("-n");
            flags.push("-H");
        },
        SandboxGrepMode::FilesWithMatches => {
            flags.push("-l");
        },
        SandboxGrepMode::Count => {
            flags.push("-c");
            flags.push("-H");
        },
    }
    let include_arg = opts
        .include_glob
        .map(|g| format!("--include={}", shell_single_quote(g)))
        .unwrap_or_default();
    let flags_str = flags.join(" ");
    // grep exits 0 on match, 1 on no match, 2 on error. Treat 0 and 1
    // as success so empty-result calls don't error.
    let script = format!(
        "grep {flags_str} {include_arg} -- {pattern_q} {path_q} 2>/dev/null; \
         rc=$?; if [ $rc -eq 1 ]; then exit 0; else exit $rc; fi"
    );
    let result = backend.exec(id, &script, &default_opts()).await?;
    if result.exit_code != 0 {
        let detail = if result.stderr.trim().is_empty() {
            format!("grep exited with code {}", result.exit_code)
        } else {
            result.stderr.trim().to_string()
        };
        return Err(Error::message(format!("sandbox grep failed: {detail}")));
    }

    let lines: Vec<&str> = result
        .stdout
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();

    match opts.mode {
        SandboxGrepMode::FilesWithMatches => Ok(json!({
            "mode": "files_with_matches",
            "files": lines.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "truncated": false,
        })),
        SandboxGrepMode::Count => {
            let mut counts = Vec::new();
            for line in &lines {
                // Format: "path:count"
                if let Some((path, count_str)) = line.rsplit_once(':')
                    && let Ok(count) = count_str.parse::<usize>()
                    && count > 0
                {
                    counts.push(json!({
                        "path": path,
                        "count": count,
                    }));
                }
            }
            Ok(json!({
                "mode": "count",
                "counts": counts,
                "truncated": false,
            }))
        },
        SandboxGrepMode::Content => {
            let mut matches = Vec::new();
            for line in &lines {
                // Format: "path:lineno:text"
                let mut parts = line.splitn(3, ':');
                let (Some(path), Some(lineno_str), Some(text)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let Ok(lineno) = lineno_str.parse::<usize>() else {
                    continue;
                };
                matches.push(json!({
                    "path": path,
                    "line": lineno,
                    "match": text,
                    "block": vec![format!("{lineno}:{text}")],
                }));
            }
            Ok(json!({
                "mode": "content",
                "matches": matches,
                "truncated": false,
            }))
        },
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
pub(crate) mod test_helpers {
    //! A stub `Sandbox` implementation that records commands and
    //! replies with pre-programmed `ExecResult`s.

    use {
        super::*,
        crate::{exec::ExecResult, sandbox::types::BuildImageResult},
        async_trait::async_trait,
        std::sync::Mutex,
    };

    pub struct MockSandbox {
        pub calls: Mutex<Vec<String>>,
        pub responses: Mutex<Vec<ExecResult>>,
    }

    impl MockSandbox {
        pub fn new(responses: Vec<ExecResult>) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(responses),
            })
        }

        pub fn last_command(&self) -> Option<String> {
            self.calls.lock().unwrap().last().cloned()
        }
    }

    #[async_trait]
    impl Sandbox for MockSandbox {
        fn backend_name(&self) -> &'static str {
            "mock"
        }

        fn is_real(&self) -> bool {
            true
        }

        async fn ensure_ready(&self, _id: &SandboxId, _image_override: Option<&str>) -> Result<()> {
            Ok(())
        }

        async fn exec(
            &self,
            _id: &SandboxId,
            command: &str,
            _opts: &ExecOpts,
        ) -> Result<ExecResult> {
            self.calls.lock().unwrap().push(command.to_string());
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(ExecResult {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            } else {
                Ok(responses.remove(0))
            }
        }

        async fn cleanup(&self, _id: &SandboxId) -> Result<()> {
            Ok(())
        }

        async fn build_image(
            &self,
            _base: &str,
            _packages: &[String],
        ) -> Result<Option<BuildImageResult>> {
            Ok(None)
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::{test_helpers::MockSandbox, *},
        crate::{exec::ExecResult, sandbox::types::SandboxScope},
    };

    fn test_id() -> SandboxId {
        SandboxId {
            scope: SandboxScope::Session,
            key: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn sandbox_read_ok_decodes_base64() {
        let encoded = BASE64.encode(b"hello sandbox");
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: encoded,
            stderr: String::new(),
            exit_code: 0,
        }]);
        let backend: Arc<dyn Sandbox> = mock.clone();

        let result = sandbox_read(&backend, &test_id(), "/data/x.txt", 1024)
            .await
            .unwrap();
        match result {
            SandboxReadResult::Ok(bytes) => assert_eq!(bytes, b"hello sandbox"),
            other => panic!("expected Ok, got {other:?}"),
        }
        assert!(mock.last_command().unwrap().contains("/data/x.txt"));
    }

    #[tokio::test]
    async fn sandbox_read_not_found_maps_exit_10() {
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: EXIT_NOT_FOUND,
        }]);
        let backend: Arc<dyn Sandbox> = mock;
        let result = sandbox_read(&backend, &test_id(), "/missing", 1024)
            .await
            .unwrap();
        assert!(matches!(result, SandboxReadResult::NotFound));
        let payload = result.into_typed_payload("/missing", 1024).unwrap();
        assert_eq!(payload["kind"], "not_found");
    }

    #[tokio::test]
    async fn sandbox_read_too_large_captures_size_from_stderr() {
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: String::new(),
            stderr: "12345\n".to_string(),
            exit_code: EXIT_TOO_LARGE,
        }]);
        let backend: Arc<dyn Sandbox> = mock;
        let result = sandbox_read(&backend, &test_id(), "/big", 100)
            .await
            .unwrap();
        assert!(matches!(result, SandboxReadResult::TooLarge(12345)));
        let payload = result.into_typed_payload("/big", 100).unwrap();
        assert_eq!(payload["kind"], "too_large");
        assert_eq!(payload["bytes"], 12345);
    }

    #[tokio::test]
    async fn sandbox_write_ok_encodes_content_and_succeeds() {
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        }]);
        let backend: Arc<dyn Sandbox> = mock.clone();
        let res = sandbox_write(&backend, &test_id(), "/data/out.txt", b"abc")
            .await
            .unwrap();
        assert!(res.is_none());
        let cmd = mock.last_command().unwrap();
        assert!(cmd.contains("/data/out.txt"));
        assert!(cmd.contains(&BASE64.encode(b"abc")));
    }

    #[tokio::test]
    async fn sandbox_write_rejects_oversized_payload() {
        let mock = MockSandbox::new(vec![]);
        let backend: Arc<dyn Sandbox> = mock;
        let big = vec![0u8; MAX_SANDBOX_WRITE_BYTES + 1];
        let err = sandbox_write(&backend, &test_id(), "/data/big.bin", &big)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("limited to"));
    }

    #[tokio::test]
    async fn sandbox_write_symlink_returns_typed_payload() {
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: EXIT_SYMLINK,
        }]);
        let backend: Arc<dyn Sandbox> = mock;
        let payload = sandbox_write(&backend, &test_id(), "/data/link", b"x")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(payload["kind"], "path_denied");
    }

    #[tokio::test]
    async fn sandbox_grep_files_with_matches_parses_paths() {
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: "/data/a.rs\n/data/b.rs\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        }]);
        let backend: Arc<dyn Sandbox> = mock.clone();
        let value = sandbox_grep(&backend, &test_id(), SandboxGrepOptions {
            pattern: "foo",
            path: "/data",
            mode: SandboxGrepMode::FilesWithMatches,
            case_insensitive: false,
            include_glob: None,
        })
        .await
        .unwrap();
        assert_eq!(value["mode"], "files_with_matches");
        let files = value["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert!(mock.last_command().unwrap().contains("-l"));
    }

    #[tokio::test]
    async fn sandbox_grep_content_parses_line_numbers() {
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: "/data/a.rs:3:fn alpha()\n/data/a.rs:7:fn beta()\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        }]);
        let backend: Arc<dyn Sandbox> = mock.clone();
        let value = sandbox_grep(&backend, &test_id(), SandboxGrepOptions {
            pattern: "fn",
            path: "/data",
            mode: SandboxGrepMode::Content,
            case_insensitive: false,
            include_glob: Some("*.rs"),
        })
        .await
        .unwrap();
        assert_eq!(value["mode"], "content");
        let matches = value["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0]["line"], 3);
        assert_eq!(matches[0]["match"], "fn alpha()");
        let cmd = mock.last_command().unwrap();
        assert!(cmd.contains("-n"));
        assert!(cmd.contains("--include="));
    }

    #[tokio::test]
    async fn sandbox_grep_count_filters_zero_matches() {
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: "/data/a.rs:5\n/data/b.rs:0\n/data/c.rs:2\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        }]);
        let backend: Arc<dyn Sandbox> = mock;
        let value = sandbox_grep(&backend, &test_id(), SandboxGrepOptions {
            pattern: "foo",
            path: "/data",
            mode: SandboxGrepMode::Count,
            case_insensitive: true,
            include_glob: None,
        })
        .await
        .unwrap();
        let counts = value["counts"].as_array().unwrap();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0]["path"], "/data/a.rs");
        assert_eq!(counts[0]["count"], 5);
        assert_eq!(counts[1]["path"], "/data/c.rs");
        assert_eq!(counts[1]["count"], 2);
    }

    #[tokio::test]
    async fn sandbox_grep_no_match_is_ok_empty() {
        // grep exits 1 on no match — bridge script maps that to exit 0.
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        }]);
        let backend: Arc<dyn Sandbox> = mock;
        let value = sandbox_grep(&backend, &test_id(), SandboxGrepOptions {
            pattern: "needle",
            path: "/data",
            mode: SandboxGrepMode::FilesWithMatches,
            case_insensitive: false,
            include_glob: None,
        })
        .await
        .unwrap();
        assert_eq!(value["files"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn sandbox_list_files_parses_find_output() {
        let mock = MockSandbox::new(vec![ExecResult {
            stdout: "/data/a.rs\n/data/b.rs\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        }]);
        let backend: Arc<dyn Sandbox> = mock;
        let files = sandbox_list_files(&backend, &test_id(), "/data")
            .await
            .unwrap();
        assert_eq!(files, vec![
            "/data/a.rs".to_string(),
            "/data/b.rs".to_string()
        ]);
    }
}
