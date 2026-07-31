# The Vector arm of the cross-framework comparison.
#
# Build with the REPOSITORY ROOT as context, which every entrant does uniformly —
# the context needs `entrants/vector/` and `workload/schema/`.
#
#   bench build vector
#
# or by hand:
#
#   docker build -f entrants/vector/Dockerfile -t spate-bench-vector .
#
# README.md carries the full run recipe including the cgroup caps.

# ---------------------------------------------------------------------------
# Stage 1 — bake THE schema into the config.
#
# Vector has no Schema Registry client: its avro decoder takes a static inline
# schema. methodology/ forbids re-declaring the schema by hand, so the inline
# copy is manufactured HERE from the committed workload/schema/sensor_batch.avsc
# — it cannot drift from the schema every other arm fetches from the registry,
# because there is no second copy to drift. The substitution asserts the
# placeholder exists first: a template that silently stopped containing it would
# otherwise ship a config whose schema is the literal string
# `__AVRO_SCHEMA_JSON__`, discovered only when the container starts.
#
# json.dumps of the minified schema text produces a double-quoted, escaped
# scalar, which is valid YAML — no YAML-aware templating needed.
# ---------------------------------------------------------------------------
FROM python:3.12-slim AS gen

WORKDIR /gen
COPY workload/schema/sensor_batch.avsc entrants/vector/vector.yaml.tmpl ./

RUN python3 - <<'PY'
import json

schema = json.dumps(json.load(open("sensor_batch.avsc")), separators=(",", ":"))
tmpl = open("vector.yaml.tmpl").read()
# The full `schema:` line, not the bare token, so the template's comments may
# NAME the placeholder without being substituted themselves.
needle = "schema: __AVRO_SCHEMA_JSON__"
assert needle in tmpl, "vector.yaml.tmpl lost its schema placeholder"
open("vector.yaml", "w").write(tmpl.replace(needle, f"schema: {json.dumps(schema)}"))
PY

# ---------------------------------------------------------------------------
# Stage 2 — the runtime image.
#
# 0.57.0 pinned: the current release at activation, and the exact binary every
# published number describes — `[version].pinned` in entrant.toml refuses the
# run if the resolved version ever disagrees. (The clickhouse sink's `format`
# option — the ArrowStream / JSONEachRow axis this arm publishes — shipped
# earlier, in 0.53 via vector#24373; what 0.57 specifically brings is the
# env-var interpolation gate handled below.) The -debian variant rather than
# -distroless-static: glibc, plus a shell for the descriptor's version command.
# ---------------------------------------------------------------------------
FROM timberio/vector:0.57.0-debian

COPY --from=gen /gen/vector.yaml /etc/vector/vector.yaml
COPY entrants/vector/transform.vrl /etc/vector/transform.vrl

# Defaults target the bench network; the driver overrides what it varies. The
# knob defaults are kept EQUAL to the descriptor's published knobs, so a
# reviewer who starts the container by hand gets the configuration the numbers
# describe (rule 7 sends this image's configuration upstream).
#
# VECTOR_DANGEROUSLY_ALLOW_ENV_VAR_INTERPOLATION gates a 0.57 breaking change:
# without it, Vector leaves `${...}` in config files as literal text — the `:-`
# machinery is part of the interpolator, so the defaults are NOT applied — and
# the numeric fields then fail config deserialization, which the `vector
# validate` below turns into a build failure rather than a runtime surprise.
# The flag is what makes the knobs knobs. The name is upstream's, not a comment
# on this use: the values interpolated here are the driver's own knobs, not
# secrets.
#
# VECTOR_LOG was `warn`, to keep per-batch chatter off the hot path's stdout.
# It was measured instead: on the first live run the arrow_stream variant sat
# for its whole 1800s drain deadline at 0 of 110,250,000 rows and emitted
# ZERO log lines — the harness printed its `--- spate-bench-sut-sut ---`
# header with nothing under it. An arm that produces no evidence when it
# stalls cannot be diagnosed at all, and the sink's start-up (arrow_stream
# fetches the table schema from system.columns) and librdkafka's assignment
# lifecycle both log at INFO, so `warn` discarded precisely the two things
# worth reading.
#
# `info` is the level, not `debug`: Vector logs component lifecycle and sink
# start-up at INFO and per-request detail at DEBUG/TRACE, so the hot path
# stays quiet. If a future run shows INFO is chatty enough to matter, narrow
# it with a directive (`warn,vector::sinks=info`) rather than returning to a
# level that makes a stall silent. No metrics exporter is configured, because
# no published figure comes from an arm's self-report.
ENV VECTOR_DANGEROUSLY_ALLOW_ENV_VAR_INTERPOLATION=true \
    VECTOR_LOG=info \
    BOOTSTRAP=spate-bench-redpanda:29092 \
    CLICKHOUSE_URL=http://spate-bench-clickhouse:8123 \
    TOPIC=comparison-sensor-batches \
    GROUP_ID=comparison-vector \
    AUTO_OFFSET_RESET=earliest \
    FORMAT=arrow_stream \
    VECTOR_THREADS=6 \
    SINK_BATCH_EVENTS=262144 \
    SINK_BATCH_TIMEOUT_SECS=1 \
    SINK_REQUEST_CONCURRENCY=8 \
    SINK_BUFFER_EVENTS=524288 \
    SINK_COMPRESSION=none

# Build-time drift guard: parses the baked YAML (including the alias-expanded
# eight-source topology), compiles transform.vrl, and rejects unknown keys —
# against the exact binary that will run it. `--no-environment` skips the
# health checks, because there is no Kafka or ClickHouse at build time.
RUN vector validate --no-environment /etc/vector/vector.yaml

# Inherited from the base image: the entrypoint runs `vector`, whose default
# config path is /etc/vector/vector.yaml — the file baked above. Not restated,
# so a base-image relocation of the binary cannot silently diverge from a path
# hardcoded here.
