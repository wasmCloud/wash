//go:generate go tool wit-bindgen-go generate --world plugin-guest-http --out gen ../../wit
package main

import (
	"net/http"
	"runtime"
	"time"

	monotonicclock "github.com/wasmcloud/wash/plugins/oauth/gen/wasi/clocks/v0.2.0/monotonic-clock"
	"github.com/wasmcloud/wash/plugins/oauth/gen/wasi/filesystem/v0.2.0/preopens"
	fsTypes "github.com/wasmcloud/wash/plugins/oauth/gen/wasi/filesystem/v0.2.0/types"
	"github.com/wasmcloud/wash/plugins/oauth/gen/wasi/logging/v0.1.0-draft/logging"
	"github.com/wasmcloud/wash/plugins/oauth/gen/wasmcloud/wash/v0.0.1/plugin"
	"github.com/wasmcloud/wash/plugins/oauth/gen/wasmcloud/wash/v0.0.1/types"
	"go.bytecodealliance.org/cm"
	"go.wasmcloud.dev/component/net/wasihttp"
)

var (
	wasiTransport       = &wasihttp.Transport{ConnectTimeout: 30 * time.Second}
	httpClient          = &http.Client{Transport: wasiTransport}
	portConfig          = "port"
	clientIdConfig      = "client_id"
	clientSecretConfig  = "client_secret"
	authenticatedConfig = "authenticated"
)

func init() {
	// Register the http.Handler returned by Router as the handler for all incoming requests.
	wasihttp.Handle(Router())
	plugin.Exports.Info = Info
	plugin.Exports.Initialize = Initialize
	plugin.Exports.Run = Run
	plugin.Exports.Hook = Hook
}

// Info represents the caller-defined, exported function "info".
// Called by wash to retrieve the plugin metadata. Avoid computation or logging here.
func Info() plugin.Metadata {
	flags := []cm.Tuple[string, types.CommandArgument]{
		{
			F0: "port",
			F1: types.CommandArgument{
				Name:        "port",
				Description: "Port to run the OAuth2 server on",
				Env:         cm.None[string](),
				Default:     cm.Some("8888"),
				Value:       cm.None[string](),
			},
		},
		{
			F0: "client-id",
			F1: types.CommandArgument{
				Name:        "client-id",
				Description: "Client ID for the OAuth2 application",
				Env:         cm.Some("WASH_OAUTH_CLIENT_ID"),
				Default:     cm.None[string](),
				Value:       cm.None[string](),
			},
		},
		{
			F0: "client-secret",
			F1: types.CommandArgument{
				Name:        "client-secret",
				Description: "Client secret for the OAuth2 application",
				Env:         cm.Some("WASH_OAUTH_CLIENT_SECRET"),
				Default:     cm.None[string](),
				Value:       cm.None[string](),
			},
		},
	}
	usage := "wash oauth run --port <port> --client-id <client-id> --client-secret <client-secret>"
	cmds := []types.Command{
		{
			ID:          "run",
			Name:        "run",
			Description: "Runs the OAuth2 login flow",
			Flags:       cm.ToList(flags),
			Arguments:   cm.List[types.CommandArgument]{},
			Usage:       cm.NewList(&usage, 1),
		},
	}
	return types.Metadata{
		ID:          "oauth",
		Name:        "oauth",
		Description: "OAuth2 server for authentication",
		Contact:     "wasmCloud Team",
		URL:         "https://github.com/wasmcloud/wash",
		License:     "Apache-2.0",
		Version:     "0.1.0",
		Command:     cm.None[types.Command](),
		SubCommands: cm.ToList(cmds),
		Hooks:       cm.List[types.HookType]{},
	}
}

// NOTE: cm.BoolResult is used semantically similar to a function that returns an error.
// If you return `false`, it is like returning `nil` in Go, indicating success.
// If you return `true`, it indicates an error occurred, similar to returning an error in Go.

// Initialize represents the caller-defined, exported function "initialize".
// Instantiates the plugin. Can check context and fail if information is missing or invalid.
func Initialize(runner plugin.Runner) (result cm.Result[string, string, string]) {
	return cm.OK[cm.Result[string, string, string]]("")
}

