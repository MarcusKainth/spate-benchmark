package dev.kainth.spatebench.flink;

import org.apache.avro.Schema;

import java.io.IOException;
import java.io.InputStream;
import java.io.Serializable;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.time.LocalDateTime;
import java.time.ZoneOffset;

/**
 * The one Avro schema, read from {@code workload/schema/sensor_batch.avsc}.
 *
 * <p>The Dockerfile copies that committed file into this jar's resources; the
 * schema is never re-declared here, which {@code methodology/} requires.
 *
 * <p>Field access on the hot path is <em>positional</em> ({@code GenericRecord.get(int)}),
 * because a name lookup per field would cost a hash probe on every one of ~44
 * fields per message. {@link #assertFieldOrder} proves the constants still match
 * the committed file, so a schema edit fails at operator open instead of silently
 * writing the wrong column — the same guard the Spate arm carries as
 * {@code avro_field_order_matches_the_positional_constants}.
 */
final class SensorBatchSchema implements Serializable {

    private static final long serialVersionUID = 1L;

    static final String RESOURCE = "/sensor_batch.avsc";

    // SensorBatch
    static final int BATCH_ID = 0;
    static final int SENSOR = 1;
    static final int REGION = 2;
    static final int BATCH_TS_MS = 3;
    static final int SEND_TS_US = 4;
    static final int EVENTS = 5;

    // SensorBatch.events[].Event
    static final int EV_SEQ = 0;
    static final int EV_NAME = 1;
    static final int EV_UNIT = 2;
    static final int EV_VALUE = 3;
    static final int EV_QUALITY = 4;
    static final int EV_TAGS = 5;

    private SensorBatchSchema() {}

    /** The committed schema text, as it sits in the jar. */
    static String json() {
        try (InputStream in = SensorBatchSchema.class.getResourceAsStream(RESOURCE)) {
            if (in == null) {
                throw new IllegalStateException(
                        RESOURCE + " is not on the classpath. The Dockerfile copies it from "
                                + "workload/schema/sensor_batch.avsc; a build "
                                + "that skips that step would silently decode nothing.");
            }
            return new String(in.readAllBytes(), StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    static Schema parse(String json) {
        return new Schema.Parser().parse(json);
    }

    static Schema read() {
        return parse(json());
    }

    static void assertFieldOrder(Schema batch) {
        expect(batch, BATCH_ID, "batch_id");
        expect(batch, SENSOR, "sensor");
        expect(batch, REGION, "region");
        expect(batch, BATCH_TS_MS, "batch_ts_ms");
        expect(batch, SEND_TS_US, "send_ts_us");
        expect(batch, EVENTS, "events");

        Schema event = batch.getField("events").schema().getElementType();
        expect(event, EV_SEQ, "seq");
        expect(event, EV_NAME, "name");
        expect(event, EV_UNIT, "unit");
        expect(event, EV_VALUE, "value");
        expect(event, EV_QUALITY, "quality");
        expect(event, EV_TAGS, "tags");
    }

    private static void expect(Schema record, int pos, String name) {
        Schema.Field f = record.getField(name);
        if (f == null) {
            throw new IllegalStateException(
                    record.getFullName() + " has no field '" + name + "'");
        }
        if (f.pos() != pos) {
            throw new IllegalStateException(
                    record.getFullName() + "." + name + " moved from position " + pos
                            + " to " + f.pos() + "; the positional constants in "
                            + SensorBatchSchema.class.getName() + " must be updated with the schema.");
        }
    }

    /**
     * Epoch milliseconds to the {@code LocalDateTime} the ClickHouse connector's
     * {@code DataWriter} requires for a {@code DateTime64} column.
     *
     * <p>{@code DataWriter.writeDateTime64} accepts only {@code LocalDateTime} or
     * {@code ZonedDateTime} and serialises with a hardcoded {@code ZoneId.of("UTC")}.
     * A bare {@code Long} in the payload map would throw, and a {@code LocalDateTime}
     * built in any other zone would land offset — which is the same 1970-class trap
     * the shared DDL warns about for the Spate Native encoder. Verified end to end
     * against the live server (whose {@code timezone()} is UTC).
     */
    static LocalDateTime fromEpochMillis(long ms) {
        return LocalDateTime.ofEpochSecond(
                Math.floorDiv(ms, 1_000L),
                (int) (Math.floorMod(ms, 1_000L) * 1_000_000L),
                ZoneOffset.UTC);
    }

    /** Epoch microseconds to {@code LocalDateTime}; see {@link #fromEpochMillis}. */
    static LocalDateTime fromEpochMicros(long us) {
        return LocalDateTime.ofEpochSecond(
                Math.floorDiv(us, 1_000_000L),
                (int) (Math.floorMod(us, 1_000_000L) * 1_000L),
                ZoneOffset.UTC);
    }
}
