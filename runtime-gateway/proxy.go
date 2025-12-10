package main

import (
	"context"
	"net"
	"net/http"
	"net/http/httputil"
	"time"

	ctrl "sigs.k8s.io/controller-runtime"
)

type HTTPGateway struct {
	BindAddr string
	Proxy    *httputil.ReverseProxy
	Resolver HostResolver
}

func (h *HTTPGateway) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	h.Proxy.ModifyResponse = h.modifyResponse
	h.Proxy.Rewrite = h.rewrite
	h.Proxy.ErrorHandler = h.proxyErrorHandler
	return manager.Add(h)
}

func (h *HTTPGateway) Start(ctx context.Context) error {
	log := ctrl.LoggerFrom(ctx).WithName("http-gateway")

	publicFacing := &http.Server{
		BaseContext: func(_ net.Listener) context.Context {
			return ctx
		},
		Addr:              h.BindAddr,
		Handler:           h.Proxy,
		IdleTimeout:       120 * time.Second,
		ReadTimeout:       0,
		WriteTimeout:      0,
		ReadHeaderTimeout: 10 * time.Second,
	}

	go func() {
		<-ctx.Done()
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()

		if err := publicFacing.Shutdown(shutdownCtx); err != nil {
			log.Error(err, "HTTP server shutdown error")
		}

	}()

	log.Info("Starting HTTP Gateway", "bindAddr", h.BindAddr)
	if err := publicFacing.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		return err
	}

	return nil

}

func (h *HTTPGateway) rewrite(req *httputil.ProxyRequest) {
	// X-Forwarded-For handling
	clientIP := req.In.RemoteAddr
	if xff := req.In.Header.Get("X-Forwarded-For"); xff != "" {
		clientIP = xff
		req.Out.Header.Set("X-Forwarded-For", xff)
	}
	req.Out.Header.Set("X-Real-IP", clientIP)
	req.SetXForwarded()

	// Preserve Connection header from the original request
	if connection := req.In.Header.Get("Connection"); connection != "" {
		req.Out.Header.Set("Connection", connection)
	}

	// Workload Lookup
	lookupRes := h.Resolver.Resolve(req.In.Context(), req.In)

	originalURL := req.In.URL
	originalURL.Host = lookupRes.Hostname
	originalURL.Scheme = lookupRes.Scheme
	// make sure we keep public url information intact. important for query string hashing.
	req.Out.URL = originalURL
	if lookupRes.WorkloadID != "" {
		req.Out.Header.Set("X-Workload-Id", lookupRes.WorkloadID)
	}

	req.Out.Host = req.In.Host
}

func (h *HTTPGateway) modifyResponse(resp *http.Response) error {
	return nil
}

func (h *HTTPGateway) proxyErrorHandler(w http.ResponseWriter, req *http.Request, err error) {
	log := ctrl.LoggerFrom(req.Context()).WithName("http-gateway")
	log.Error(err, "proxy error")

	w.Header().Set("Connection", "close")
	http.Error(w, "Gateway Error", http.StatusBadGateway)
}
