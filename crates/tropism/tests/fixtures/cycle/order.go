package order

import "example.com/svc/api/user"

// Owner closes the loop back to the user package.
func Owner(o *Order) *user.User {
	return user.Load(o.UserID)
}
