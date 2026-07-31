#!/bin/sh
# Selects the variant's config.
#
# Each config hardcodes its own `format` and carries exactly the encoder that
# format needs, so the pairing cannot be got wrong by an environment variable:
# FORMAT chooses a file, not a field. An unknown value fails here rather than
# starting an arm that would measure something nobody asked for.
set -eu

case "${FORMAT:-arrow_stream}" in
    arrow_stream)  config=/etc/vector/vector-arrow.yaml ;;
    json_each_row) config=/etc/vector/vector-json.yaml ;;
    *)
        echo "FORMAT must be arrow_stream or json_each_row, got '${FORMAT:-}'" >&2
        exit 1
        ;;
esac

exec vector --config "$config"
