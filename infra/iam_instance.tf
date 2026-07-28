# The benchmark box's identity: write-only, one prefix, one bucket.
#
# The box builds and runs code from the repository at an approved SHA — but it
# also runs entrant containers, which are third-party systems under test. Its
# credentials are therefore scoped so that even full compromise of the box
# yields nothing but the ability to PUT objects into a quarantine prefix that
# `bench validate` firewalls before anything reaches git. No read, no list, no
# other service.
#
# Defence in depth on the credential path itself: the launcher starts the box
# with IMDSv2 required and a hop limit of 1, so a process inside a container
# (one network hop further) cannot fetch the instance credentials at all.

resource "aws_iam_role" "instance" {
  name = "spate-bench-instance"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect    = "Allow"
        Principal = { Service = "ec2.amazonaws.com" }
        Action    = "sts:AssumeRole"
      }
    ]
  })
}

resource "aws_iam_role_policy" "instance" {
  name = "upload-results-only"
  role = aws_iam_role.instance.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "UploadIntoQuarantine"
        Effect   = "Allow"
        Action   = "s3:PutObject"
        Resource = "${aws_s3_bucket.results.arn}/incoming/*"
      }
    ]
  })
}

# Interactive debugging is off unless deliberately enabled: the box is supposed
# to be unreachable, and its logs land in S3 either way.
resource "aws_iam_role_policy_attachment" "instance_ssm_debug" {
  count = var.enable_ssm_debug ? 1 : 0

  role       = aws_iam_role.instance.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_instance_profile" "instance" {
  name = "spate-bench-instance"
  role = aws_iam_role.instance.name
}
