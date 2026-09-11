//! R18.1, R18.2, R18.3, N1: secrets never render through Debug or Display,
//! classification ordering supports threshold redaction, and the kernel span
//! helper attaches correlation identifiers.

use std::io::Write;
use std::sync::{Arc, Mutex};

use observability::classification::{Classification, Classified, Redacted, Secret};
use observability::{KernelFields, init_tracing, kernel_span};

const SECRET_VALUE: &str = "hunter2";

#[test]
fn secret_debug_never_renders_the_value() {
    let rendered = format!("{:?}", Secret::new(SECRET_VALUE));
    assert!(
        !rendered.contains(SECRET_VALUE),
        "Debug leaked the secret: {rendered}"
    );
    assert_eq!(rendered, "[REDACTED]");
}

#[test]
fn secret_display_never_renders_the_value() {
    let rendered = format!("{}", Secret::new(SECRET_VALUE));
    assert!(
        !rendered.contains(SECRET_VALUE),
        "Display leaked the secret: {rendered}"
    );
    assert_eq!(rendered, "[REDACTED]");
}

#[test]
fn secret_expose_is_the_only_access_path() {
    let secret = Secret::new(SECRET_VALUE.to_string());
    assert_eq!(secret.expose(), SECRET_VALUE);
}

#[test]
fn redacted_debug_hides_but_access_is_allowed() {
    let redacted = Redacted::new("x".to_string());
    assert_eq!(format!("{redacted:?}"), "[REDACTED]");
    assert_eq!(redacted.get(), "x");
    assert_eq!(redacted.into_inner(), "x");
}

#[test]
fn classification_orders_public_below_secret() {
    assert!(Classification::Public < Classification::Internal);
    assert!(Classification::Internal < Classification::Confidential);
    assert!(Classification::Confidential < Classification::Secret);
    assert!(Classification::Public < Classification::Secret);
    assert_eq!(Classification::Public as u8, 0);
    assert_eq!(Classification::Secret as u8, 3);
}

#[test]
fn classification_ordering_redacts_at_or_above_sink_threshold() {
    fn redacted_at_sink(level: Classification, sink_threshold: Classification) -> bool {
        level >= sink_threshold
    }

    let sink_threshold = Classification::Confidential;
    assert!(redacted_at_sink(Classification::Secret, sink_threshold));
    assert!(redacted_at_sink(
        Classification::Confidential,
        sink_threshold
    ));
    assert!(!redacted_at_sink(Classification::Internal, sink_threshold));
    assert!(!redacted_at_sink(Classification::Public, sink_threshold));
}

struct ClassifiedThing;

impl Classified for ClassifiedThing {
    fn classification(&self) -> Classification {
        Classification::Confidential
    }
}

#[test]
fn classified_reports_its_classification() {
    assert_eq!(
        ClassifiedThing.classification(),
        Classification::Confidential
    );
}

struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("capture buffer poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_span_output(fields: &KernelFields) -> String {
    let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || SharedWriter(sink.clone()))
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        let span = kernel_span(fields);
        let _guard = span.enter();
        tracing::info!("ping");
    });

    buffer
        .lock()
        .map_err(|_| std::io::Error::other("capture buffer poisoned"))
        .and_then(|bytes| {
            String::from_utf8(bytes.clone()).map_err(|err| std::io::Error::other(err.to_string()))
        })
        .unwrap_or_default()
}

#[test]
fn kernel_span_attaches_kernel_field_ids() {
    let fields = KernelFields {
        correlation_id: Some("corr-1".to_string()),
        run_id: Some("run-1".to_string()),
        task_id: Some("task-1".to_string()),
        effect_id: Some("eff-1".to_string()),
    };
    let output = capture_span_output(&fields);
    assert!(
        output.contains("correlation_id=\"corr-1\""),
        "missing correlation_id: {output}"
    );
    assert!(
        output.contains("run_id=\"run-1\""),
        "missing run_id: {output}"
    );
    assert!(
        output.contains("task_id=\"task-1\""),
        "missing task_id: {output}"
    );
    assert!(
        output.contains("effect_id=\"eff-1\""),
        "missing effect_id: {output}"
    );
}

#[test]
fn kernel_span_omits_absent_kernel_field_ids() {
    let output = capture_span_output(&KernelFields::default());
    for field in ["correlation_id", "run_id", "task_id", "effect_id"] {
        assert!(!output.contains(field), "unexpected {field}: {output}");
    }
}

#[test]
fn init_tracing_installs_a_sink() {
    let result = init_tracing(true);
    assert!(result.is_ok(), "init_tracing returned {result:?}");
}
