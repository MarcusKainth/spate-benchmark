# AWS infrastructure

The complete AWS footprint of the benchmark pipeline, as Terraform. Public on
purpose: every permission the pipeline holds is reviewable here, and nothing
in this directory is a secret — security comes from OIDC trust conditions,
least-privilege policies and layered spend controls, not from hiding the
shape of the account.

**CI never applies this.** The `Infra` CI job formats, validates, lints and
scans; `terraform apply` is a maintainer action from a reviewed checkout. A
merged change to this directory does nothing until a human who read the diff
applies it.

## How a run works

```
workflow_dispatch / push to main
        │
   [plan job]      prints the exact arm list (bench run --dry-run)
        │
   [aws-bench environment]   ← required reviewer approves; the IAM trust
        │                      policy conditions on this environment, so an
        │                      unapproved job cannot obtain AWS credentials
   [launch job]    OIDC → spate-bench-launcher → ec2 run-instances
        │
   [EC2 c8g.8xlarge]   clones the repo at the approved SHA, builds the
        │              harness, runs the suite, uploads appended result
        │              lines + logs to s3://…/incoming/<run_id>/,
        │              then shuts down → terminates
        │
   [collector workflow, every 30 min]   OIDC → spate-bench-collector →
        │              downloads, re-validates with `bench validate`,
        │              opens a results PR with the GitHub App token
        ▼
   ordinary PR review; required checks; merge; site deploy
```

Cost containment is layered, each layer catching the failure of the one
before it: in-instance `timeout` → self-shutdown with
instance-initiated-shutdown-behavior=terminate and DeleteOnTermination EBS →
hourly reaper Lambda keyed on the `ttl-hours` tag (emails when it fires) →
IAM conditions restricting launches to one instance type with mandatory tags →
an on-demand vCPU service quota of 32 (exactly one box) → AWS Budgets alerts
at 50/80/100%.

## One-time setup

1. **State bucket** — `cd bootstrap && terraform init && terraform apply
   -var state_bucket_name=<unique>`. Local state, gitignored; see the header
   of `bootstrap/main.tf`.
2. **Main stack** — `terraform init -backend-config="bucket=<unique>"
   -backend-config="region=eu-west-2"`, then `terraform apply
   -var results_bucket_name=<unique> -var alert_email=<you>`.
3. **Service quota** — in Service Quotas → EC2, set *Running On-Demand
   Standard instances* to **32 vCPUs** in the region. This is a deliberate
   ceiling: one c8g.8xlarge, no concurrent second box, and a hard circuit
   breaker no workflow bug can exceed.
4. **Region offering check** — confirm the instance type exists here:
   `aws ec2 describe-instance-type-offerings --location-type region
   --filters Name=instance-type,Values=c8g.8xlarge --region eu-west-2`.
   If absent, re-apply with `-var region=eu-west-1` and update the repo
   variables to match.
5. **OIDC `sub` format check** — before trusting the default
   `launcher_oidc_sub`/`collector_oidc_sub` values, decode a real token:
   run a throwaway workflow step with `actions/github-script` calling
   `core.getIDToken("sts.amazonaws.com")`, split the JWT on `.`, and
   base64-decode the payload. Repositories on GitHub's immutable-claims
   format emit `repo:owner@<id>/name@<id>:…` instead of `repo:owner/name:…`;
   if that is what you see, override both variables with the observed
   values.
6. **GitHub App** — reuse the spate-etl release App if it can be installed
   on this repository; otherwise create `spate-benchmark-bot`. Permissions:
   Contents read/write, Pull requests read/write, Issues read/write —
   repository-scoped to this repo only. Install it, note the App ID, and
   generate a private key.
7. **GitHub environment `aws-bench`** — Settings → Environments:
   - Required reviewers: the maintainer. Leave **prevent self-review OFF**:
     with a single maintainer it would deadlock every push-triggered run.
   - Deployment branches and tags: `main` and `bootstrap/*` (the ceilings
     bootstrap flow dispatches from a `bootstrap/…` branch).
