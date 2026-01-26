//go:generate go tool wit-bindgen-go generate --world wasmcloud:http-server/http-server --out gen ./wit

package main

import (
	"io"
	"net/http"
	"sort"
	"strings"

	"github.com/julienschmidt/httprouter"
	"go.wasmcloud.dev/component/log/wasilog"
	"go.wasmcloud.dev/component/net/wasihttp"
)

const Index = `/error - return a 500 error
  /form - echo the fields of a POST request
  /headers - echo your user agent back as a server side header
  /post - echo the body of a POST request`

func init() {
	router := httprouter.New()
	router.HandlerFunc(http.MethodGet, "/", indexHandler)
	router.HandlerFunc(http.MethodGet, "/error", errorHandler)
	router.HandlerFunc(http.MethodGet, "/headers", headersHandler)
	router.HandlerFunc(http.MethodPost, "/form", formHandler)
	router.HandlerFunc(http.MethodPost, "/post", postHandler)
	wasihttp.Handle(router)
}

func indexHandler(w http.ResponseWriter, r *http.Request) {
	logger := wasilog.ContextLogger("indexHandler")
	logger.Info("Request received", "host", r.Host, "agent", r.Header.Get("User-Agent"))

	handlers := []string{"/error", "/form", "/headers", "/post"}
	sort.Strings(handlers)
	w.Header().Add("content-type", "text/plain")
	w.Header().Add("X-Requested-Path", r.URL.Path)
	w.Header().Add("X-Existing-Paths", strings.Join(handlers, ","))
	_, _ = w.Write([]byte(Index))
}

func headersHandler(w http.ResponseWriter, r *http.Request) {
	w.Header().Add("X-Your-User-Agent", r.Header.Get("User-Agent"))

	_, _ = w.Write([]byte("Check headers!"))
}

func errorHandler(w http.ResponseWriter, _ *http.Request) {
	http.Error(w, "Something went wrong", http.StatusInternalServerError)
}

func formHandler(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}

	for key, values := range r.Form {
		_, _ = w.Write([]byte(key + ": " + strings.Join(values, ",") + "\n"))
	}
}

func postHandler(w http.ResponseWriter, r *http.Request) {
	logger := wasilog.ContextLogger("postHandler")

	w.WriteHeader(http.StatusOK)

	n, err := io.Copy(w, r.Body)
	if err != nil {
		logger.Error("Error copying body", "error", err)
		return
	}

	logger.Info("Copied body", "bytes", n)
}

func main() {}