// Run represents the caller-defined, exported function "run".
// Executes a given command with the provided arguments.
func Run(runner plugin.Runner, cmd plugin.Command) (result cm.Result[string, string, string]) {
	if cmd.Name != "run" {
		return cm.Err[cm.Result[string, string, string]]("Unsupported command: " + cmd.Name)
	}

	flags := cmd.Flags.Slice()

	port := flagOrDefault(flags, "port")
	if port == nil {
		// This shouldn't happen since we provided a default, but we'll fall back just in case
		fallback := "8888"
		port = &fallback
	}
	clientId := flagOrDefault(flags, "client-id")
	clientSecret := flagOrDefault(flags, "client-secret")

	if clientId == nil || clientSecret == nil {
		logging.Log(logging.LevelError, cmd.Name, "Client ID and Client Secret must be provided")
		return cm.Err[cm.Result[string, string, string]]("Client ID and Client Secret must be provided")
	}

	// SAFETY: We know that port, clientId, and clientSecret are not nil here.
	runnerPluginConfig, err, isErr := runner.PluginConfig().Result()
	if isErr {
		logging.Log(logging.LevelError, cmd.Name, err)
		return cm.Err[cm.Result[string, string, string]]("Failed to get plugin config: " + err)
	}
	runnerPluginConfig.Set(portConfig, *port)
	runnerPluginConfig.Set(clientIdConfig, *clientId)
	runnerPluginConfig.Set(clientSecretConfig, *clientSecret)
	runnerPluginConfig.Set(authenticatedConfig, *clientId+".creds")

	dirs := preopens.GetDirectories().Slice()
	if len(dirs) == 0 {
		logging.Log(logging.LevelError, cmd.Name, "No preopens found for filesystem access, can't validate authentication")
		return cm.Err[cm.Result[string, string, string]]("No preopens found for filesystem access, can't validate authentication")
	}
	if len(dirs) > 1 {
		logging.Log(logging.LevelWarn, cmd.Name, "Multiple preopens found, using the first one for authentication validation")
	}

	dir := dirs[0].F0

	// Now that we've set the context, we wait for the authentication to complete.
	logging.Log(logging.LevelInfo, "oauth", "waiting for authentication at http://localhost:8888/login")
	backoffDuration := 1 * time.Millisecond
	// var res fsTypes.Descriptor
	for {
		_, _, isErr := dir.OpenAt(0, *clientId+".creds", 0, fsTypes.DescriptorFlagsRead).Result()
		if !isErr {
			break
		}
		// res = openedDescriptor
		// TODO: make sure this reads the file and validates?
		logging.Log(logging.LevelDebug, cmd.Name, "creds does not exist, waiting for authentication")
		runtime.Gosched()
		backoff := monotonicclock.SubscribeDuration(monotonicclock.Duration(backoffDuration))
		backoff.Block()
		backoffDuration *= 2
		if backoffDuration > 5*time.Second {
			backoffDuration = 5 * time.Second // Cap the backoff duration
		}
	}
	// Read the file to ensure it exists and log the user DI
	// data, fsErr, isErr := res.Read(4096, 0).Result()
	// if isErr {
	// 	logging.Log(logging.LevelError, "oauth_http", "Failed to read creds file: "+fsErr.String())
	// 	return
	// }

	// userJSON := data.F0.Slice()
	// var tokenInfo map[string]string
	// if err := json.Unmarshal(userJSON, &tokenInfo); err != nil {
	// 	logging.Log(logging.LevelError, "oauth_http", "Failed to parse user JSON: "+err.Error())
	// 	return
	// }
	logging.Log(logging.LevelInfo, cmd.Name, "Authenticated user, stored access token as"+*clientId+".creds") // Read the file to ensure it exists

	// Command success
	return cm.OK[cm.Result[string, string, string]]("Command executed successfully")
}

// Helper function to get a flag value, the default, or return nil if not found.
func flagOrDefault(flags []cm.Tuple[string, types.CommandArgument], name string) *string {
	for _, flag := range flags {
		if flag.F0 == name {
			if flag.F1.Value.Some() != nil {
				return flag.F1.Value.Some()
			}
			return flag.F1.Default.Some()
		}
	}
	return nil
}

// No hooks registered
func Hook(runner plugin.Runner, hook plugin.HookType) cm.Result[string, string, string] {
	return cm.Err[cm.Result[string, string, string]](hook.String() + " is not supported by this plugin")
}

// Since we don't run this program like a CLI, the `main` function is empty. Instead,
// we call the `Run` function when the plugin is invoked by the host.
func main() {}
