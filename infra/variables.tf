variable "region" {
  description = "Region every resource lives in. Changing it is an environment change: the instance type offering, AZ set and EBS behaviour all move with it."
  type        = string
  default     = "eu-west-2"
}

variable "github_repo" {
  description = "owner/name of the repository whose workflows may assume the roles."
  type        = string
  default     = "spate-etl/benchmark"
}

variable "launcher_oidc_sub" {
  description = <<-EOT
    Exact OIDC `sub` claim the launcher trust policy matches. The default is the
    classic format; repositories on GitHub's immutable-claims format emit
    numeric IDs (repo:owner@ID/name@ID:environment:...) instead. Decode a real
    token from a throwaway workflow before first apply (see infra/README.md) and
    override this if the observed claim differs.
  EOT
  type        = string
  default     = "repo:spate-etl/benchmark:environment:aws-bench"
}

variable "collector_oidc_sub" {
  description = "Exact OIDC `sub` claim the collector trust policy matches. Same immutable-claims caveat as launcher_oidc_sub."
  type        = string
  default     = "repo:spate-etl/benchmark:ref:refs/heads/main"
}

variable "results_bucket_name" {
  description = "Globally unique name for the results-in-flight bucket (incoming/ and processed/ prefixes)."
  type        = string
}

variable "instance_type" {
  description = "The one instance type the launcher may start. Part of the environment's provenance — see environments/c8g-8xl-ec2-docker.toml."
  type        = string
  default     = "c8g.8xlarge"
}

variable "max_ttl_hours" {
  description = "Upper bound on a benchmark box's lifetime. The reaper terminates anything older than its ttl-hours tag; this caps what that tag may usefully be."
  type        = number
  default     = 36
}

variable "budget_limit_usd" {
  description = "Monthly cost budget. Alerts at 50/80/100% of this figure."
  type        = number
  default     = 150
}

variable "alert_email" {
  description = "Address for budget alerts and reaper notifications. Empty disables the email subscriptions (the SNS topic still exists)."
  type        = string
  default     = ""
}

variable "enable_ssm_debug" {
  description = "Attach AmazonSSMManagedInstanceCore to the benchmark box for interactive debugging. Off by default: the box should be unreachable."
  type        = bool
  default     = false
}
