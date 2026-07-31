package main

import (
	"fmt"

	"github.com/spf13/cobra"

	"example.com/shop/api"
	"example.com/shop/store"
)

func main() {
	cmd := &cobra.Command{Use: "shop"}
	// Reaching past the api layer straight into storage.
	fmt.Println(api.Name, store.Driver, cmd.Use)
}
