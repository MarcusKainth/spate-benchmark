package dev.kainth.spatebench.flink;

import org.apache.avro.generic.GenericRecord;
import org.apache.flink.api.common.functions.OpenContext;
import org.apache.flink.api.common.functions.RichFlatMapFunction;
import org.apache.flink.util.Collector;

import java.time.LocalDateTime;
import java.util.List;

/**
 * Tier A: one {@code SensorBatch} message becomes {@code events.length} rows.
 *
 * <p>Column mapping only, plus the one piece of real work tier A carries: a null
 * {@code region} is coalesced to {@code ""}, because the target column is
 * {@code LowCardinality(String)} rather than {@code LowCardinality(Nullable(String))}.
 */
public final class FlattenTierA extends RichFlatMapFunction<GenericRecord, SensorRow> {

    private static final long serialVersionUID = 1L;

    private final String schemaJson;

    /**
     * Re-used across the fan-out; see {@link SensorRow} for why that is safe.
     * Transient because it is per-subtask state, created in {@link #open}.
     */
    private transient SensorRow row;

    public FlattenTierA(String schemaJson) {
        this.schemaJson = schemaJson;
    }

    @Override
    public void open(OpenContext openContext) {
        SensorBatchSchema.assertFieldOrder(SensorBatchSchema.parse(schemaJson));
        row = new SensorRow();
    }

    @Override
    public void flatMap(GenericRecord batch, Collector<SensorRow> out) {
        long batchId = (Long) batch.get(SensorBatchSchema.BATCH_ID);
        String sensor = batch.get(SensorBatchSchema.SENSOR).toString();
        Object regionRaw = batch.get(SensorBatchSchema.REGION);
        String region = regionRaw == null ? "" : regionRaw.toString();

        // Hoisted out of the event loop: both timestamps are per-message, so one
        // LocalDateTime pair is shared by all rows of the batch. LocalDateTime is
        // immutable, so sharing it across buffered payloads is safe.
        LocalDateTime batchTs =
                SensorBatchSchema.fromEpochMillis((Long) batch.get(SensorBatchSchema.BATCH_TS_MS));
        LocalDateTime sendTs =
                SensorBatchSchema.fromEpochMicros((Long) batch.get(SensorBatchSchema.SEND_TS_US));

        List<?> events = (List<?>) batch.get(SensorBatchSchema.EVENTS);
        for (int i = 0, n = events.size(); i < n; i++) {
            GenericRecord ev = (GenericRecord) events.get(i);
            SensorRow r = row;
            r.batchId = batchId;
            r.eventSeq = (Integer) ev.get(SensorBatchSchema.EV_SEQ);
            r.sensor = sensor;
            r.region = region;
            r.name = ev.get(SensorBatchSchema.EV_NAME).toString();
            r.unit = ev.get(SensorBatchSchema.EV_UNIT).toString();
            r.value = (Long) ev.get(SensorBatchSchema.EV_VALUE);
            r.quality = (Double) ev.get(SensorBatchSchema.EV_QUALITY);
            r.tags = Rows.tags(ev.get(SensorBatchSchema.EV_TAGS));
            r.batchTs = batchTs;
            r.sendTs = sendTs;
            out.collect(r);
        }
    }
}
