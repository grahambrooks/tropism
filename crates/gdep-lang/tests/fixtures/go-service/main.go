package main

import (
	"fmt"

	"github.com/spf13/cobra"
	"example.com/shop/api"
)

func main() {
	cmd := &cobra.Command{Use: "shop"}
	fmt.Println(api.Name, cmd.Use)
}
