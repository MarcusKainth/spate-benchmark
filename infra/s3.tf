# The results-in-flight bucket. Nothing in it is a system of record: results
# become real when they are validated, PR'd, reviewed and merged into git.
# incoming/ is a quarantine the box writes and the collector reads; processed/
# is a short-lived archive of what the collector already turned into a PR.
# Lifecycle rules are the cleanup mechanism — no credential in the pipeline can
# delete an object.

# A dedicated key, as for the state bucket: key usage is separately visible in
# CloudTrail, and the three roles' access to run data is gated by KMS policy as
# well as bucket policy.
resource "aws_kms_key" "results" {
  description             = "spate-benchmark results in flight"
  enable_key_rotation     = true
  deletion_window_in_days = 30
}

resource "aws_kms_alias" "results" {
  name          = "alias/spate-bench-results"
  target_key_id = aws_kms_key.results.key_id
}

# Server-access logging is deliberately absent (it needs a log bucket that
# cannot itself log, and so on): every principal that can touch this bucket is
# a role whose sessions are already attributable in CloudTrail, which is the
# audit trail that matters here.
#trivy:ignore:AVD-AWS-0089
resource "aws_s3_bucket" "results" {
  bucket = var.results_bucket_name
}

resource "aws_s3_bucket_public_access_block" "results" {
  bucket = aws_s3_bucket.results.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_versioning" "results" {
  bucket = aws_s3_bucket.results.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "results" {
  bucket = aws_s3_bucket.results.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = aws_kms_key.results.arn
    }
    bucket_key_enabled = true
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "results" {
  bucket = aws_s3_bucket.results.id

  # 30 days is deliberately much longer than the collector's 30-minute cadence:
  # a broken collector has a month of slack before data is lost, and a failed
  # run's logs stay inspectable for the whole post-mortem window.
  rule {
    id     = "expire-incoming"
    status = "Enabled"

    filter {
      prefix = "incoming/"
    }

    expiration {
      days = 30
    }

    noncurrent_version_expiration {
      noncurrent_days = 7
    }
  }

  rule {
    id     = "expire-processed"
    status = "Enabled"

    filter {
      prefix = "processed/"
    }

    expiration {
      days = 90
    }

    noncurrent_version_expiration {
      noncurrent_days = 7
    }
  }

  rule {
    id     = "abort-incomplete-uploads"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

resource "aws_s3_bucket_policy" "results" {
  bucket = aws_s3_bucket.results.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "DenyInsecureTransport"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:*"
        Resource = [
          aws_s3_bucket.results.arn,
          "${aws_s3_bucket.results.arn}/*",
        ]
        Condition = {
          Bool = { "aws:SecureTransport" = "false" }
        }
      }
    ]
  })
}
