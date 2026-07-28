# The reaper: the backstop behind self-termination. Runs hourly, terminates
# any project-tagged instance older than its ttl-hours tag, and emails when it
# does — because a reap means the box's own two exit paths both failed.

data "archive_file" "reaper" {
  type        = "zip"
  source_file = "${path.module}/reaper/main.py"
  output_path = "${path.module}/reaper/build/reaper.zip"
}

resource "aws_iam_role" "reaper" {
  name = "spate-bench-reaper"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect    = "Allow"
        Principal = { Service = "lambda.amazonaws.com" }
        Action    = "sts:AssumeRole"
      }
    ]
  })
}

resource "aws_iam_role_policy" "reaper" {
  name = "reap-expired-boxes"
  role = aws_iam_role.reaper.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "Describe"
        Effect   = "Allow"
        Action   = "ec2:DescribeInstances"
        Resource = "*"
      },
      {
        # Termination is scoped by tag: this function can kill benchmark boxes
        # and nothing else in the account.
        Sid      = "TerminateProjectInstancesOnly"
        Effect   = "Allow"
        Action   = "ec2:TerminateInstances"
        Resource = "arn:aws:ec2:${var.region}:${data.aws_caller_identity.current.account_id}:instance/*"
        Condition = {
          StringEquals = {
            "aws:ResourceTag/spate-bench" = "true"
          }
        }
      },
      {
        Sid      = "Notify"
        Effect   = "Allow"
        Action   = "sns:Publish"
        Resource = aws_sns_topic.alerts.arn
      },
      {
        # Publishing to a CMK-encrypted topic needs a data key.
        Sid    = "EncryptNotifications"
        Effect = "Allow"
        Action = [
          "kms:GenerateDataKey",
          "kms:Decrypt",
        ]
        Resource = aws_kms_key.alerts.arn
      },
      {
        Sid    = "Logs"
        Effect = "Allow"
        Action = [
          "logs:CreateLogGroup",
          "logs:CreateLogStream",
          "logs:PutLogEvents",
        ]
        Resource = "arn:aws:logs:${var.region}:${data.aws_caller_identity.current.account_id}:*"
      },
    ]
  })
}

# X-Ray tracing is deliberately absent: a 40-line hourly sweep whose every
# action already lands in CloudTrail gains nothing from a tracing backend.
#trivy:ignore:AVD-AWS-0066
resource "aws_lambda_function" "reaper" {
  function_name = "spate-bench-reaper"
  role          = aws_iam_role.reaper.arn

  filename         = data.archive_file.reaper.output_path
  source_code_hash = data.archive_file.reaper.output_base64sha256

  runtime       = "python3.13"
  architectures = ["arm64"]
  handler       = "main.handler"
  timeout       = 60

  environment {
    variables = {
      SNS_TOPIC_ARN = aws_sns_topic.alerts.arn
      MAX_TTL_HOURS = tostring(var.max_ttl_hours)
    }
  }
}

resource "aws_cloudwatch_event_rule" "reaper" {
  name                = "spate-bench-reaper"
  description         = "Hourly TTL sweep over benchmark instances"
  schedule_expression = "rate(1 hour)"
}

resource "aws_cloudwatch_event_target" "reaper" {
  rule = aws_cloudwatch_event_rule.reaper.name
  arn  = aws_lambda_function.reaper.arn
}

resource "aws_lambda_permission" "reaper_from_eventbridge" {
  statement_id  = "AllowEventBridge"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.reaper.function_name
  principal     = "events.amazonaws.com"
  source_arn    = aws_cloudwatch_event_rule.reaper.arn
}

resource "aws_kms_key" "alerts" {
  description             = "spate-benchmark alert topic"
  enable_key_rotation     = true
  deletion_window_in_days = 30
}

resource "aws_kms_alias" "alerts" {
  name          = "alias/spate-bench-alerts"
  target_key_id = aws_kms_key.alerts.key_id
}

resource "aws_sns_topic" "alerts" {
  name              = "spate-bench-alerts"
  kms_master_key_id = aws_kms_key.alerts.arn
}

resource "aws_sns_topic_subscription" "alerts_email" {
  count = var.alert_email == "" ? 0 : 1

  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "email"
  endpoint  = var.alert_email
}
