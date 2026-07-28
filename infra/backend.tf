# State lives in the bucket created by infra/bootstrap — private, versioned,
# KMS-encrypted, TLS-only. The bucket name is deliberately not committed:
#
#   terraform init \
#     -backend-config="bucket=<state_bucket_name>" \
#     -backend-config="region=eu-west-2"
#
# `use_lockfile` is S3-native locking (Terraform >= 1.10); no DynamoDB table.

terraform {
  backend "s3" {
    key          = "spate-benchmark/infra.tfstate"
    use_lockfile = true
  }
}
