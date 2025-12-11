package main

import (
	"context"
	"net"
	"net/http"

	ctrl "sigs.k8s.io/controller-runtime"
)

// Fallback defines an interface for obtaining a fallback endpoint
type Fallback interface {
	// InvalidHostname returns the scheme and endpoint to use as a fallback when the hostname is not recognized
	InvalidHostname(hostname string) (scheme string, endpoint string)
	// NoWorkloads returns the scheme and endpoint to use when there are no workloads registered for a hostname
	NoWorkloads(hostname string) (scheme string, endpoint string)
}

var _ Fallback = (*ExternalFallback)(nil)

// ExternalFallback represents an externally provided fallback endpoint
type ExternalFallback struct {
	Scheme   string
	Endpoint string
}

func (ef *ExternalFallback) InvalidHostname(hostname string) (string, string) {
	return ef.Scheme, ef.Endpoint
}

func (ef *ExternalFallback) NoWorkloads(hostname string) (string, string) {
	return ef.Scheme, ef.Endpoint
}

var _ Fallback = (*FallbackServer)(nil)

// FallbackServer provides a simple HTTP server that responds with a 503 Service Unavailable
// when no workloads are registered to handle incoming requests.
type FallbackServer struct {
	// Address to bind the HTTP gateway to. Prefer local addresses, for ex: "127.0.0.1:0"
	BindAddr string
	// internal fallback endpoint, for when no workloads are registered
	fallbackEndpoint string
}

func (fs *FallbackServer) InvalidHostname(hostname string) (string, string) {
	return "http", fs.fallbackEndpoint
}

func (fs *FallbackServer) NoWorkloads(hostname string) (string, string) {
	return "http", fs.fallbackEndpoint
}

func (fs *FallbackServer) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	return manager.Add(fs)
}

func (fs *FallbackServer) Start(ctx context.Context) error {
	internalMux := http.NewServeMux()

	internalMux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		log := ctrl.LoggerFrom(r.Context()).WithName("fallback")
		log.Info("no backend available for request", "host", r.Host, "path", r.URL.Path)

		w.Header().Set("Connection", "close")
		http.Error(w, "No backend available", http.StatusServiceUnavailable)
	})

	listener, err := net.Listen("tcp", fs.BindAddr)
	if err != nil {
		return err
	}

	fs.fallbackEndpoint = listener.Addr().String()
	log := ctrl.LoggerFrom(ctx).WithName("fallback")
	log.Info("starting", "endpoint", fs.fallbackEndpoint)

	internalServer := &http.Server{
		Handler: internalMux,
		BaseContext: func(_ net.Listener) context.Context {
			return ctx
		},
	}
	go func() {
		<-ctx.Done()
		_ = internalServer.Close()
	}()

	return internalServer.Serve(listener)
}
