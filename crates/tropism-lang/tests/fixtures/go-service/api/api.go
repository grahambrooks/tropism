package api

import (
	"github.com/google/uuid"
	"github.com/rs/zerolog"

	"example.com/shop/store"
)

var Name = "api"

func New() string {
	logger := zerolog.Nop()
	logger.Info().Msg("new")
	return uuid.New().String() + store.Driver
}
