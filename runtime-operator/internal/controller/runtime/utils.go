package runtime

import (
	"context"
	"fmt"
	"hash/fnv"
	"strings"
	"time"

	"github.com/distribution/reference"
	"github.com/docker/cli/cli/config/configfile"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/pkg/wasmbus"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/emptypb"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/apimachinery/pkg/util/rand"
	"k8s.io/apimachinery/pkg/util/uuid"
	"sigs.k8s.io/controller-runtime/pkg/client"
)

func randHash() string {
	h := fnv.New32a()
	uuid := uuid.NewUUID()
	_, _ = h.Write([]byte(uuid))
	return rand.SafeEncodeString(fmt.Sprint(h.Sum32()))
}

func isOwnedByController(rawObj client.Object, gvk schema.GroupVersionKind) (string, bool) {
	for _, ref := range rawObj.GetOwnerReferences() {
		if ref.Controller != nil && *ref.Controller {
			if ref.APIVersion == gvk.GroupVersion().String() {
				if ref.Kind == gvk.Kind {
					return ref.Name, true
				}
			}
		}
	}

	return "", false
}

func gvkForType(obj client.Object, scheme *runtime.Scheme) (schema.GroupVersionKind, error) {
	gvks, _, err := scheme.ObjectKinds(obj)
	if err != nil {
		return schema.GroupVersionKind{}, err
	}

	if len(gvks) == 0 {
		return schema.GroupVersionKind{}, fmt.Errorf("no GVK found for %T", obj)
	}

	return gvks[0], nil
}

func ResolveConfigFrom(ctx context.Context, kubeClient client.Client, namespace string, configFrom []corev1.LocalObjectReference) (map[string]string, error) {
	configs := make(map[string]string)
	for _, localRef := range configFrom {
		var config corev1.ConfigMap
		if err := kubeClient.Get(ctx, client.ObjectKey{Namespace: namespace, Name: localRef.Name}, &config); err != nil {
			return nil, err
		}
		for key, value := range config.Data {
			configs[key] = value
		}
	}
	return configs, nil
}

func ResolveSecretFrom(ctx context.Context, kubeClient client.Client, namespace string, secretFrom []corev1.LocalObjectReference) (map[string]string, error) {
	secrets := make(map[string]string)
	for _, localRef := range secretFrom {
		var secret corev1.Secret
		if err := kubeClient.Get(ctx, client.ObjectKey{Namespace: namespace, Name: localRef.Name}, &secret); err != nil {
			return nil, err
		}
		for key, value := range secret.Data {
			secrets[key] = string(value)
		}
	}
	return secrets, nil
}

// MergeMaps merges multiple maps of strings into a single map.
func MergeMaps(maps ...map[string]string) map[string]string {
	ret := make(map[string]string)

	for _, m := range maps {
		for k, v := range m {
			ret[k] = v
		}
	}

	return ret
}

func MaterializeImagePullSecret(ctx context.Context,
	kubeClient client.Client,
	namespace string,
	name string,
	image string,
) (*runtimev2.ImagePullSecret, error) {
	var secret corev1.Secret
	if err := kubeClient.Get(ctx, client.ObjectKey{Namespace: namespace, Name: name}, &secret); err != nil {
		return nil, err
	}
	if secret.Type != corev1.SecretTypeDockerConfigJson {
		return nil, fmt.Errorf("image pull secret %q is not of type %q", name, corev1.SecretTypeDockerConfigJson)
	}

	cfg := configfile.New("in-memory")
	if err := cfg.LoadFromReader(strings.NewReader(string(secret.Data[corev1.DockerConfigJsonKey]))); err != nil {
		return nil, fmt.Errorf("loading docker config json from secret %q: %w", name, err)
	}

	// Normalize the image reference to extract the registry domain
	// ex: "ubuntu:latest" -> "docker.io/ubuntu:latest"
	registryRef, err := reference.ParseNormalizedNamed(image)
	if err != nil {
		return nil, fmt.Errorf("parsing image reference %q: %w", image, err)
	}

	// extract the domain from the normalized reference
	// assume the default docker registry if none is specified
	configKey := getAuthConfigKey(reference.Domain(registryRef))
	authConfig, err := cfg.GetAuthConfig(configKey)
	if err != nil {
		return nil, fmt.Errorf("getting auth config for image %q: %w", image, err)
	}

	// NOTE(lxf): here we have the opportunity to run credential helpers if needed

	return &runtimev2.ImagePullSecret{
		Username: authConfig.Username,
		Password: authConfig.Password,
	}, nil
}

