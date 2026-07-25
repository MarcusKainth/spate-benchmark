package dev.kainth.spatebench.flink;

import java.time.LocalDateTime;
import java.util.List;

/**
 * One tier-B output row. Field order mirrors {@code sensor_events_t} in
 * {@code common/clickhouse/ddl.sql}.
 *
 * <p>See {@link SensorRow} for why this is a mutable POJO and why re-using one
 * instance across the fan-out is safe.
 */
public final class SensorRowT {

    public long batchId;
    public int eventSeq;
    public String sensor;
    public String region;
    public String nameUpper;
    public String unit;
    public long value;
    public long valueScaled;
    public Double quality;
    public List<String> tags;
    public LocalDateTime batchTs;
    public LocalDateTime sendTs;

    public SensorRowT() {}
}
