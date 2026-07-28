# The collector role: read the quarantine prefix, mark runs claimed, archive.
#
# The collector runs from main on a schedule — it is not behind the approval
# environment because it spends no money and writes nothing outside the bucket.
# Its GitHub-side power (opening the results PR) comes from the GitHub App
# token, which never touches AWS; its AWS-side power is listed here in full.
# Deletion is deliberately absent — the bucket lifecycle expires incoming/
# after 30 days and processed/ after 90, so cleanup needs no credential that
# could also destroy evidence of a bad run.

resource "aws_iam_role" "collector" {
  name = "spate-bench-collector"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "GitHubOidcMainBranchOnly"
        Effect = "Allow"
        Principal = {
          Federated = aws_iam_openid_connect_provider.github.arn
        }
        Action = "sts:AssumeRoleWithWebIdentity"
        Condition = {
          StringEquals = {
            "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
            "token.actions.githubusercontent.com:sub" = var.collector_oidc_sub
          }
        }
      }
    ]
  })
}

resource "aws_iam_role_policy" "collector" {
  name = "collect-results"
  role = aws_iam_role.collector.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "ListIncoming"
        Effect   = "Allow"
        Action   = "s3:ListBucket"
        Resource = aws_s3_bucket.results.arn
        Condition = {
          StringLike = {
            "s3:prefix" = "incoming/*"
          }
        }
      },
      {
        Sid      = "ReadIncoming"
        Effect   = "Allow"
        Action   = "s3:GetObject"
        Resource = "${aws_s3_bucket.results.arn}/incoming/*"
      },
      {
        Sid    = "ClaimAndArchive"
        Effect = "Allow"
        Action = "s3:PutObject"
        Resource = [
          "${aws_s3_bucket.results.arn}/incoming/*/_CLAIMED*",
          "${aws_s3_bucket.results.arn}/processed/*",
        ]
      },
    ]
  })
}