func getAuthConfigKey(domainName string) string {
	if domainName == "docker.io" || domainName == "index.docker.io" {
		return "https://index.docker.io/v1/"
	}
	return domainName
}

func MaterializeConfigLayer(ctx context.Context,
	kubeClient client.Client, namespace string, configLayer *runtimev1alpha1.ConfigLayer,
) (map[string]string, error) {
	ret := make(map[string]string)
	if configLayer == nil {
		return ret, nil
	}

	ret = MergeMaps(ret, configLayer.Config)

	configs, err := ResolveConfigFrom(ctx, kubeClient, namespace, configLayer.ConfigFrom)
	if err != nil {
		return nil, fmt.Errorf("resolving local resources config: %w", err)
	}
	ret = MergeMaps(ret, configs)

	secrets, err := ResolveSecretFrom(ctx, kubeClient, namespace, configLayer.SecretFrom)
	if err != nil {
		return nil, fmt.Errorf("resolving local resources secret: %w", err)
	}
	ret = MergeMaps(ret, secrets)

	return ret, nil
}

func NewWorkloadClient(bus wasmbus.Bus, hostID string) *WorkloadClient {
	return &WorkloadClient{
		Bus:    bus,
		HostID: hostID,
	}
}

type WorkloadClient struct {
	Bus    wasmbus.Bus
	HostID string
}

func (w *WorkloadClient) subject(parts ...string) string {
	return strings.Join(append([]string{
		"runtime",
		"host",
		w.HostID,
	}, parts...), ".")
}

func (w *WorkloadClient) Heartbeat(ctx context.Context) (*runtimev2.HostHeartbeat, error) {
	var resp runtimev2.HostHeartbeat
	if err := RoundTrip(ctx, w.Bus, w.subject("heartbeat"), &emptypb.Empty{}, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

func (w *WorkloadClient) Start(ctx context.Context, req *runtimev2.WorkloadStartRequest) (*runtimev2.WorkloadStartResponse, error) {
	var resp runtimev2.WorkloadStartResponse
	if err := RoundTrip(ctx, w.Bus, w.subject("workload.start"), req, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

func (w *WorkloadClient) Status(ctx context.Context, req *runtimev2.WorkloadStatusRequest) (*runtimev2.WorkloadStatusResponse, error) {
	var resp runtimev2.WorkloadStatusResponse
	if err := RoundTrip(ctx, w.Bus, w.subject("workload.status"), req, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

func (w *WorkloadClient) Stop(ctx context.Context, req *runtimev2.WorkloadStopRequest) (*runtimev2.WorkloadStopResponse, error) {
	var resp runtimev2.WorkloadStopResponse
	if err := RoundTrip(ctx, w.Bus, w.subject("workload.stop"), req, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// RoundTrip sends a request and waits for a response.
func RoundTrip[Req proto.Message, Resp proto.Message](ctx context.Context, bus wasmbus.Bus, subject string, req Req, resp Resp) error {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()

	json, err := protojson.Marshal(req)
	if err != nil {
		return err
	}

	msg := wasmbus.NewMessage(subject)
	msg.Data = json

	reply, err := bus.Request(ctx, msg)
	if err != nil {
		return err
	}

	return protojson.Unmarshal(reply.Data, resp)
}
