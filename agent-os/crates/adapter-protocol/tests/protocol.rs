//! ADP-002 protocol behavior: framing, handshake identity pinning, order,
//! deadlines, cancel.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use adapter_protocol::{
    CallError, ExpectedIdentity, MAX_FRAME_BYTES, SessionPhase, accept_inbound, dispatch_call,
    encode_frame, read_frame, write_frame,
};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPing, AdapterPong, CancelCall, PortCallRequest,
    PortCallResponse, adapter_frame::Body,
};
use domain::ids::DaemonInstanceId;
use errors::codes::ErrorCode;

fn identity() -> ExpectedIdentity {
    let provider = testkit::ids::DeterministicIds::new(1);
    ExpectedIdentity {
        daemon_instance_id: DaemonInstanceId::new(&provider),
        daemon_fencing_epoch: 7,
        adapter_instance_id: "inst-1".to_owned(),
        adapter_id: "fixture.increment".to_owned(),
        adapter_version: "1.0.0".to_owned(),
        expected_bundle_digest: "sha256:abc".to_owned(),
        protocol_version: 1,
    }
}

fn hello(identity: &ExpectedIdentity) -> AdapterHello {
    AdapterHello {
        adapter_instance_id: identity.adapter_instance_id.clone(),
        adapter_id: identity.adapter_id.clone(),
        adapter_version: identity.adapter_version.clone(),
        bundle_digest: identity.expected_bundle_digest.clone(),
        protocol_version: identity.protocol_version,
        implemented_ports: vec![],
        capability_document: vec![],
    }
}

#[test]
fn wrong_digest_handshake_fails() {
    let identity = identity();
    let mut bad = hello(&identity);
    bad.bundle_digest = "sha256:evil".to_owned();
    let err = expected_err(identity.verify_hello(&bad, "n"));
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
}

#[test]
fn wrong_instance_nonce_protocol_fails() {
    let identity = identity();
    let mut bad = hello(&identity);
    bad.adapter_instance_id = "inst-2".to_owned();
    expected_err(identity.verify_hello(&bad, "n"));

    let mut bad = hello(&identity);
    bad.protocol_version = 99;
    let err = expected_err(identity.verify_hello(&bad, "n"));
    assert!(err.message().contains("protocol version"));
}

#[test]
fn oversized_frame_rejected() {
    let (mut a, mut b) = UnixStream::pair().expect("pair");
    a.write_all(&((MAX_FRAME_BYTES as u32 + 1).to_be_bytes()))
        .expect("header");
    let err = expected_err(read_frame(&mut b));
    assert_eq!(err.code(), ErrorCode::ResourceExhausted);
}

#[test]
fn unexpected_order_frames_rejected() {
    let frame = |body| AdapterFrame { body: Some(body) };
    // Request before hello: order violation.
    let req = frame(Body::Request(PortCallRequest {
        call_id: "c".to_owned(),
        port_id: "p".to_owned(),
        operation: "o".to_owned(),
        context: None,
        payload: vec![],
    }));
    assert!(accept_inbound(SessionPhase::AwaitingHello, &req).is_err());
    // Response while ready: ok.
    let resp = frame(Body::Response(PortCallResponse {
        call_id: "c".to_owned(),
        payload: vec![],
        error_code: String::new(),
    }));
    assert!(accept_inbound(SessionPhase::Ready, &resp).is_ok());
    // Kernel-side frame inbound: rejected.
    let boot = identity().bootstrap_frame("n");
    assert!(accept_inbound(SessionPhase::Ready, &boot).is_err());
    // After shutdown: nothing admitted.
    assert!(accept_inbound(SessionPhase::Draining, &resp).is_err());
    // Body-less frame: malformed.
    assert!(accept_inbound(SessionPhase::Ready, &AdapterFrame { body: None }).is_err());
}

#[test]
fn request_response_roundtrip() {
    let (mut kernel, mut child) = UnixStream::pair().expect("pair");
    let request = PortCallRequest {
        call_id: "c1".to_owned(),
        port_id: "effect.execute".to_owned(),
        operation: "increment".to_owned(),
        context: None,
        payload: b"x".to_vec(),
    };
    let responder = std::thread::spawn(move || {
        let frame = read_frame(&mut child).expect("read").expect("frame");
        let Some(Body::Request(req)) = frame.body else {
            panic!("expected request");
        };
        write_frame(
            &mut child,
            &AdapterFrame {
                body: Some(Body::Response(PortCallResponse {
                    call_id: req.call_id,
                    payload: b"y".to_vec(),
                    error_code: String::new(),
                })),
            },
        )
        .expect("respond");
    });
    let mut phase = SessionPhase::Ready;
    let resp = dispatch_call(
        &mut kernel,
        &mut phase,
        request,
        Instant::now() + Duration::from_secs(5),
    )
    .expect("response");
    assert_eq!(resp.payload, b"y");
    responder.join().expect("join");
}

