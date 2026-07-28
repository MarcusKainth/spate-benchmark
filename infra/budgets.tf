# The static spend ceiling. The layered controls in front of this (TTL +
# self-termination, the reaper, the 32-vCPU service quota) should make these
# alerts never fire; if one does, something structural failed and the email is
# the point.

resource "aws_budgets_budget" "monthly" {
  name        = "spate-benchmark-monthly"
  budget_type = "COST"
  time_unit   = "MONTHLY"

  limit_amount = tostring(var.budget_limit_usd)
  limit_unit   = "USD"

  dynamic "notification" {
    for_each = var.alert_email == "" ? [] : [50, 80, 100]

    content {
      comparison_operator        = "GREATER_THAN"
      threshold                  = notification.value
      threshold_type             = "PERCENTAGE"
      notification_type          = "ACTUAL"
      subscriber_email_addresses = [var.alert_email]
    }
  }
}
