#!/bin/sh
# Renders the two properties templates from the container environment, then
# execs Connect standalone. sed on @VAR@ markers because Connect does not
# expand environment variables inside properties files — a literal ${VAR} would
# be read as those characters — and because a failed substitution leaves a
# visible @MARKER@ in the rendered file rather than a silently-empty value.
#
# set -u makes a missing variable fail the container at start-up with its name,
# instead of rendering an empty string into a config the worker then runs with.
set -eu

# The driver hands one CLICKHOUSE_URL (the same value every arm receives); the
# connector wants hostname/port/ssl split. Parsed here rather than declared as
# three variables so the descriptor's [env] stays in the driver's vocabulary.
hostport="${CLICKHOUSE_URL#http://}"
hostport="${hostport#https://}"
hostport="${hostport%%/*}"
case "$hostport" in
  *:*)
    CLICKHOUSE_HOST="${hostport%%:*}"
    CLICKHOUSE_PORT="${hostport##*:}"
    ;;
  *)
    CLICKHOUSE_HOST="$hostport"
    CLICKHOUSE_PORT=8123
    ;;
esac

# Rendered into connect-data/, the one directory the image keeps
# appuser-writable, so a reviewer can read the exact configuration in force out
# of the running container (`docker exec ... cat`), per methodology/ rule 7.
render() {
  sed \
    -e "s|@BOOTSTRAP@|${BOOTSTRAP}|g" \
    -e "s|@REGISTRY_URL@|${REGISTRY_URL}|g" \
    -e "s|@TOPIC@|${TOPIC}|g" \
    -e "s|@GROUP_ID@|${GROUP_ID}|g" \
    -e "s|@OFFSET_RESET@|${OFFSET_RESET}|g" \
    -e "s|@TASKS_MAX@|${TASKS_MAX}|g" \
    -e "s|@BUFFER_COUNT@|${BUFFER_COUNT}|g" \
    -e "s|@BUFFER_FLUSH_MS@|${BUFFER_FLUSH_MS}|g" \
    -e "s|@CLICKHOUSE_HOST@|${CLICKHOUSE_HOST}|g" \
    -e "s|@CLICKHOUSE_PORT@|${CLICKHOUSE_PORT}|g" \
    -e "s|@CLICKHOUSE_PASSWORD@|${CLICKHOUSE_PASSWORD}|g" \
    "$1" > "$2"
}

render /opt/connect/worker.properties.tmpl        /opt/kafka/connect-data/worker.properties
render /opt/connect/clickhouse-sink.properties.tmpl /opt/kafka/connect-data/clickhouse-sink.properties

# A marker that survived rendering is a template/entrypoint drift; refuse to
# start rather than run Connect on a config with a literal @VAR@ in it.
# Non-comment lines only: the templates' own comments name the mechanism.
if grep -n '^[^#]*@[A-Z_]*@' /opt/kafka/connect-data/worker.properties \
                        /opt/kafka/connect-data/clickhouse-sink.properties; then
  echo "FATAL: unrendered @VAR@ marker(s) above; template and entrypoint have drifted." >&2
  exit 1
fi

# Foreground, worker + connector in one JVM. kafka-run-class.sh appends
# KAFKA_OPTS (the GC log) and honours KAFKA_HEAP_OPTS / KAFKA_LOG4J_OPTS from
# the image ENV.
exec /opt/kafka/bin/connect-standalone.sh \
  /opt/kafka/connect-data/worker.properties \
  /opt/kafka/connect-data/clickhouse-sink.properties
