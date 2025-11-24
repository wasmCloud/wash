package wasmbustest

import (
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats.go"
)

// interface that can be fulfilled by any testing.T implementation ( std, ginkgo, testify )
type TestingT interface {
	Helper()
	Fatalf(string, ...any)
	Logf(string, ...any)
}

func MustStartNats(t TestingT) func() {
	t.Helper()

	opts := &server.Options{
		ServerName:      "test",
		Port:            nats.DefaultPort,
		JetStream:       true,
		NoSigs:          true,
		JetStreamDomain: "default",
	}

	s, err := server.NewServer(opts)
	if err != nil {
		t.Fatalf("failed to create nats server: %v", err)
		return func() {}
	}

	s.Start()

	if !s.ReadyForConnections(5 * time.Second) {
		s.Shutdown()
		t.Fatalf("nats server did not start")
		return func() {}
	}

	return func() {
		s.Shutdown()
		s.WaitForShutdown()
	}
}
