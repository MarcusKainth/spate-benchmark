package dev.kainth.spatebench.flink;

import com.clickhouse.data.ClickHouseColumn;
import com.clickhouse.data.ClickHouseDataType;
import org.apache.flink.connector.clickhouse.convertor.ColumnBinding;
import org.apache.flink.connector.clickhouse.convertor.DataMapper;

import java.util.List;
import java.util.Map;

/**
 * The tier-B column contract, mirroring {@code sensor_events_t} in
 * {@code common/clickhouse/ddl.sql}. See {@link TierAMapper} for the details that
 * apply to both tiers.
 *
 * <p>Note the column is {@code name_upper}, not {@code name} — tier B replaces the
 * raw metric name with its ASCII uppercase.
 */
public final class TierBMapper extends DataMapper<SensorRowT> {

    private static final long serialVersionUID = 1L;

    @Override
    public void toMap(SensorRowT r, Map<String, Object> m) {
        m.put("batch_id", r.batchId);
        m.put("event_seq", r.eventSeq);
        m.put("sensor", r.sensor);
        m.put("region", r.region);
        m.put("name_upper", r.nameUpper);
        m.put("unit", r.unit);
        m.put("value", r.value);
        m.put("value_scaled", r.valueScaled);
        m.put("quality", r.quality);
        m.put("tags", r.tags);
        m.put("batch_ts", r.batchTs);
        m.put("send_ts", r.sendTs);
    }

    @Override
    public List<ColumnBinding> bindings() {
        return List.of(
                ColumnBinding.scalar("batch_id", "batch_id", ClickHouseDataType.UInt64),
                ColumnBinding.scalar("event_seq", "event_seq", ClickHouseDataType.UInt16),
                TierAMapper.lowCardinality("sensor"),
                TierAMapper.lowCardinality("region"),
                TierAMapper.lowCardinality("name_upper"),
                TierAMapper.lowCardinality("unit"),
                ColumnBinding.scalar("value", "value", ClickHouseDataType.Int64),
                ColumnBinding.scalar("value_scaled", "value_scaled", ClickHouseDataType.Int64),
                ColumnBinding.scalar("quality", "quality", ClickHouseDataType.Float64, true, false),
                ColumnBinding.array("tags", "tags",
                        ClickHouseColumn.of("tags", "LowCardinality(String)")),
                ColumnBinding.dateTime64("batch_ts", "batch_ts", 3),
                ColumnBinding.dateTime64("send_ts", "send_ts", 6));
    }
}
