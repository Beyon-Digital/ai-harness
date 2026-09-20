//! WRK-001: parser corpus, traversal rejection, resolver gating.

use domain::generated::contract::ExecutionContext;
use errors::codes::ErrorCode;
use proptest::prelude::*;
use resource_uri::{
    DenyAllGate, PermitSetGate, ResolvedResource, ResourceResolver, ResourceUri,
};
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;

fn ctx() -> ExecutionContext {
    ExecutionContext {
        call_id: String::new(),
        operation_id: String::new(),
        effect_id: String::new(),
        actor_id: String::new(),
        run_id: String::new(),
        delegation_chain_ref: String::new(),
        deadline_unix_ms: 0,
        fencing_token: 0,
        capability_grant_ids: Vec::new(),
    }
}

fn workspace_uri() -> (domain::ids::WorkspaceId, String) {
    let ids = DeterministicIds::new(SEED);
    let id = domain::ids::WorkspaceId::new(&ids);
    (id, format!("workspace://{id}/src/lib.rs"))
}

#[test]
fn canonical_forms_round_trip() {
    let (wid, text) = workspace_uri();
    let uri = ResourceUri::parse(&text).expect("parse");
    assert_eq!(uri.scheme(), "workspace");
    assert_eq!(uri.to_string(), text);
    assert_eq!(
        uri,
        ResourceUri::Workspace {
            workspace_id: wid,
            relative_path: "src/lib.rs".to_owned()
        }
    );

    let ids = DeterministicIds::new(SEED + 1);
    for (text, scheme) in [
        (format!("run://{}", domain::ids::RunId::new(&ids)), "run"),
        (format!("task://{}", domain::ids::TaskId::new(&ids)), "task"),
        (
            format!("session://{}", domain::ids::SessionId::new(&ids)),
            "session",
        ),
        (
            format!("sandbox://{}", domain::ids::SandboxId::new(&ids)),
            "sandbox",
        ),
        (
            format!("artifact://{}", domain::ids::ArtifactId::new(&ids)),
            "artifact",
        ),
    ] {
        let uri = ResourceUri::parse(&text).expect(text.as_str());
        assert_eq!(uri.scheme(), scheme);
        assert_eq!(uri.to_string(), text);
    }

    let adapter = ResourceUri::parse(&format!(
        "adapter://{}@1.2.3#abc123",
        domain::ids::AdapterId::new(&ids)
    ))
    .expect("adapter uri");
    assert_eq!(adapter.scheme(), "adapter");
    match adapter {
        ResourceUri::Adapter {
            version, digest, ..
        } => {
            assert_eq!(version, "1.2.3");
            assert_eq!(digest, "abc123");
        }
        _ => panic!("adapter variant"),
    }
}

#[test]
fn traversal_corpus_is_rejected() {
    let (wid, _) = workspace_uri();
    for path in [
        "..",
        "../x",
        "a/../b",
        "a/./b",
        "a//b",
        "/abs",
        "a\\b",
        "%2e%2e",
        "%2e%2e/x",
        "x/%2e%2e/y",
        "%2E%2E/x",
        "%2fetc%2fpasswd",
        "x%2f..%2fy",
        "%5c",
        "%",
        "%x1",
        "%1",
        "",
    ] {
        let text = format!("workspace://{wid}/{path}");
        assert_eq!(
            ResourceUri::parse(&text).map_err(|e| e.code()),
            Err(ErrorCode::InvalidArgument),
            "rejected: {path:?}"
        );
    }
    // Encoded separators inside a segment must not survive decoding.
    assert!(
        ResourceUri::parse(&format!("workspace://{wid}/a%2fb")).is_err(),
        "encoded slash rejected"
    );
}

#[test]
fn wrong_scheme_and_shape_are_rejected() {
    for text in [
        "file:///etc/passwd",
        "workspace://not-a-uuid/x",
        "run://run-1",
        "secret://only-one-part",
        "adapter://id@noversion",
        "adapter://id@v#", // empty digest
        "artifact://a/b",
        "noscheme",
        "workspace://",
    ] {
        assert_eq!(
            ResourceUri::parse(text).map_err(|e| e.code()),
            Err(ErrorCode::InvalidArgument),
            "rejected: {text:?}"
        );
    }
}

#[test]
fn resolver_checks_capability_before_dispatch() {
    let (wid, _) = workspace_uri();
    let uri = ResourceUri::parse(&format!("workspace://{wid}/a.txt")).expect("parse");
    let resolver = ResourceResolver::new(PermitSetGate::new(["workspace:read".to_owned()]));
    let resolved = resolver.resolve(&ctx(), &uri).expect("permitted");
    assert_eq!(
        resolved,
        ResolvedResource::WorkspacePath {
            workspace_id: wid,
            relative_path: "a.txt".to_owned()
        }
    );

    // Without the capability the same URI fails before dispatch.
    let denied = ResourceResolver::new(DenyAllGate);
    let err = denied
        .resolve(&ctx(), &uri)
        .expect_err("permission denied");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
}

#[test]
fn secret_resolution_never_exposes_a_path() {
    let uri = ResourceUri::parse("secret://openrouter/key").expect("parse");
    assert_eq!(uri.required_capability(), "secret:use");
    let resolver = ResourceResolver::new(PermitSetGate::new(["secret:use".to_owned()]));
    let resolved = resolver.resolve(&ctx(), &uri).expect("permitted");
    match resolved {
        ResolvedResource::SecretRef { namespace, name } => {
            assert_eq!((namespace, name), ("openrouter".to_owned(), "key".to_owned()));
        }
        other => panic!("unexpected handle: {other:?}"),
    }
}

proptest! {
    /// Any string we render must re-parse to the identical value
    /// (canonical round-trip), and arbitrary junk must not panic.
    #[test]
    fn arbitrary_strings_never_panic(input in ".*") {
        let _ = ResourceUri::parse(&input);
    }

    #[test]
    fn workspace_paths_round_trip(
        segs in prop::collection::vec(
            "[a-z0-9_.-]{1,16}"
                .prop_filter("not a dot segment", |s| s.as_str() != "." && s.as_str() != ".."),
            1..4,
        ),
    ) {
        let ids = DeterministicIds::new(SEED);
        let wid = domain::ids::WorkspaceId::new(&ids);
        let path = segs.join("/");
        let text = format!("workspace://{wid}/{path}");
        let uri = ResourceUri::parse(&text).expect("valid uri parses");
        assert_eq!(uri.to_string(), text);
    }

    #[test]
    fn traversal_segments_always_reject(
        prefix in "[a-z0-9_-]{1,4}",
        dotdot in "\\.\\.|%2e%2e|%2E%2E",
        suffix in "[a-z0-9_-]{1,4}",
    ) {
        let ids = DeterministicIds::new(SEED);
        let wid = domain::ids::WorkspaceId::new(&ids);
        // `..` as a complete path segment, literal or percent-encoded.
        for text in [
            format!("workspace://{wid}/{prefix}/{dotdot}/{suffix}"),
            format!("workspace://{wid}/{dotdot}"),
            format!("workspace://{wid}/{prefix}/{dotdot}"),
        ] {
            prop_assert!(ResourceUri::parse(&text).is_err(), "{text}");
        }
    }
}
