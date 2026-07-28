# GitHub Actions federates into this account via OIDC; there is no long-lived
# AWS credential anywhere in the pipeline. A workflow job presents a token
# minted by GitHub, and the role trust policies in iam_launcher.tf and
# iam_collector.tf decide — on the token's `sub` claim — whether that job may
# assume anything.
#
# The thumbprints are the published roots for token.actions.githubusercontent.com.
# AWS has trusted GitHub's provider against its root CA since 2023 and largely
# ignores them, but the argument is required and correct values beat
# placeholders.

resource "aws_iam_openid_connect_provider" "github" {
  url            = "https://token.actions.githubusercontent.com"
  client_id_list = ["sts.amazonaws.com"]
  thumbprint_list = [
    "6938fd4d98bab03faadb97b34396831e3780aea1",
    "1c58a3a8518e8759bf075b76b750d4f2df264fcd",
  ]
}
