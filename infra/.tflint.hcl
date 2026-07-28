# tflint, run by the Infra CI job. Only the bundled terraform ruleset: no
# plugin downloads, so the lint works identically offline and on a fork.
plugin "terraform" {
  enabled = true
  preset  = "recommended"
}