8. **Repository variables** (Settings → Actions → Variables), from
   `terraform output`:

   | Variable | Source |
   |---|---|
   | `AWS_REGION` | `region` output |
   | `AWS_LAUNCHER_ROLE_ARN` | `launcher_role_arn` |
   | `AWS_COLLECTOR_ROLE_ARN` | `collector_role_arn` |
   | `BENCH_BUCKET` | `results_bucket` |
   | `BENCH_SUBNET_IDS` | `subnet_ids` |
   | `BENCH_SG_ID` | `security_group_id` |
   | `BENCH_INSTANCE_PROFILE` | `instance_profile_name` |
   | `BENCH_APP_ID` | the GitHub App id |

   And one **secret**: `BENCH_APP_PRIVATE_KEY` — the App's PEM.
9. **SNS confirmation** — the `alert_email` subscription sends a
   confirmation mail; click it or reaper notifications go nowhere.

## Day-2 operations

- **Apply**: always from a clean checkout of `main`, always `terraform plan`
  first. There is no automation to race against.
- **Debugging a box**: logs land in `s3://<bucket>/incoming/<run_id>/logs/`
  even when the run fails (the user-data trap uploads them on any exit). For
  interactive access re-apply with `-var enable_ssm_debug=true` and use SSM
  Session Manager; turn it back off afterwards.
- **A reaper email** means self-termination failed — read the run's logs and
  open an issue on the run before touching anything.
- **Rotating the App key**: generate a new key in the App settings, update
  `BENCH_APP_PRIVATE_KEY`, delete the old key. Nothing in AWS changes.

<!-- BEGIN_TF_DOCS -->
## Requirements

