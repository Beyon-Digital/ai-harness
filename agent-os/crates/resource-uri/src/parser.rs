//! Canonical resource URI parsing (resource-uri.md).
//!
//! Supported schemes: `workspace`, `artifact`, `secret`, `sandbox`, `run`,
//! `task`, `session`, `adapter`. The parser rejects path traversal
//! (including percent-encoded forms), malformed percent-encoding, empty
//! identifiers, extra separators, and scheme confusion: callers only ever
//! handle the typed [`ResourceUri`].

use std::fmt;
use std::str::FromStr;

use domain::ids::{AdapterId, ArtifactId, RunId, SandboxId, SessionId, TaskId, WorkspaceId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

/// A parsed logical resource reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceUri {
    /// `workspace://<workspace-id>/<relative-path>`
    Workspace {
        /// Workspace the path is relative to.
        workspace_id: WorkspaceId,
        /// Normalized `/`-separated relative path (no `.`/`..`/empties).
        relative_path: String,
    },
    /// `artifact://<artifact-id>`
    Artifact(ArtifactId),
    /// `secret://<namespace>/<name>` — a logical reference only; the
    /// physical path/value is never part of the URI.
    Secret {
        /// Secret namespace (e.g. `github`, `openrouter`).
        namespace: String,
        /// Secret name within the namespace.
        name: String,
    },
    /// `sandbox://<sandbox-id>`
    Sandbox(SandboxId),
    /// `run://<run-id>`
    Run(RunId),
    /// `task://<task-id>`
    Task(TaskId),
    /// `session://<session-id>`
    Session(SessionId),
    /// `adapter://<adapter-id>@<version>#<digest>`
    Adapter {
        /// Adapter identifier.
        adapter_id: AdapterId,
        /// Pinned version string.
        version: String,
        /// Content digest of the adapter bundle.
        digest: String,
    },
}

impl ResourceUri {
    /// Parses a resource URI string.
    pub fn parse(raw: &str) -> errors::Result<Self> {
        let (scheme, rest) = raw
            .split_once("://")
            .ok_or_else(|| invalid("missing ://"))?;
        if rest.is_empty() {
            return Err(invalid("empty URI body"));
        }
        match scheme {
            "workspace" => {
                let (id, path) = rest
                    .split_once('/')
                    .ok_or_else(|| invalid("workspace URI needs <id>/<path>"))?;
                let workspace_id =
                    WorkspaceId::from_str(id).map_err(|_| invalid("workspace id"))?;
                Ok(Self::Workspace {
                    workspace_id,
                    relative_path: normalize_path(path)?,
                })
            }
            "artifact" => Ok(Self::Artifact(
                ArtifactId::from_str(single_id(rest, "artifact id")?)
                    .map_err(|_| invalid("artifact id"))?,
            )),
            "secret" => {
                let (ns, name) = rest
                    .split_once('/')
                    .ok_or_else(|| invalid("secret URI needs <namespace>/<name>"))?;
                if ns.is_empty() || name.is_empty() || name.contains('/') || ns.contains('/') {
                    return Err(invalid("secret namespace/name malformed"));
                }
                Ok(Self::Secret {
                    namespace: decode_token(ns)?,
                    name: decode_token(name)?,
                })
            }
            "sandbox" => Ok(Self::Sandbox(
                SandboxId::from_str(single_id(rest, "sandbox id")?)
                    .map_err(|_| invalid("sandbox id"))?,
            )),
            "run" => Ok(Self::Run(
                RunId::from_str(single_id(rest, "run id")?).map_err(|_| invalid("run id"))?,
            )),
            "task" => Ok(Self::Task(
                TaskId::from_str(single_id(rest, "task id")?).map_err(|_| invalid("task id"))?,
            )),
            "session" => Ok(Self::Session(
                SessionId::from_str(single_id(rest, "session id")?)
                    .map_err(|_| invalid("session id"))?,
            )),
            "adapter" => {
                let (id_and_version, digest) = rest
                    .split_once('#')
                    .ok_or_else(|| invalid("adapter URI needs #<digest>"))?;
                let (id, version) = id_and_version
                    .split_once('@')
                    .ok_or_else(|| invalid("adapter URI needs <id>@<version>"))?;
                if version.is_empty() || digest.is_empty() {
                    return Err(invalid("adapter version/digest empty"));
                }
                Ok(Self::Adapter {
                    adapter_id: AdapterId::from_str(id).map_err(|_| invalid("adapter id"))?,
                    version: version.to_owned(),
                    digest: digest.to_owned(),
                })
            }
            _ => Err(KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                format!("unsupported resource scheme {scheme:?}"),
            )),
        }
    }

    /// The scheme token (`workspace`, `secret`, ...).
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Workspace { .. } => "workspace",
            Self::Artifact(_) => "artifact",
            Self::Secret { .. } => "secret",
            Self::Sandbox(_) => "sandbox",
            Self::Run(_) => "run",
            Self::Task(_) => "task",
            Self::Session(_) => "session",
            Self::Adapter { .. } => "adapter",
        }
    }

    /// Capability string required before this URI may resolve (the resolver
    /// consults the execution context's authority for it).
    pub fn required_capability(&self) -> &'static str {
        match self {
            Self::Workspace { .. } => "workspace:read",
            Self::Artifact(_) => "artifact:read",
            Self::Secret { .. } => "secret:use",
            Self::Sandbox(_) => "sandbox:operate",
            Self::Run(_) | Self::Task(_) | Self::Session(_) => "run:observe",
            Self::Adapter { .. } => "adapter:invoke",
        }
    }
}

