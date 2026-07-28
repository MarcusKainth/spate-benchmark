# The AWS footprint of the benchmark pipeline, in full. Nothing here is applied
# by CI: `terraform apply` is a maintainer action from a reviewed checkout, which
# is the control that makes a public infrastructure definition safe — a merged
# change to this directory does nothing until a human who read the diff applies
# it. See infra/README.md for the operating procedure and the manual checklist.

terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    archive = {
      source  = "hashicorp/archive"
      version = "~> 2.7"
    }
  }
}

provider "aws" {
  region = var.region

  # Every resource this stack creates carries the project tag; the reaper and
  # the launcher's IAM conditions key off it.
  default_tags {
    tags = {
      project = "spate-benchmark"
    }
  }
}

data "aws_caller_identity" "current" {}