| Name | Version |
|------|---------|
| <a name="requirement_terraform"></a> [terraform](#requirement\_terraform) | >= 1.10 |
| <a name="requirement_archive"></a> [archive](#requirement\_archive) | ~> 2.7 |
| <a name="requirement_aws"></a> [aws](#requirement\_aws) | ~> 6.0 |

## Providers

| Name | Version |
|------|---------|
| <a name="provider_archive"></a> [archive](#provider\_archive) | ~> 2.7 |
| <a name="provider_aws"></a> [aws](#provider\_aws) | ~> 6.0 |

## Modules

No modules.

## Resources

| Name | Type |
|------|------|
| [aws_budgets_budget.monthly](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/budgets_budget) | resource |
| [aws_cloudwatch_event_rule.reaper](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/cloudwatch_event_rule) | resource |
| [aws_cloudwatch_event_target.reaper](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/cloudwatch_event_target) | resource |
| [aws_iam_instance_profile.instance](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_instance_profile) | resource |
| [aws_iam_openid_connect_provider.github](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_openid_connect_provider) | resource |
| [aws_iam_role.collector](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role) | resource |
| [aws_iam_role.instance](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role) | resource |
| [aws_iam_role.launcher](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role) | resource |
| [aws_iam_role.reaper](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role) | resource |
| [aws_iam_role_policy.collector](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role_policy) | resource |
| [aws_iam_role_policy.instance](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role_policy) | resource |
| [aws_iam_role_policy.launcher](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role_policy) | resource |
| [aws_iam_role_policy.reaper](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role_policy) | resource |
| [aws_iam_role_policy_attachment.instance_ssm_debug](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/iam_role_policy_attachment) | resource |
| [aws_internet_gateway.bench](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/internet_gateway) | resource |
| [aws_kms_alias.alerts](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/kms_alias) | resource |
| [aws_kms_alias.results](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/kms_alias) | resource |
| [aws_kms_key.alerts](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/kms_key) | resource |
| [aws_kms_key.results](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/kms_key) | resource |
| [aws_lambda_function.reaper](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/lambda_function) | resource |
| [aws_lambda_permission.reaper_from_eventbridge](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/lambda_permission) | resource |
| [aws_route_table.public](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/route_table) | resource |
| [aws_route_table_association.public](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/route_table_association) | resource |
| [aws_s3_bucket.results](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/s3_bucket) | resource |
| [aws_s3_bucket_lifecycle_configuration.results](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/s3_bucket_lifecycle_configuration) | resource |
| [aws_s3_bucket_policy.results](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/s3_bucket_policy) | resource |
| [aws_s3_bucket_public_access_block.results](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/s3_bucket_public_access_block) | resource |
| [aws_s3_bucket_server_side_encryption_configuration.results](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/s3_bucket_server_side_encryption_configuration) | resource |
| [aws_s3_bucket_versioning.results](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/s3_bucket_versioning) | resource |
| [aws_security_group.bench](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/security_group) | resource |
| [aws_sns_topic.alerts](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/sns_topic) | resource |
| [aws_sns_topic_subscription.alerts_email](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/sns_topic_subscription) | resource |
| [aws_subnet.public](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/subnet) | resource |
| [aws_vpc.bench](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/vpc) | resource |
| [archive_file.reaper](https://registry.terraform.io/providers/hashicorp/archive/latest/docs/data-sources/file) | data source |
| [aws_availability_zones.available](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/data-sources/availability_zones) | data source |
| [aws_caller_identity.current](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/data-sources/caller_identity) | data source |

## Inputs

| Name | Description | Type | Default | Required |
|------|-------------|------|---------|:--------:|
| <a name="input_alert_email"></a> [alert\_email](#input\_alert\_email) | Address for budget alerts and reaper notifications. Empty disables the email subscriptions (the SNS topic still exists). | `string` | `""` | no |
| <a name="input_budget_limit_usd"></a> [budget\_limit\_usd](#input\_budget\_limit\_usd) | Monthly cost budget. Alerts at 50/80/100% of this figure. | `number` | `150` | no |
| <a name="input_collector_oidc_sub"></a> [collector\_oidc\_sub](#input\_collector\_oidc\_sub) | Exact OIDC `sub` claim the collector trust policy matches. Same immutable-claims caveat as launcher\_oidc\_sub. | `string` | `"repo:spate-etl/benchmark:ref:refs/heads/main"` | no |
| <a name="input_enable_ssm_debug"></a> [enable\_ssm\_debug](#input\_enable\_ssm\_debug) | Attach AmazonSSMManagedInstanceCore to the benchmark box for interactive debugging. Off by default: the box should be unreachable. | `bool` | `false` | no |
| <a name="input_instance_type"></a> [instance\_type](#input\_instance\_type) | The one instance type the launcher may start. Part of the environment's provenance — see environments/c8g-8xl-ec2-docker.toml. | `string` | `"c8g.8xlarge"` | no |
| <a name="input_launcher_oidc_sub"></a> [launcher\_oidc\_sub](#input\_launcher\_oidc\_sub) | Exact OIDC `sub` claim the launcher trust policy matches. The default is the<br/>classic format; repositories on GitHub's immutable-claims format emit<br/>numeric IDs (repo:owner@ID/name@ID:environment:...) instead. Decode a real<br/>token from a throwaway workflow before first apply (see infra/README.md) and<br/>override this if the observed claim differs. | `string` | `"repo:spate-etl/benchmark:environment:aws-bench"` | no |
| <a name="input_max_ttl_hours"></a> [max\_ttl\_hours](#input\_max\_ttl\_hours) | Upper bound on a benchmark box's lifetime. The reaper clamps every instance's ttl-hours tag to this, so a fat-fingered or forged tag cannot buy more than this many hours. | `number` | `36` | no |
| <a name="input_region"></a> [region](#input\_region) | Region every resource lives in. Changing it is an environment change: the instance type offering, AZ set and EBS behaviour all move with it. | `string` | `"eu-west-2"` | no |
| <a name="input_results_bucket_name"></a> [results\_bucket\_name](#input\_results\_bucket\_name) | Globally unique name for the results-in-flight bucket (incoming/ and processed/ prefixes). | `string` | n/a | yes |

## Outputs

| Name | Description |
|------|-------------|
| <a name="output_collector_role_arn"></a> [collector\_role\_arn](#output\_collector\_role\_arn) | → repo variable AWS\_COLLECTOR\_ROLE\_ARN |
| <a name="output_instance_profile_name"></a> [instance\_profile\_name](#output\_instance\_profile\_name) | → repo variable BENCH\_INSTANCE\_PROFILE |
| <a name="output_launcher_role_arn"></a> [launcher\_role\_arn](#output\_launcher\_role\_arn) | → repo variable AWS\_LAUNCHER\_ROLE\_ARN |
| <a name="output_region"></a> [region](#output\_region) | → repo variable AWS\_REGION |
| <a name="output_results_bucket"></a> [results\_bucket](#output\_results\_bucket) | → repo variable BENCH\_BUCKET |
| <a name="output_security_group_id"></a> [security\_group\_id](#output\_security\_group\_id) | → repo variable BENCH\_SG\_ID |
| <a name="output_subnet_ids"></a> [subnet\_ids](#output\_subnet\_ids) | → repo variable BENCH\_SUBNET\_IDS (space-separated) |
<!-- END_TF_DOCS -->
