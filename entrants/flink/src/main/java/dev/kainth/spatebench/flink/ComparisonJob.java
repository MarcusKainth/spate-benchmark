package dev.kainth.spatebench.flink;

import com.clickhouse.data.ClickHouseFormat;
import org.apache.avro.Schema;
import org.apache.avro.generic.GenericRecord;
import org.apache.flink.api.common.eventtime.WatermarkStrategy;
import org.apache.flink.api.common.serialization.DeserializationSchema;
import org.apache.flink.connector.clickhouse.convertor.ClickHouseConvertor;
import org.apache.flink.connector.clickhouse.convertor.DataMapper;
import org.apache.flink.connector.clickhouse.sink.ClickHouseAsyncSink;
import org.apache.flink.connector.clickhouse.sink.ClickHouseClientConfig;
import org.apache.flink.connector.kafka.source.KafkaSource;
import org.apache.flink.connector.kafka.source.enumerator.initializer.OffsetsInitializer;
import org.apache.flink.formats.avro.registry.confluent.ConfluentRegistryAvroDeserializationSchema;
import org.apache.flink.streaming.api.datastream.DataStreamSource;
import org.apache.flink.streaming.api.environment.StreamExecutionEnvironment;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * The Flink arm of the cross-framework comparison: consume the Confluent-framed
 * Avro topic, flatten each message's {@code events} array, insert one row per event
 * into ClickHouse.
 *
 * <p>{@code METHODOLOGY.md} is normative and this job conforms to
 * it. Three consequences worth stating where they are easy to check:
 *
 * <ul>
 *   <li><b>No instrumentation.</b> Nothing here counts, times or reports anything
 *       for the benchmark's benefit. Throughput comes from {@code SELECT count()},
 *       CPU and memory from cgroup v2, latency from {@code ingest_ts - send_ts}
 *       computed inside ClickHouse. The Flink and connector metrics that do exist
 *       are the ones they ship in production and are not read as results.</li>
 *   <li><b>No self-termination.</b> The source is unbounded and the job runs until
 *       the driver removes the container, exactly as every other arm does.</li>
 *   <li><b>Framework internals are not hand-written.</b> Decoding goes through
 *       Flink's own {@link ConfluentRegistryAvroDeserializationSchema} and the sink
 *       through ClickHouse's own connector. Tuning lives in {@code config.yaml} and
 *       in the env-driven sink batch settings below, because configuration is not
 *       code we wrote. The one exception is opt-in, labelled and never the headline:
 *       {@code DESER=reusing} selects {@link ReusingAvroDeserializationSchema}.</li>
 * </ul>
 *
 * <p>Environment:
 * <ul>
 *   <li>{@code TIER} ({@code a}) — {@code a} is decode/flatten/insert into
 *       {@code sensor_events}; {@code b} adds the specified filters and derivations
 *       and targets {@code sensor_events_t}.</li>
 *   <li>{@code DESER} ({@code shipped}) — {@code shipped} is the published number;
 *       {@code reusing} is the secondary arm that quantifies the per-message record
 *       allocation in Flink's shipped schema.</li>
 *   <li>{@code BOOTSTRAP}, {@code TOPIC}, {@code GROUP_ID}, {@code STARTING_OFFSETS}
 *       ({@code earliest}|{@code committed}).</li>
 *   <li>{@code REGISTRY_URL}.</li>
 *   <li>{@code CLICKHOUSE_URL}, {@code CLICKHOUSE_USER}, {@code CLICKHOUSE_PASSWORD},
 *       {@code CLICKHOUSE_DATABASE}, {@code CLICKHOUSE_TABLE} (defaults derive from
 *       the tier).</li>
 *   <li>{@code SINK_MAX_BATCH_ROWS}, {@code SINK_MAX_BUFFERED_ROWS},
 *       {@code SINK_MAX_BATCH_BYTES}, {@code SINK_LINGER_MS},
 *       {@code SINK_MAX_IN_FLIGHT}, {@code SINK_MAX_ROW_BYTES} — the sink's batch
 *       shape. See README.md for why each default is what it is.</li>
 * </ul>
 *
 * <p>Parallelism, object reuse, buffer timeout, checkpoint interval and mode, and
 * all memory sizing are deliberately <em>not</em> set here: they live in
 * {@code config.yaml} so a reviewer can read the whole tuning surface in one file
 * without decompiling a jar.
 */
