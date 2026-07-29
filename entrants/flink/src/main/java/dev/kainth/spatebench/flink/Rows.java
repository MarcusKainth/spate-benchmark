package dev.kainth.spatebench.flink;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * The pipeline logic the contract leaves to each arm: the fan-out's per-field
 * conversions, the workload's filter constants and its derived columns.
 *
 * <p>{@code methodology/} rule 1 splits framework internals (which we may
 * not hand-write) from pipeline logic (which every arm writes). Everything in this
 * class is the latter.
 */
final class Rows {

    /** The filter sentinel; {@code UNITS[3]} in the shared generator. */
    static final String DROP_UNIT = "drop";

    /** The quality floor. */
    static final double QUALITY_FLOOR = 0.2d;

    private Rows() {}

    /**
     * Avro's {@code array<string>} to a {@code List<String>}.
     *
     * <p>Two reasons this cannot be passed through: Avro yields
     * {@code org.apache.avro.util.Utf8}, and (a) the ClickHouse connector's
     * {@code DataWriter} would stringify it with {@code String.valueOf} anyway, and
     * (b) the connector's checkpointed payload map only accepts a fixed set of
     * value types — a {@code Utf8} in there fails at checkpoint serialisation, not
     * at write time. So the conversion is required work, not avoidable overhead.
     *
     * <p>A fresh list per row is deliberate: {@link FlattenEvents} re-uses the row
     * object, and the sink retains whatever reference it is handed until the batch
     * flushes. A shared mutable list would corrupt buffered rows.
     */
    static List<String> tags(Object avroArray) {
        List<?> src = (List<?>) avroArray;
        int n = src.size();
        if (n == 0) {
            return Collections.emptyList();
        }
        List<String> out = new ArrayList<>(n);
        for (int i = 0; i < n; i++) {
            out.add(src.get(i).toString());
        }
        return out;
    }

    /**
     * ASCII-only uppercase, the {@code name_upper} derivation.
     *
     * <p>Specified as ASCII-only precisely because {@code String.toUpperCase()} is
     * locale-dependent and {@code toUpperCase(Locale.ROOT)} is still Unicode-aware
     * (it maps {@code ß} to {@code SS} and {@code ı} to {@code I}), so neither
     * matches the other arms' {@code to_ascii_uppercase}. Only {@code a-z} is
     * folded here.
     *
     * <p>Returns the input unchanged when there is nothing to fold, so the fixed
     * lowercase metric names in this corpus cost one scan and no allocation beyond
     * the result.
     */
    static String asciiUpper(String s) {
        int n = s.length();
        char[] buf = null;
        for (int i = 0; i < n; i++) {
            char c = s.charAt(i);
            if (c >= 'a' && c <= 'z') {
                if (buf == null) {
                    buf = new char[n];
                    s.getChars(0, n, buf, 0);
                }
                buf[i] = (char) (c - ('a' - 'A'));
            }
        }
        return buf == null ? s : new String(buf);
    }

    /**
     * {@code value_scaled = value * 1000 / (event_seq + 1)}, integer division
     * truncating toward zero.
     *
     * <p>Java's {@code /} on longs truncates toward zero, and {@code value} is
     * non-negative by construction, so truncation is unambiguous. {@code value}
     * is bounded by {@code 2^31 - 1}, so {@code value * 1000} cannot overflow a
     * {@code long}.
     */
    static long valueScaled(long value, int eventSeq) {
        return value * 1000L / (eventSeq + 1L);
    }
}
