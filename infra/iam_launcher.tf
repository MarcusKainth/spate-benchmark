# The launcher role: what an APPROVED workflow job may do, and nothing else.
#
# The trust policy conditions on the `aws-bench` GitHub environment, and that
# environment requires a reviewer — so the approval gate and credential
# issuance are the same control. An unapproved job never reaches
# AssumeRoleWithWebIdentity with a `sub` this policy accepts.
#
# The permission policy is sized to "start exactly our benchmark box":
# one instance type, mandatory tags (which the reaper keys off), gp3-only
# volumes, Canonical-owned AMIs, and PassRole restricted to the one write-only
# instance role. Deliberately absent: TerminateInstances (the reaper's job,
# and the box terminates itself) and any S3 read.

resource "aws_iam_role" "launcher" {
  name = "spate-bench-launcher"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "GitHubOidcApprovedEnvironmentOnly"
        Effect = "Allow"
        Principal = {
          Federated = aws_iam_openid_connect_provider.github.arn
        }
        Action = "sts:AssumeRoleWithWebIdentity"
        Condition = {
          StringEquals = {
            "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
            "token.actions.githubusercontent.com:sub" = var.launcher_oidc_sub
          }
        }
      }
    ]
  })
}

# Canonical's AMI-publishing account. The launcher resolves the AMI id from the
# SSM public parameter at launch time; this condition means even a wrong or
# poisoned parameter value cannot make it boot somebody else's image.
locals {
  canonical_owner_id = "099720109477"
}

resource "aws_iam_role_policy" "launcher" {
  name = "launch-benchmark-box"
  role = aws_iam_role.launcher.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "RunTaggedBenchmarkInstance"
        Effect   = "Allow"
        Action   = "ec2:RunInstances"
        Resource = "arn:aws:ec2:${var.region}:${data.aws_caller_identity.current.account_id}:instance/*"
        Condition = {
          StringEquals = {
            "ec2:InstanceType"           = var.instance_type
            "aws:RequestTag/spate-bench" = "true"
          }
          Null = {
            "aws:RequestTag/ttl-hours" = "false"
          }
        }
      },
      {
        Sid      = "RunOnGp3Volumes"
        Effect   = "Allow"
        Action   = "ec2:RunInstances"
        Resource = "arn:aws:ec2:${var.region}:${data.aws_caller_identity.current.account_id}:volume/*"
        Condition = {
          StringEquals = {
            "ec2:VolumeType" = "gp3"
          }
        }
      },
      {
        Sid      = "RunFromCanonicalImages"
        Effect   = "Allow"
        Action   = "ec2:RunInstances"
        Resource = "arn:aws:ec2:${var.region}::image/*"
        Condition = {
          StringEquals = {
            "ec2:Owner" = local.canonical_owner_id
          }
        }
      },
      {
        Sid    = "RunInOurNetwork"
        Effect = "Allow"
        Action = "ec2:RunInstances"
        Resource = [
          "arn:aws:ec2:${var.region}:${data.aws_caller_identity.current.account_id}:network-interface/*",
          "arn:aws:ec2:${var.region}:${data.aws_caller_identity.current.account_id}:subnet/*",
          "arn:aws:ec2:${var.region}:${data.aws_caller_identity.current.account_id}:security-group/*",
        ]
      },
      {
        Sid      = "TagAtLaunchOnly"
        Effect   = "Allow"
        Action   = "ec2:CreateTags"
        Resource = "arn:aws:ec2:${var.region}:${data.aws_caller_identity.current.account_id}:*/*"
        Condition = {
          StringEquals = {
            "ec2:CreateAction" = "RunInstances"
          }
        }
      },
      {
        # Describe* has no resource-level scoping in EC2.
        Sid    = "Describe"
        Effect = "Allow"
        Action = [
          "ec2:DescribeInstances",
          "ec2:DescribeInstanceTypeOfferings",
          "ec2:DescribeSubnets",
          "ec2:DescribeImages",
        ]
        Resource = "*"
      },
      {
        Sid      = "PassOnlyTheInstanceRole"
        Effect   = "Allow"
        Action   = "iam:PassRole"
        Resource = aws_iam_role.instance.arn
        Condition = {
          StringEquals = {
            "iam:PassedToService" = "ec2.amazonaws.com"
          }
        }
      },
      {
        # The Ubuntu 24.04 arm64 AMI id, resolved at launch. Public parameters
        # have no account id in their ARN.
        Sid      = "ResolveCanonicalAmi"
        Effect   = "Allow"
        Action   = "ssm:GetParameter"
        Resource = "arn:aws:ssm:${var.region}::parameter/aws/service/canonical/*"
      },
      {
        # The run manifest is the collector's source of truth. The launcher may
        # write exactly that one object shape and read nothing.
        Sid      = "WriteRunManifest"
        Effect   = "Allow"
        Action   = "s3:PutObject"
        Resource = "${aws_s3_bucket.results.arn}/incoming/*/manifest.json"
      },
    ]
  })
}