public final class ComparisonJob {

    private ComparisonJob() {}

    public static void main(String[] args) throws Exception {
        final String tier = Cfg.oneOf("TIER", "a", "a", "b");
        final String deser = Cfg.oneOf("DESER", "shipped", "shipped", "reusing");
        final String startingOffsets =
                Cfg.oneOf("STARTING_OFFSETS", "earliest", "earliest", "committed");

        final String schemaJson = SensorBatchSchema.json();
        final Schema schema = SensorBatchSchema.parse(schemaJson);
        // Fail at submission if the committed schema no longer matches the
        // positional constants, rather than at the first record on a task manager.
        SensorBatchSchema.assertFieldOrder(schema);

        final String defaultTable = "a".equals(tier) ? "sensor_events" : "sensor_events_t";
        final String table = Cfg.str("CLICKHOUSE_TABLE", defaultTable);

        System.out.printf(
                "flink arm: tier=%s deser=%s table=%s startingOffsets=%s format=%s%n",
                tier, deser, table, startingOffsets, ClickHouseFormat.RowBinaryWithNamesAndTypes);

        final StreamExecutionEnvironment env = StreamExecutionEnvironment.getExecutionEnvironment();

        final KafkaSource<GenericRecord> source =
                kafkaSource(schema, schemaJson, deser, startingOffsets);

        // No per-operator setParallelism anywhere in this job: every operator runs
        // at parallelism.default from config.yaml, which is what keeps the whole
        // pipeline FORWARD-connected and therefore in one chain.
        final DataStreamSource<GenericRecord> batches =
                env.fromSource(source, WatermarkStrategy.noWatermarks(), "kafka-sensor-batches");
        batches.uid("kafka-sensor-batches");

        if ("a".equals(tier)) {
            batches.flatMap(new FlattenTierA(schemaJson))
                    .name("flatten-tier-a")
                    .uid("flatten-tier-a")
                    .sinkTo(sink(SensorRow.class, new TierAMapper(), table))
                    .name("clickhouse-" + table)
                    .uid("clickhouse-sink-a");
        } else {
            batches.flatMap(new FlattenTierB(schemaJson))
                    .name("flatten-tier-b")
                    .uid("flatten-tier-b")
                    .sinkTo(sink(SensorRowT.class, new TierBMapper(), table))
                    .name("clickhouse-" + table)
                    .uid("clickhouse-sink-b");
        }

        env.execute("comparison-flink-tier-" + tier + "-" + deser);
    }

