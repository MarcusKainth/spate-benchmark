# One-time bootstrap: the bucket the main stack keeps its state in.
#
# Terraform state is the one artefact of this repository that is genuinely
# sensitive — it records every resource attribute, including ones the HCL never
# shows. It therefore lives in a private, versioned, KMS-encrypted bucket that
# is created here, once, with LOCAL state. The local state file for this tiny
# stack describes one bucket and one key, is gitignored, and losing it costs
# nothing: both resources are trivially importable.
#
#   cd infra/bootstrap
#   terraform init
#   terraform apply -var region=eu-west-2 -var state_bucket_name=<globally-unique-name>
#
# Then configure the main stack against it:
#
#   cd ..
#   terraform init \
#     -backend-config="bucket=<state_bucket_name>" \
#     -backend-config="region=eu-west-2"

terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

provider "aws" {
  region = var.region

  default_tags {
    tags = {
      project = "spate-benchmark"
    }
  }
}

variable "region" {
  description = "Region for the state bucket. Must match the main stack's region."
  type        = string
  default     = "eu-west-2"
}

variable "state_bucket_name" {
  description = "Globally unique name for the Terraform state bucket."
  type        = string
}

# A dedicated key rather than the account's default aws/s3 key: access to state
# is then gated by KMS key policy as well as bucket policy, and key usage is
# separately visible in CloudTrail.
resource "aws_kms_key" "state" {
  description             = "spate-benchmark Terraform state"
  enable_key_rotation     = true
  deletion_window_in_days = 30
}

resource "aws_kms_alias" "state" {
  name          = "alias/spate-benchmark-tfstate"
  target_key_id = aws_kms_key.state.key_id
}

# Server-access logging is deliberately absent (it needs a log bucket that
# cannot itself log): the only principal that touches state is the maintainer
# running terraform, and both S3 data events and KMS key usage are already
# attributable in CloudTrail.
#trivy:ignore:AVD-AWS-0089
resource "aws_s3_bucket" "state" {
  bucket = var.state_bucket_name
}

resource "aws_s3_bucket_public_access_block" "state" {
  bucket = aws_s3_bucket.state.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# Versioning is what makes `use_lockfile` state locking safe and every previous
# state recoverable after a bad apply.
resource "aws_s3_bucket_versioning" "state" {
  bucket = aws_s3_bucket.state.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "state" {
  bucket = aws_s3_bucket.state.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = aws_kms_key.state.arn
    }
    bucket_key_enabled = true
  }
}

# Old state versions are kept for 90 days — long enough to recover from a bad
# apply discovered late, short enough that state history does not accumulate
# forever.
resource "aws_s3_bucket_lifecycle_configuration" "state" {
  bucket = aws_s3_bucket.state.id

  rule {
    id     = "expire-noncurrent"
    status = "Enabled"

    filter {}

    noncurrent_version_expiration {
      noncurrent_days = 90
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

# TLS-only, unconditionally. There is no plaintext-HTTP reader of state.
resource "aws_s3_bucket_policy" "state" {
  bucket = aws_s3_bucket.state.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "DenyInsecureTransport"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:*"
        Resource = [
          aws_s3_bucket.state.arn,
          "${aws_s3_bucket.state.arn}/*",
        ]
        Condition = {
          Bool = { "aws:SecureTransport" = "false" }
        }
      }
    ]
  })
}

output "state_bucket" {
  value = aws_s3_bucket.state.id
}

output "state_kms_key_arn" {
  value = aws_kms_key.state.arn
}
