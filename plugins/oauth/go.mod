module github.com/cosmonic/wash/plugins/oauth

go 1.23.0

require (
	github.com/julienschmidt/httprouter v1.3.0
	go.wasmcloud.dev/component v0.0.5
	// NOTE: Don't update this to the latest (v0.28.0 at time of writing), it's incompatible with TinyGo
	golang.org/x/oauth2 v0.24.0
)

require (
	github.com/coreos/go-semver v0.3.1 // indirect
	github.com/docker/libtrust v0.0.0-20160708172513-aabc10ec26b7 // indirect
	github.com/google/go-cmp v0.6.0 // indirect
	github.com/klauspost/compress v1.17.11 // indirect
	github.com/opencontainers/go-digest v1.0.0 // indirect
	github.com/regclient/regclient v0.7.2 // indirect
	github.com/sirupsen/logrus v1.9.3 // indirect
	github.com/ulikunitz/xz v0.5.12 // indirect
	github.com/urfave/cli/v3 v3.0.0-alpha9.2 // indirect
	go.bytecodealliance.org v0.4.0 // indirect
	golang.org/x/mod v0.21.0 // indirect
	golang.org/x/sys v0.26.0 // indirect
)

tool go.bytecodealliance.org/cmd/wit-bindgen-go