    // OffsetResetStrategy is deprecated in kafka-clients 4.x (superseded by
    // AutoOffsetResetStrategy), but OffsetsInitializer.committedOffsets in
    // flink-connector-kafka 5.0.0-2.2 still takes only that type — there is no
    // non-deprecated way to express "resume, else earliest" through the connector's
    // public API. Suppressed rather than avoided so the STARTING_OFFSETS=committed
    // path keeps Spate's exact semantics available.
    @SuppressWarnings("deprecation")
    private static KafkaSource<GenericRecord> kafkaSource(
            Schema schema, String schemaJson, String deser, String startingOffsets) {

        final String registryUrl = Cfg.str("REGISTRY_URL", "http://spate-bench-redpanda:8081");

        // setValueOnlyDeserializer, not setDeserializer: the pipeline needs nothing
        // from the Kafka record envelope, and the value-only path skips wrapping each
        // record in a ConsumerRecord view.
        //
        // The produced type is GenericRecordAvroTypeInfo (both schemas report it), so
        // a chain break here would cost Avro serialization rather than Kryo — but it
        // would still serialize the schema-resolved record on every hop, which is why
        // the job is one chain.
        final DeserializationSchema<GenericRecord> valueSchema = "reusing".equals(deser)
                ? new ReusingAvroDeserializationSchema(schemaJson, registryUrl)
                : ConfluentRegistryAvroDeserializationSchema.forGeneric(schema, registryUrl);

        final OffsetsInitializer offsets = "committed".equals(startingOffsets)
                // Same semantics as the Spate arm's auto.offset.reset=earliest with a
                // stable group id: resume where the group left off, else start at 0.
                ? OffsetsInitializer.committedOffsets(
                        org.apache.kafka.clients.consumer.OffsetResetStrategy.EARLIEST)
                // Default. A drain measurement has to replay the same corpus every
                // run, and this is also flink-connector-kafka's own default.
                : OffsetsInitializer.earliest();

        return KafkaSource.<GenericRecord>builder()
                .setBootstrapServers(Cfg.str("BOOTSTRAP", "spate-bench-redpanda:29092"))
                .setTopics(Cfg.str("TOPIC", "comparison-sensor-batches"))
                .setGroupId(Cfg.str("GROUP_ID", "comparison-flink"))
                .setStartingOffsets(offsets)
                .setValueOnlyDeserializer(valueSchema)
                // The topic's partition count is fixed for the whole comparison, so
                // rediscovery would only add a metadata request every 5 minutes.
                .setProperty("partition.discovery.interval.ms", "-1")
                .build();
    }

    private static <T> ClickHouseAsyncSink<T> sink(
            Class<T> inputType, DataMapper<T> mapper, String table) {

        final ClickHouseClientConfig clientConfig = new ClickHouseClientConfig(
                Cfg.str("CLICKHOUSE_URL", "http://spate-bench-clickhouse:8123"),
                Cfg.str("CLICKHOUSE_USER", "default"),
                Cfg.str("CLICKHOUSE_PASSWORD", "bench"),
                Cfg.str("CLICKHOUSE_DATABASE", "default"),
                table);

        // Matches the Spate arm's `settings: { async_insert: "0" }`. It is also the
        // server default, and is set explicitly so the two arms cannot diverge if a
        // future ClickHouse release changes that default under both of them.
        final Map<String, String> serverSettings = new LinkedHashMap<>();
        serverSettings.put("async_insert", "0");
        clientConfig.setServerSettings(serverSettings);

        // Typed (POJO) mode. The connector forces RowBinaryWithNamesAndTypes here and
        // ignores setClickHouseFormat, so the format is not set: passing anything else
        // would only produce a warning. The alternative shipped path is String mode,
        // which would mean building CSV or JSONEachRow text per row and moving work to
        // the server — measurably worse, and not what a competent user would deploy
        // for a typed stream.
        final ClickHouseConvertor<T> convertor = new ClickHouseConvertor<>(inputType, mapper);

        final int maxBatchRows = Cfg.i("SINK_MAX_BATCH_ROWS", 25_000);
        final int maxBufferedRows = Cfg.i("SINK_MAX_BUFFERED_ROWS", 50_000);
        if (maxBufferedRows <= maxBatchRows) {
            // AsyncSinkWriter enforces this, but its message names neither knob.
            throw new IllegalArgumentException(
                    "SINK_MAX_BUFFERED_ROWS (" + maxBufferedRows
                            + ") must be strictly greater than SINK_MAX_BATCH_ROWS ("
                            + maxBatchRows + ")");
        }

        return ClickHouseAsyncSink.<T>builder()
                .setElementConverter(convertor)
                .setClickHouseClientConfig(clientConfig)
                .setMaxBatchSize(maxBatchRows)
                .setMaxBufferedRequests(maxBufferedRows)
                .setMaxBatchSizeInBytes(Cfg.l("SINK_MAX_BATCH_BYTES", 16L * 1024 * 1024))
                .setMaxRecordSizeInBytes(Cfg.l("SINK_MAX_ROW_BYTES", 1L * 1024 * 1024))
                .setMaxTimeInBufferMS(Cfg.l("SINK_LINGER_MS", 1_000L))
                .setMaxInFlightRequests(Cfg.i("SINK_MAX_IN_FLIGHT", 2))
                .build();
    }
}
