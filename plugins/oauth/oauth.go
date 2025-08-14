package main

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"

	"github.com/wasmcloud/wash/plugins/oauth/gen/wasi/filesystem/v0.2.0/preopens"
	fsTypes "github.com/wasmcloud/wash/plugins/oauth/gen/wasi/filesystem/v0.2.0/types"
	"github.com/wasmcloud/wash/plugins/oauth/gen/wasi/logging/v0.1.0-draft/logging"
	"github.com/julienschmidt/httprouter"
	"go.bytecodealliance.org/cm"
	"go.wasmcloud.dev/component/gen/wasi/config/runtime"
	"golang.org/x/oauth2"
)

var dexEndpoint = oauth2.Endpoint{
	// AuthURL:  "http://localhost:8080/oidc/auth",
	// TokenURL: "http://localhost:8080/oidc/token",

	// github test
	AuthURL:  "https://github.com/login/oauth/authorize",
	TokenURL: "https://github.com/login/oauth/access_token",
}

func newOAuth2Config() *oauth2.Config {
	clientId, err, isErr := runtime.Get(clientIdConfig).Result()
	if isErr || clientId.None() {
		logging.Log(logging.LevelError, "oauth", "failed to get client_id from config: "+err.String())
		return nil
	}
	clientSecret, err, isErr := runtime.Get(clientSecretConfig).Result()
	if isErr || clientSecret.None() {
		logging.Log(logging.LevelError, "oauth", "failed to get client_secret from config: "+err.String())
		return nil
	}
	// TODO: Get port redirect from wash?
	return &oauth2.Config{
		ClientID:     *clientId.Some(),
		ClientSecret: *clientSecret.Some(),
		RedirectURL:  "http://localhost:8888/auth/callback",
		Scopes:       []string{"openid", "email", "profile", "groups", "offline_access"},
		Endpoint:     dexEndpoint,
	}
}

// loginHandler initiates the OAuth flow by redirecting the user to Dex
func loginHandler(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	config := newOAuth2Config()
	url := config.AuthCodeURL("state", oauth2.AccessTypeOffline)
	http.Redirect(w, r, url, http.StatusTemporaryRedirect)
}

// callbackHandler exchanges the code for tokens and stores user info
func callbackHandler(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	config := newOAuth2Config()

	code := r.URL.Query().Get("code")
	if code == "" {
		http.Error(w, "missing code in callback", http.StatusBadRequest)
		return
	}

	// exchange code for token
	ctx := context.WithValue(r.Context(), oauth2.HTTPClient, httpClient)
	token, err := config.Exchange(ctx, code)
	if err != nil {
		http.Error(w, "token exchange failed: "+err.Error(), http.StatusInternalServerError)
		return
	}

	// extract id_token for future verification
	rawIDToken, ok := token.Extra("id_token").(string)
	if !ok {
		http.Error(w, "missing id_token in response", http.StatusInternalServerError)
		return
	}

	// base64-encode and set in a cookie
	tokenInfo := map[string]string{
		"access_token": token.AccessToken,
		"id_token":     rawIDToken,
	}
	userJSON, _ := json.Marshal(tokenInfo)
	http.SetCookie(w, &http.Cookie{
		Name:  "user",
		Value: base64.RawStdEncoding.EncodeToString(userJSON),
		Path:  "/",
	})

	dirs := preopens.GetDirectories().Slice()
	if len(dirs) == 0 {
		logging.Log(logging.LevelError, "oauth_http", "No preopens found for filesystem access, can't validate authentication")
	}
	if len(dirs) > 1 {
		logging.Log(logging.LevelWarn, "oauth_http", "Multiple preopens found, using the first one for authentication validation")
	}

	dir := dirs[0].F0

	res, fsErr, isErr := dir.OpenAt(0, config.ClientID+".creds", fsTypes.OpenFlagsCreate|fsTypes.OpenFlagsTruncate, fsTypes.DescriptorFlagsWrite).Result()
	if isErr {
		errMsg := "Failed to open creds file: " + fsErr.String()
		logging.Log(logging.LevelError, "oauth_http", errMsg)
		http.Error(w, errMsg, http.StatusInternalServerError)
		return
	}
	writtenBytes, fsErr, isErr := res.Write(cm.ToList(userJSON), 0).Result()
	if isErr {
		errMsg := "Failed to write creds file: " + fsErr.String()
		logging.Log(logging.LevelError, "oauth_http", errMsg)
		http.Error(w, errMsg, http.StatusInternalServerError)
		return
	}
	if int(writtenBytes) != len(userJSON) {
		logging.Log(logging.LevelError, "oauth_http", "Failed to write all bytes to creds file")
		http.Error(w, "failed to write all bytes to creds file", http.StatusInternalServerError)
		return
	}

	http.Redirect(w, r, "/", http.StatusFound)
}

// homeHandler reads the user JSON from the creds file and displays it
func homeHandler(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	config := newOAuth2Config()
	if config == nil {
		http.Error(w, "OAuth config not available", http.StatusInternalServerError)
		return
	}

	dirs := preopens.GetDirectories().Slice()
	if len(dirs) == 0 {
		logging.Log(logging.LevelError, "oauth_http", "No preopens found for filesystem access")
		http.Error(w, "No filesystem access", http.StatusInternalServerError)
		return
	}
	dir := dirs[0].F0

	res, fsErr, isErr := dir.OpenAt(0, config.ClientID+".creds", 0, fsTypes.DescriptorFlagsRead).Result()
	if isErr {
		logging.Log(logging.LevelWarn, "oauth_http", "No creds file found: "+fsErr.String())
		http.Error(w, "Not authenticated. Please log in.", http.StatusUnauthorized)
		return
	}

	// Read the file contents
	data, fsErr, isErr := res.Read(4096, 0).Result()
	if isErr {
		logging.Log(logging.LevelError, "oauth_http", "Failed to read creds file: "+fsErr.String())
		http.Error(w, "Failed to read credentials", http.StatusInternalServerError)
		return
	}

	userJSON := data.F0.Slice()
	var tokenInfo map[string]string
	if err := json.Unmarshal(userJSON, &tokenInfo); err != nil {
		logging.Log(logging.LevelError, "oauth_http", "Failed to parse user JSON: "+err.Error())
		http.Error(w, "Invalid user data", http.StatusInternalServerError)
		return
	}

	// Display user info (for demo, just dump JSON)
	w.Header().Set("Content-Type", "application/json")
	w.Write(userJSON)
}
