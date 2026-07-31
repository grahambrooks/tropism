package api

import "example.com/svc/api/order"

// Load returns the order most recently placed by a user.
func Load(id string) *order.Order {
	return order.Find(id)
}
