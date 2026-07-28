"""Terminate benchmark boxes that outlived their TTL.

The box's primary exit is self-termination (`shutdown -h now` with the
instance-initiated-shutdown-behavior flag set to terminate), backstopped by an
in-instance `timeout`. This Lambda is the layer behind both: it catches the
box whose user-data crashed before the trap installed, whose kernel wedged, or
whose clock never reached the shutdown. Every launch tags the instance
`spate-bench=true` and `ttl-hours=<n>`; IAM lets the launcher start instances
only WITH those tags, and lets this function terminate instances only that
CARRY the first one.

A reap is a notification-worthy event by definition: it means the two layers
in front of this one both failed. The SNS message says which instance and how
old it was.
"""

import datetime
import json
import os

import boto3

# An instance carrying the project tag but no ttl-hours should not be possible
# (IAM requires the tag at RunInstances) — treat one as already expired after a
# grace hour rather than letting it run forever.
DEFAULT_TTL_HOURS = 1.0


def handler(event, context):
    ec2 = boto3.client("ec2")
    now = datetime.datetime.now(datetime.timezone.utc)
    reaped = []

    pages = ec2.get_paginator("describe_instances").paginate(
        Filters=[
            {"Name": "tag:spate-bench", "Values": ["true"]},
            {"Name": "instance-state-name", "Values": ["pending", "running", "stopping", "stopped"]},
        ]
    )
    for page in pages:
        for reservation in page["Reservations"]:
            for instance in reservation["Instances"]:
                tags = {t["Key"]: t["Value"] for t in instance.get("Tags", [])}
                try:
                    ttl_hours = float(tags["ttl-hours"])
                except (KeyError, ValueError):
                    ttl_hours = DEFAULT_TTL_HOURS
                age = now - instance["LaunchTime"]
                age_hours = age.total_seconds() / 3600.0
                if age_hours > ttl_hours:
                    reaped.append(
                        {
                            "instance": instance["InstanceId"],
                            "run_id": tags.get("run-id", "unknown"),
                            "age_hours": round(age_hours, 1),
                            "ttl_hours": ttl_hours,
                        }
                    )

    if reaped:
        ec2.terminate_instances(InstanceIds=[r["instance"] for r in reaped])
        topic = os.environ.get("SNS_TOPIC_ARN")
        if topic:
            boto3.client("sns").publish(
                TopicArn=topic,
                Subject="spate-benchmark reaper terminated %d instance(s)" % len(reaped),
                Message=(
                    "Self-termination failed and the reaper stepped in. Look at the\n"
                    "run's logs under incoming/<run-id>/logs/ in the results bucket.\n\n"
                    + json.dumps(reaped, indent=2)
                ),
            )

    return {"reaped": reaped}