impl fmt::Display for ResourceUri {
    /// Renders the canonical form — re-parseable and normalized.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace {
                workspace_id,
                relative_path,
            } => write!(f, "workspace://{workspace_id}/{relative_path}"),
            Self::Artifact(id) => write!(f, "artifact://{id}"),
            Self::Secret { namespace, name } => write!(f, "secret://{namespace}/{name}"),
            Self::Sandbox(id) => write!(f, "sandbox://{id}"),
            Self::Run(id) => write!(f, "run://{id}"),
            Self::Task(id) => write!(f, "task://{id}"),
            Self::Session(id) => write!(f, "session://{id}"),
            Self::Adapter {
                adapter_id,
                version,
                digest,
            } => write!(f, "adapter://{adapter_id}@{version}#{digest}"),
        }
    }
}

/// Parses a URI body that must be a single bare id — no path segments.
fn single_id<'a>(rest: &'a str, what: &str) -> errors::Result<&'a str> {
    if rest.contains('/') || rest.contains('#') || rest.contains('@') || rest.is_empty() {
        return Err(invalid(what));
    }
    Ok(rest)
}

/// Normalizes a workspace-relative path: percent-decodes each segment
/// strictly, then rejects empty/`.`/`..`/separator segments so the result
/// cannot escape the workspace root.
fn normalize_path(raw: &str) -> errors::Result<String> {
    if raw.is_empty() || raw.starts_with('/') || raw.contains('\\') {
        return Err(invalid("workspace path is not a relative unix path"));
    }
    let mut segments = Vec::new();
    for segment in raw.split('/') {
        let decoded = decode_segment(segment)?;
        match decoded.as_str() {
            "" | "." | ".." => return Err(invalid("path segment escapes or is empty")),
            value if value.contains('/') || value.contains('\\') => {
                return Err(invalid("decoded path segment reintroduces a separator"));
            }
            value => segments.push(value.to_owned()),
        }
    }
    Ok(segments.join("/"))
}

/// Percent-decodes a single URI token and rejects empty / `.` / `..` /
/// decoded separators — the same post-decode rule `normalize_path` enforces
/// per workspace segment, applied to non-path tokens like secret names so a
/// `%2f` or `%2e%2e` cannot smuggle structure past the grammar.
fn decode_token(segment: &str) -> errors::Result<String> {
    let decoded = decode_segment(segment)?;
    match decoded.as_str() {
        "" | "." | ".." => Err(invalid("token escapes or is empty")),
        value if value.contains('/') || value.contains('\\') || value.contains('\0') => {
            Err(invalid("decoded token reintroduces a separator"))
        }
        _ => Ok(decoded),
    }
}

/// Strict percent-decoding: `%` must be followed by two hex digits; bytes
/// must form valid UTF-8. Rejects anything else.
fn decode_segment(segment: &str) -> errors::Result<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(invalid("truncated percent-encoding"));
                }
                let hi = hex(bytes[i + 1])?;
                let lo = hex(bytes[i + 2])?;
                out.push(hi << 4 | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| invalid("percent-decoded bytes are not UTF-8"))
}

fn hex(byte: u8) -> errors::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(invalid("malformed percent-encoding")),
    }
}

fn invalid(message: impl Into<String>) -> KernelError {
    KernelError::new(
        ErrorCode::InvalidArgument,
        RetryClass::Never,
        message.into(),
    )
}
