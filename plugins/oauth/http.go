package main

import (
	"net/http"

	// Generated interfaces

	"github.com/julienschmidt/httprouter"
)

// Router creates a [http.Handler] and registers the application-specific
// routes with their respective handlers for the application.
func Router() http.Handler {
	router := httprouter.New()
	// OAuth2 handlers
	// GET  /login                -> Initiates OAuth flow
	// GET  /callback             -> Handles OAuth callback
	router.GET("/", homeHandler)
	router.GET("/login", loginHandler)
	router.GET("/auth/callback", callbackHandler)
	return router
}
