# Everything a workflow needs to reference, ready to copy into GitHub repository
# variables (Settings → Secrets and variables → Actions → Variables). The
# mapping is listed in infra/README.md. None of these values are secret — the
# trust policies are what gate their use — which is why they are plain
# variables, not secrets.

output "launcher_role_arn" {
  description = "→ repo variable AWS_LAUNCHER_ROLE_ARN"
  value       = aws_iam_role.launcher.arn
}

output "collector_role_arn" {
  description = "→ repo variable AWS_COLLECTOR_ROLE_ARN"
  value       = aws_iam_role.collector.arn
}

output "instance_profile_name" {
  description = "→ repo variable BENCH_INSTANCE_PROFILE"
  value       = aws_iam_instance_profile.instance.name
}

output "results_bucket" {
  description = "→ repo variable BENCH_BUCKET"
  value       = aws_s3_bucket.results.id
}

output "subnet_ids" {
  description = "→ repo variable BENCH_SUBNET_IDS (space-separated)"
  value       = join(" ", aws_subnet.public[*].id)
}

output "security_group_id" {
  description = "→ repo variable BENCH_SG_ID"
  value       = aws_security_group.bench.id
}

output "region" {
  description = "→ repo variable AWS_REGION"
  value       = var.region
}
