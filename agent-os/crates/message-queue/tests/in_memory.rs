use domain::generated::contract::{
    ExecutionContext, QueueAckRequest, QueueConsumeRequest, QueueNackRequest, QueuePublishRequest,
};
use errors::codes::ErrorCode;
use message_queue::{InMemoryQueue, MessageQueuePort};

fn publish(stream: &str, id: &str, payload: &[u8]) -> QueuePublishRequest {
    QueuePublishRequest {
        context: Some(ExecutionContext::default()),
        stream: stream.to_owned(),
        message_id: id.to_owned(),
        payload: payload.to_vec(),
        options_json: Vec::new(),
    }
}

fn consume(subscription: &str, max: u32) -> QueueConsumeRequest {
    QueueConsumeRequest {
        context: Some(ExecutionContext::default()),
        subscription: subscription.to_owned(),
        options_json: format!("{{\"max\":{max}}}").into_bytes(),
    }
}

fn ack(delivery_id: &str) -> QueueAckRequest {
    QueueAckRequest {
        context: Some(ExecutionContext::default()),
        delivery_id: delivery_id.to_owned(),
    }
}

fn nack(delivery_id: &str) -> QueueNackRequest {
    QueueNackRequest {
        context: Some(ExecutionContext::default()),
        delivery_id: delivery_id.to_owned(),
        options_json: Vec::new(),
    }
}

#[tokio::test]
async fn publish_consume_ack_happy_path() {
    let queue = InMemoryQueue::default();
    let result = queue
        .publish(publish("stream-a", "m-1", b"payload-1"))
        .await
        .expect("publish");
    assert_eq!(result.message_id, "m-1");

    let deliveries = queue.consume(&consume("worker", 1)).await.expect("consume");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].message_id, "m-1");
    assert_eq!(deliveries[0].payload, b"payload-1");

    queue
        .ack(&ack(&deliveries[0].delivery_id))
        .await
        .expect("ack");
    // Acked messages are gone; nothing redelivers.
    let again = queue.consume(&consume("worker", 1)).await.expect("empty");
    assert!(again.is_empty());
}

#[tokio::test]
async fn nack_redelivers_the_message() {
    let queue = InMemoryQueue::default();
    queue
        .publish(publish("s", "m-1", b"x"))
        .await
        .expect("publish");
    let first = queue.consume(&consume("w", 1)).await.expect("consume");
    queue
        .nack(&nack(&first[0].delivery_id))
        .await
        .expect("nack");
    let second = queue.consume(&consume("w", 1)).await.expect("reconsume");
    assert_eq!(second[0].message_id, "m-1");
    assert_ne!(second[0].delivery_id, first[0].delivery_id);
    queue.ack(&ack(&second[0].delivery_id)).await.expect("ack");
}

#[tokio::test]
async fn bounded_capacity_applies_backpressure() {
    let queue = InMemoryQueue::new(3);
    for i in 0..3 {
        queue
            .publish(publish("s", &format!("m-{i}"), b"x"))
            .await
            .expect("publish within capacity");
    }
    let err = queue
        .publish(publish("s", "m-overflow", b"x"))
        .await
        .expect_err("capacity");
    assert_eq!(err.code(), ErrorCode::ResourceExhausted);
    // Space frees once an in-flight delivery is acked.
    let d = queue.consume(&consume("w", 1)).await.expect("consume");
    queue.ack(&ack(&d[0].delivery_id)).await.expect("ack");
    queue
        .publish(publish("s", "m-overflow", b"x"))
        .await
        .expect("publish after ack");
}

#[tokio::test]
async fn duplicate_message_id_is_idempotent_or_conflicts() {
    let queue = InMemoryQueue::default();
    queue
        .publish(publish("s", "m-1", b"same"))
        .await
        .expect("first");
    let replay = queue
        .publish(publish("s", "m-1", b"same"))
        .await
        .expect("idempotent republish");
    assert_eq!(replay.message_id, "m-1");
    let conflict = queue
        .publish(publish("s", "m-1", b"different"))
        .await
        .expect_err("conflict");
    assert_eq!(conflict.code(), ErrorCode::Conflict);
}

#[tokio::test]
async fn unknown_delivery_ids_are_not_found() {
    let queue = InMemoryQueue::default();
    let err = queue.ack(&ack("d-999")).await.expect_err("ack unknown");
    assert_eq!(err.code(), ErrorCode::NotFound);
    let err = queue.nack(&nack("d-999")).await.expect_err("nack unknown");
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn capabilities_truthfully_report_non_durable() {
    let queue = InMemoryQueue::default();
    let caps = queue.capabilities();
    assert!(!caps.durable);
    assert!(!caps.replay);
    assert!(!caps.cross_restart);
}
