package api

import (
	"github.com/google/uuid"
	"github.com/rs/zerolog"

	"example.com/shop/store"
)

var Name = "api"

func New() string {
	zerolog.Nop().Info().Msg("new")
	return uuid.New().String() + store.Driver
}
