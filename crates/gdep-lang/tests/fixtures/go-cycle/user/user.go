package user

import "example.com/tangle/order"

func Latest() string { return order.Newest() }
