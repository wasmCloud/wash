
# Disable go.work from repository toplevel as this example is not part of the monorepo
GOWORK = off
export GOWORK

all: build

.PHONY: build
build:
	tinygo build -target wasip2 -wit-package ./wit -wit-world http-server -o go-hello-world.wasm ./

.PHONY: bindgen
bindgen:
	go generate ./...
