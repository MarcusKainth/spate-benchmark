package dev.kainth.spatebench.flink;

import java.time.LocalDateTime;
import java.util.List;

/**
 * One tier-A output row. Field order mirrors {@code sensor_events} in
 * {@code common/clickhouse/ddl.sql}, which is the wire contract.
 *
 * <p>A plain mutable POJO with public fields and a public no-arg constructor, so
 * Flink's type extractor resolves it as a POJO rather than falling back to Kryo.
 * In practice no serializer runs at all: source, flatMap and sink writer share one
 * operator chain, so the row never crosses a network or serialization boundary.
 *
 * <p>{@link FlattenTierA} re-uses a single instance across the fan-out, which is
 * only legal because (a) {@code pipeline.object-reuse} is on and the chain hands
 * the reference straight to the sink writer, and (b) the sink's
 * {@code ClickHouseConvertor} copies every field into its own payload map and
 * serialises it before returning. Every value stored here is immutable
 * ({@code String}, boxed primitives, {@code LocalDateTime}) or freshly allocated
 * ({@code tags}), so no buffered payload can observe a later mutation.
 */
public final class SensorRow {

    public long batchId;
    public int eventSeq;
    public String sensor;
    public String region;
    public String name;
    public String unit;
    public long value;
    /** Nullable(Float64): null must survive as null. */
    public Double quality;
    public List<String> tags;
    public LocalDateTime batchTs;
    public LocalDateTime sendTs;

    public SensorRow() {}
}
