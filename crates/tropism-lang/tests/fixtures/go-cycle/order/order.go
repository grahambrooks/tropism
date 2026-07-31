package order

import "example.com/tangle/billing"

func Newest() string { return billing.Total() }
