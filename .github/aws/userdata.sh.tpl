#!/bin/bash
# User-data for the benchmark box. Rendered by bench-launch.yml with envsubst,
# restricted to the ${...} names listed there — every other dollar sign in this
# file is ordinary shell and survives rendering untouched.
#
# This template is deliberately a stub: it installs just enough to report home,
# clones the repository at the APPROVED SHA, and hands over to
# .github/aws/run-bench.sh from that checkout — so the logic that matters is
# versioned and reviewed like any other change, and what runs is what the
# approver saw. The box holds no GitHub credential (the repository is public)
# and its only AWS power is PutObject into incoming/ on one bucket.
#
# The trap is the box's promise: whatever happens — clone failure, build
# failure, timeout, refused run — the logs land in S3 and the machine shuts
# down. Shutdown terminates (the launcher sets
# instance-initiated-shutdown-behavior=terminate) and termination deletes the
# volume (DeleteOnTermination), so the steady state is always "nothing running,
# nothing billed".
set -euo pipefail
exec > >(tee -a /var/log/bench-userdata.log) 2>&1

export RUN_ID='${RUN_ID}'
export SHA='${SHA}'
export ENV_ID='${ENV_ID}'
export SELECTOR='${SELECTOR}'
export REPS='${REPS}'
export TRIGGER='${TRIGGER}'
export MODE='${MODE}'
export BUCKET='${BUCKET}'
export TTL_HOURS='${TTL_HOURS}'
export AWS_DEFAULT_REGION='${AWS_REGION}'

finish() {
  status=$?
  set +e
  if [ ! -f /run/bench-complete ]; then
    printf '{"run_id":"%s","status":"failed","exit_code":%d}\n' "$RUN_ID" "$status" \
      > /tmp/_FAILED.json
    aws s3 cp /tmp/_FAILED.json "s3://$BUCKET/incoming/$RUN_ID/_FAILED.json"
  fi
  aws s3 cp /var/log/bench-userdata.log "s3://$BUCKET/incoming/$RUN_ID/logs/userdata.log"
  if [ -f /var/log/cloud-init-output.log ]; then
    aws s3 cp /var/log/cloud-init-output.log "s3://$BUCKET/incoming/$RUN_ID/logs/cloud-init-output.log"
  fi
  shutdown -h now
}
trap finish EXIT

apt-get update
apt-get install -y git
snap install aws-cli --classic
export PATH="$PATH:/snap/bin"

git clone https://github.com/spate-etl/benchmark /opt/bench
cd /opt/bench
git checkout --detach "$SHA"

bash .github/aws/run-bench.sh
