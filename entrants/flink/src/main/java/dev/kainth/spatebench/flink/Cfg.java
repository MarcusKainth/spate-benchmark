package dev.kainth.spatebench.flink;

import java.util.Locale;

/**
 * Environment lookups, with the defaults that target the live bench network.
 *
 * <p>Nothing here is measurement instrumentation. Per {@code METHODOLOGY.md}
 * every published figure comes from the driver's cgroup sampler and from
 * ClickHouse; this class only reads configuration.
 */
final class Cfg {

    private Cfg() {}

    static String str(String key, String fallback) {
        String v = System.getenv(key);
        return (v == null || v.isEmpty()) ? fallback : v;
    }

    static String lower(String key, String fallback) {
        return str(key, fallback).toLowerCase(Locale.ROOT);
    }

    static int i(String key, int fallback) {
        String v = System.getenv(key);
        return (v == null || v.isEmpty()) ? fallback : Integer.parseInt(v.trim());
    }

    static long l(String key, long fallback) {
        String v = System.getenv(key);
        return (v == null || v.isEmpty()) ? fallback : Long.parseLong(v.trim());
    }

    static String oneOf(String key, String fallback, String... allowed) {
        String v = lower(key, fallback);
        for (String a : allowed) {
            if (a.equals(v)) {
                return v;
            }
        }
        throw new IllegalArgumentException(
                key + "=" + v + " is not one of " + String.join("|", allowed));
    }
}
