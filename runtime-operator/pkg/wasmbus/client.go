package wasmbus

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"

	yaml "github.com/goccy/go-yaml"
)

// LatticeRequest encodes the Roundtrip logic for a Bus Request
// This is a generic implementation that can be used with any Bus
// and any Request/Response pair.
// Requests are encoded in json and Responses can be either json or yaml.
// Use Pre & Post Request hooks to modify the request/response before/after.
// The `T` and `Y` types are used to define the Request and Response types.
// `T` should be passed by reference (pointer) and `Y` by value (struct).
type LatticeRequest[T any, Y any] struct {
	// Request should be a reference to the request struct
	Request T
	// Response should be a struct that the response will be unmarshaled into
	// and should be passed by value
	Response    Y
	Subject     string
	Bus         Bus
	PreRequest  func(context.Context, T, *Message) error
	PostRequest func(context.Context, *Y, *Message) error
}

// NewLatticeRequest returns a `LatticeRequest` for a given type.
// The `T` and `Y` types are used to define the Request and Response types.
// `T` should be passed by reference (pointer) and `Y` by value (struct).
func NewLatticeRequest[T any, Y any](bus Bus, subject string, in T, out Y) *LatticeRequest[T, Y] {
	return &LatticeRequest[T, Y]{
		Request:  in,
		Response: out,
		Bus:      bus,
		Subject:  subject,
	}
}

// Execute sends the request to the bus and returns the response.
func (l *LatticeRequest[T, Y]) Execute(ctx context.Context) (*Y, error) {
	req, err := Encode(l.Subject, l.Request)
	if err != nil {
		return nil, fmt.Errorf("%w: %s", ErrEncode, err)
	}

	if l.PreRequest != nil {
		if err := l.PreRequest(ctx, l.Request, req); err != nil {
			return nil, fmt.Errorf("%w: %s", ErrOperation, err)
		}
	}

	rawResp, err := l.Bus.Request(ctx, req)
	if err != nil {
		return nil, fmt.Errorf("%w: %s", ErrTransport, err)
	}

	if err := Decode(rawResp, &l.Response); err != nil {
		return nil, fmt.Errorf("%w: %s", ErrDecode, err)
	}

	if l.PostRequest != nil {
		if err := l.PostRequest(ctx, &l.Response, rawResp); err != nil {
			return nil, fmt.Errorf("%w: %s", ErrOperation, err)
		}
	}

	return &l.Response, nil
}

// Encode marshals the payload into a Message.
// The payload is encoded as json.
func Encode(subject string, payload any) (*Message, error) {
	var err error
	wasmbusMsg := NewMessage(subject)
	wasmbusMsg.Header.Set("Content-Type", mimetypeJSON)
	wasmbusMsg.Data, err = EncodeMimetype(payload, mimetypeJSON)
	return wasmbusMsg, err
}

func EncodeMimetype(payload any, mimeType string) ([]byte, error) {
	switch mimeType {
	case mimetypeJSON, "":
		return json.Marshal(payload)
	case mimeTypeYAML:
		return yaml.Marshal(payload)
	default:
		return nil, errors.New("unsupported content type")
	}
}

// Decode unmarshals the raw response data into the provided struct.
// The content type is used to determine the unmarshaling format.
// If the content type is not supported, an error is returned.
// Acceptable content types are "application/json" and "application/yaml".
func Decode(rawResp *Message, into any) error {
	if len(rawResp.Data) == 0 {
		return nil
	}

	contentType := rawResp.Header.Get("Content-Type")
	switch contentType {
	case mimetypeJSON, "":
		return json.Unmarshal(rawResp.Data, into)
	case mimeTypeYAML:
		return yaml.Unmarshal(rawResp.Data, into)
	default:
		return errors.New("unsupported content type")
	}
}
