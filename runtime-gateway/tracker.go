package main

import (
	"context"
	"errors"
	"net"
	"net/http"
	"strconv"
	"sync"

	"k8s.io/apimachinery/pkg/util/sets"
	ctrl "sigs.k8s.io/controller-runtime"
)

var ErrHostnameNotFound = errors.New("hostname not found")

type LookupResult struct {
	Hostname   string
	Scheme     string
	WorkloadID string
}

type HostResolver interface {
	Resolve(ctx context.Context, req *http.Request) LookupResult
}

type HostRegistry interface {
	RegisterHost(ctx context.Context, hostID string, hostname string, port int) error
	DeregisterHost(ctx context.Context, hostID string) error
}

type WorkloadRegistry interface {
	RegisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string) error
	DeregisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string) error
}

var _ HostResolver = (*HostTracker)(nil)
var _ HostRegistry = (*HostTracker)(nil)
var _ WorkloadRegistry = (*HostTracker)(nil)

type HostTracker struct {
	once sync.Once
	lock sync.RWMutex
	// internal fallback endpoint, for when no workloads are registered
	fallbackEndpoint string
	// HostID to "hostname:port"
	hosts map[string]string
	// hostname to WorkloadID
	hostnames map[string]sets.Set[string]
	// WorkloadID to HostID
	workloads map[string]string
}

func (ht *HostTracker) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	ht.once.Do(func() {
		ht.hosts = make(map[string]string)
		ht.hostnames = make(map[string]sets.Set[string])
		ht.workloads = make(map[string]string)
	})
	return manager.Add(ht)
}

func (ht *HostTracker) Start(ctx context.Context) error {
	internalMux := http.NewServeMux()

	internalMux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Connection", "close")
		http.Error(w, "No backend available", http.StatusServiceUnavailable)
	})

	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return err
	}

	ht.fallbackEndpoint = listener.Addr().String()
	log := ctrl.LoggerFrom(ctx).WithName("host-tracker")
	log.Info("HostTracker internal fallback endpoint", "endpoint", ht.fallbackEndpoint)

	internalServer := &http.Server{
		Handler: internalMux,
	}
	go func() {
		<-ctx.Done()
		_ = internalServer.Close()
	}()

	return internalServer.Serve(listener)
}

func (ht *HostTracker) Resolve(ctx context.Context, req *http.Request) LookupResult {
	ht.lock.RLock()
	defer ht.lock.RUnlock()

	workloads, ok := ht.hostnames[req.Host]
	if !ok || len(workloads) == 0 {
		return LookupResult{
			Hostname: ht.fallbackEndpoint,
			Scheme:   "http",
		}
	}

	// pick a random workload
	workloadID := workloads.UnsortedList()[0]

	hostID, ok := ht.workloads[workloadID]
	if !ok {
		return LookupResult{
			Hostname: ht.fallbackEndpoint,
			Scheme:   "http",
		}
	}

	hostname, ok := ht.hosts[hostID]
	if !ok {
		return LookupResult{
			Hostname: ht.fallbackEndpoint,
			Scheme:   "http",
		}
	}

	return LookupResult{
		Hostname:   hostname,
		Scheme:     "http",
		WorkloadID: workloadID,
	}
}

func (ht *HostTracker) RegisterHost(ctx context.Context, hostID string, hostname string, port int) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	ht.hosts[hostID] = net.JoinHostPort(hostname, strconv.Itoa(port))
	return nil
}

func (ht *HostTracker) DeregisterHost(ctx context.Context, hostID string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	delete(ht.hosts, hostID)
	return nil
}

func (ht *HostTracker) RegisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	ht.workloads[workloadID] = hostID
	if workloadSet, ok := ht.hostnames[hostname]; !ok {
		ht.hostnames[hostname] = sets.New(workloadID)
	} else {
		workloadSet.Insert(workloadID)
	}
	return nil
}

func (ht *HostTracker) DeregisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	delete(ht.workloads, workloadID)
	if workloadSet, ok := ht.hostnames[hostname]; ok {
		workloadSet.Delete(workloadID)
		if workloadSet.Len() == 0 {
			delete(ht.hostnames, hostname)
		}
	}
	return nil
}
