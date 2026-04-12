## Session Summary

- Added backend-aware container file access so the fs tools keep one contract while sandbox backends choose the transport.
- Docker and Podman now resolve known bind-mounted guest paths back to the host, use in-container metadata probes for size and type checks, stream regular-file reads over `cp ... -`, stream writes in as tar payloads, and list files via in-container `find` instead of copying trees out.
- Apple Container now resolves mounted home-persistence paths directly on the host and falls back to in-container commands only when the CLI does not expose a native file-copy primitive.
- Added coverage for guest-to-host mount resolution plus mounted-path read, write, and list behavior for Docker and Apple Container, along with tar helper regression tests for the OCI streaming path.

## Validation

- `cargo +nightly-2025-11-30 fmt --all -- --check`
- `cargo test -p moltis-tools build_single_file_tar_`
- `cargo test -p moltis-tools extract_single_file_from_tar_`
- `cargo test -p moltis-tools docker_`
- `cargo test -p moltis-tools apple_container_home_`
- `cargo clippy -p moltis-tools --tests`

## Notes

- OCI regular-file reads no longer stage through host temp directories, and oversized files are rejected before the copy starts.
- OCI directory listings now stay inside the container instead of copying subtrees out.
- Apple Container still lacks an obvious copy primitive in the CLI surface, so unmapped paths continue to use backend-local command fallback.
- `bd dolt push` still fails because no Dolt remote is configured for this worktree.
