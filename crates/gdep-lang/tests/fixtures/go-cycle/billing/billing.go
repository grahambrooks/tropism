package billing

import "example.com/tangle/user"

func Total() string { return user.Latest() }
