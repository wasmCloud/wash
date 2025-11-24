package wasmbus

import (
	"context"
	"errors"
	"net/http"
	"strings"

	"github.com/nats-io/nats.go"
)

const (
	// PatternAll is a wildcard pattern that matches all subjects.
	PatternAll = "*"

	// NoBackLog is used to indicate that the subscription should not have a backlog.
	NoBackLog = 0

	mimetypeJSON = "application/json"
	mimeTypeYAML = "application/yaml"
)

var (
	// ErrEncode is returned when encoding a message fails.
	ErrEncode = errors.New("encode error")
	// ErrInternal is returned when an internal error occurs.
	ErrInternal = errors.New("internal error")
	// ErrDecode is returned when decoding a message fails.
	ErrDecode = errors.New("decode error")
	// ErrTransport is returned when a transport error occurs.
	ErrTransport = errors.New("transport error")
	// ErrOperation is returned when an operation error occurs.
	ErrOperation = errors.New("operation error")
	// ErrValidation is returned when a validation error occurs.
	ErrValidation = errors.New("validation error")
)

// Bus is the interface for the message bus.
// It provides methods for subscribing to messages and sending messages.
// It doesn't hold any state and is safe for concurrent use. See `Subscription` for stateful operations.
// Modeled after the NATS API.
type Bus interface {
	// Subscribe creates a subscription for the given subject.
	// The backlog parameter is the maximum number of messages that can be buffered in memory.
	Subscribe(subject string, backlog int) (Subscription, error)
	// QueueSubscribe creates a subscription for the given subject and queue group.
	// The backlog parameter is the maximum number of messages that can be buffered in memory.
	QueueSubscribe(subject string, queue string, backlog int) (Subscription, error)
	// Request sends a request message and waits for a reply.
	// The context is used for the request timeout.
	Request(ctx context.Context, msg *Message) (*Message, error)
	// Publish sends a message to `msg.Subject`.
	Publish(msg *Message) error
}

// Subscription is the interface for a message subscription.
// It provides methods for handling messages and draining the subscription.
// Subscriptions run in their own goroutine.
type Subscription interface {
	// Handle starts the subscription and calls the callback for each message.
	// Does not block.
	Handle(callback SubscriptionCallback)
	// Drain stops the subscription and closes the channel.
	// Blocks until all messages are processed, releasing the Subscription Thread.
	Drain() error
}

// SubscriptionCallback is the callback type for message subscriptions.
// Subscriptions are single threaded and the callback is called sequentially for each message.
// Callers should handle concurrency and synchronization themselves.
type SubscriptionCallback func(msg *Message)

// Header is the Message header type.
type Header = http.Header

// Message is the message type for the message bus.
// Modeled after the NATS message type.
type Message struct {
	Subject string
	Reply   string
	Header  Header
	Data    []byte

	// For replies
	bus Bus
}

// Bus returns the bus that the message was received on or sent to
// Might be null.
func (m *Message) Bus() Bus {
	return m.bus
}

// LastSubjectPart returns the last part of the subject.
// Example: "a.b.c" -> "c"
func (m *Message) LastSubjectPart() string {
	parts := m.SubjectParts()
	return parts[len(parts)-1]
}

// SubjectParts returns the parts of the subject.
// Example: "a.b.c" -> ["a", "b", "c"]
func (m *Message) SubjectParts() []string {
	return strings.Split(m.Subject, ".")
}

// NewMessage creates a new message with the given subject.
func NewMessage(subject string) *Message {
	return &Message{
		Header:  make(Header),
		Subject: subject,
	}
}

func NewInbox() string {
	return nats.NewInbox()
}
