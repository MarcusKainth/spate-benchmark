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
#
# http:// only, loudly: the rendered config says ssl=false and the no-port
# default is 8123, so accepting https:// here would silently speak plaintext
# to a TLS endpoint and surface as a drain timeout minutes later. Bracketed
# IPv6 literals would mis-split on ':' — refused rather than mangled.
case "$CLICKHOUSE_URL" in
  http://*) ;;
  *)
    echo "FATAL: CLICKHOUSE_URL must be http:// — this arm renders ssl=false" \
         "and a plaintext port default. Got scheme: ${CLICKHOUSE_URL%%://*}" >&2
    exit 1
    ;;
esac
hostport="${CLICKHOUSE_URL#http://}"
hostport="${hostport%%/*}"
case "$hostport" in
  \[*)
    echo "FATAL: bracketed IPv6 literals are not supported by this host/port split." >&2
    exit 1
    ;;
  *:*)
    CLICKHOUSE_HOST="${hostport%%:*}"
    CLICKHOUSE_PORT="${hostport##*:}"
    ;;
  *)
    CLICKHOUSE_HOST="$hostport"
    CLICKHOUSE_PORT=8123
    ;;
esac

# The render step is sed, and sed's replacement text gives '|' (the delimiter),
# '&' (the whole match), '\' and newlines meanings a config value must not
# have. Every value the driver supplies today is inert (hostnames, integers, a
# hex group id); this guard is what turns the first one that is not into a
# named refusal instead of a silently corrupted config. The variable NAME is
# printed, never the value — one of these is a password.
nl='
'
for name in BOOTSTRAP REGISTRY_URL TOPIC GROUP_ID OFFSET_RESET TASKS_MAX \
            BUFFER_COUNT BUFFER_FLUSH_MS CLICKHOUSE_HOST CLICKHOUSE_PORT \
            CLICKHOUSE_PASSWORD; do
  eval "v=\${${name}}"
  case "$v" in
    *\|* | *\&* | *\\* | *"$nl"*)
      echo "FATAL: ${name} contains a character the render step cannot" \
           "substitute safely (one of: | & \\ newline)." >&2
      exit 1
      ;;
  esac
done

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
# The marker shape requires a leading letter ([A-Z_][A-Z0-9_]*): '@@' in a
# value is not a marker, and a future digit-bearing marker still matches.
if grep -n '^[^#]*@[A-Z_][A-Z0-9_]*@' /opt/kafka/connect-data/worker.properties \
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
