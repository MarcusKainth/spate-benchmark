//! Kafka admin operations the driver needs: topic creation with a partition
//! count it can trust.
//!
//! Forked from `etl-rs/benchmarks/src/lib.rs` at `f41280d51165`.

use std::time::{Duration, Instant};

use rdkafka::ClientConfig;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::consumer::{BaseConsumer, Consumer};

/// Creates `topic` with `partitions` partitions.
///
/// An existing topic is reused, but only after its partition count is checked
/// against the request. Silently accepting a mismatch is how a sweep ends up
/// measuring one shape while recording another: rigs that pin a topic name and
/// vary `PARTITIONS` would create the topic on the first arm and every later
/// arm would reuse it, reporting the swept value it never ran at.
pub fn ensure_topic(bootstrap: &str, topic: &str, partitions: i32) {
    ensure_topic_with(bootstrap, topic, partitions, &[]);
}

/// [`ensure_topic`] plus topic-level configuration entries (retention, segment
/// sizing) applied at creation.
///
/// Config is only honoured on a **fresh** topic: an existing one keeps
/// whatever it was created with, and the partition check below says nothing
/// about it. A topic first created by plain [`ensure_topic`] and later reused
/// here therefore runs without these settings — delete it if the retention
/// shape matters to the measurement.
pub fn ensure_topic_with(bootstrap: &str, topic: &str, partitions: i32, configs: &[(&str, &str)]) {
    let admin: AdminClient<DefaultClientContext> = ClientConfig::new()
        .set("bootstrap.servers", bootstrap)
        .create()
        .expect("admin client");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut new_topic = NewTopic::new(topic, partitions, TopicReplication::Fixed(1));
    for (key, value) in configs {
        new_topic = new_topic.set(key, value);
    }
    let results = rt
        .block_on(admin.create_topics(&[new_topic], &AdminOptions::new()))
        .expect("create_topics call");
    for result in results {
        match result {
            Ok(_) => {}
            Err((name, rdkafka::types::RDKafkaErrorCode::TopicAlreadyExists)) => {
                let actual = topic_partitions(bootstrap, &name);
                assert_eq!(
                    actual, partitions,
                    "topic {name} already exists with {actual} partitions, but this run \
                     asked for {partitions}. Reusing it would measure {actual} while \
                     recording {partitions}; delete the topic or pick another name."
                );
                eprintln!("topic {name} already exists with {actual} partitions (matches)");
            }
            Err((name, code)) => panic!("failed to create topic {name}: {code}"),
        }
    }
}

/// Partition count of an existing topic, from the broker's metadata.
///
/// Retries while the broker reports a topic-level error: immediately after a
/// concurrent creation it can answer `LEADER_NOT_AVAILABLE` with an empty
/// partition list, which would otherwise read as "this topic has 0 partitions"
/// and abort a perfectly good run.
fn topic_partitions(bootstrap: &str, topic: &str) -> i32 {
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", bootstrap)
        .create()
        .expect("metadata consumer");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let metadata = consumer
            .fetch_metadata(Some(topic), Duration::from_secs(10))
            .unwrap_or_else(|e| panic!("fetch metadata for {topic}: {e}"));
        let t = metadata
            .topics()
            .iter()
            .find(|t| t.name() == topic)
            .unwrap_or_else(|| panic!("topic {topic} missing from metadata"));
        match t.error() {
            None if !t.partitions().is_empty() => {
                return i32::try_from(t.partitions().len()).expect("partition count");
            }
            other => {
                assert!(
                    Instant::now() < deadline,
                    "topic {topic} metadata never settled (last error {other:?}, \
                     {} partitions)",
                    t.partitions().len()
                );
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}