#[test]
fn cancel_frame_delivered() {
    let (mut kernel, mut child) = UnixStream::pair().expect("pair");
    let request = PortCallRequest {
        call_id: "c9".to_owned(),
        port_id: "p".to_owned(),
        operation: "o".to_owned(),
        context: None,
        payload: vec![],
    };
    let canceller = std::thread::spawn(move || {
        let _ = read_frame(&mut child).expect("read"); // the request
        write_frame(
            &mut child,
            &AdapterFrame {
                body: Some(Body::Cancel(CancelCall {
                    call_id: "c9".to_owned(),
                })),
            },
        )
        .expect("cancel");
    });
    let mut phase = SessionPhase::Ready;
    let outcome = dispatch_call(
        &mut kernel,
        &mut phase,
        request,
        Instant::now() + Duration::from_secs(5),
    );
    assert!(matches!(outcome, Err(CallError::Cancelled(id)) if id == "c9"));
    canceller.join().expect("join");
}

#[test]
fn deadline_closes_request() {
    let (mut kernel, mut child) = UnixStream::pair().expect("pair");
    let request = PortCallRequest {
        call_id: "slow".to_owned(),
        port_id: "p".to_owned(),
        operation: "o".to_owned(),
        context: None,
        payload: vec![],
    };
    let reader = std::thread::spawn(move || {
        let _ = read_frame(&mut child).expect("read"); // request
        let cancel = read_frame(&mut child).expect("read").expect("cancel frame");
        let Some(Body::Cancel(CancelCall { call_id })) = cancel.body else {
            panic!("expected CancelCall after deadline");
        };
        assert_eq!(call_id, "slow");
    });
    let mut phase = SessionPhase::Ready;
    let outcome = dispatch_call(
        &mut kernel,
        &mut phase,
        request,
        Instant::now() + Duration::from_millis(150),
    );
    let Err(CallError::Kernel(e)) = outcome else {
        panic!("expected deadline failure, got {outcome:?}");
    };
    assert_eq!(e.code(), ErrorCode::Unavailable);
    reader.join().expect("join");
}

#[test]
fn ping_pong_answered_during_call() {
    let (mut kernel, mut child) = UnixStream::pair().expect("pair");
    let request = PortCallRequest {
        call_id: "c1".to_owned(),
        port_id: "p".to_owned(),
        operation: "o".to_owned(),
        context: None,
        payload: vec![],
    };
    let peer = std::thread::spawn(move || {
        let _ = read_frame(&mut child).expect("read"); // request
        write_frame(
            &mut child,
            &AdapterFrame {
                body: Some(Body::Ping(AdapterPing {
                    nonce: "n1".to_owned(),
                })),
            },
        )
        .expect("ping");
        let pong = read_frame(&mut child).expect("read").expect("pong");
        let Some(Body::Pong(AdapterPong { nonce })) = pong.body else {
            panic!("expected pong");
        };
        assert_eq!(nonce, "n1");
        write_frame(
            &mut child,
            &AdapterFrame {
                body: Some(Body::Response(PortCallResponse {
                    call_id: "c1".to_owned(),
                    payload: b"ok".to_vec(),
                    error_code: String::new(),
                })),
            },
        )
        .expect("respond");
    });
    let mut phase = SessionPhase::Ready;
    let resp = dispatch_call(
        &mut kernel,
        &mut phase,
        request,
        Instant::now() + Duration::from_secs(5),
    )
    .expect("response");
    assert_eq!(resp.payload, b"ok");
    peer.join().expect("join");
}

#[test]
fn encode_roundtrip_and_clean_eof() {
    let frame = identity().bootstrap_frame("nonce-1");
    let bytes = encode_frame(&frame);
    let (mut a, mut b) = UnixStream::pair().expect("pair");
    a.write_all(&bytes).expect("write");
    let got = read_frame(&mut b).expect("read").expect("frame");
    assert_eq!(got, frame);
    drop(a);
    assert!(read_frame(&mut b).expect("eof").is_none());
}

fn expected_err<T>(r: errors::Result<T>) -> errors::KernelError {
    r.map(|_| ()).expect_err("expected error")
}
