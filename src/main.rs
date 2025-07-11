use std::io::BufWriter;

use clap::{Parser, Subcommand};
use tracing::error;
use tracing::instrument;
use tracing::warn;
use tracing::{Level, info, trace};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::Registry;
use wash::cli::CliCommandExt;
use wash::cli::{CliCommand, CliContext, CommandOutput, OutputKind};

#[derive(Debug, Clone, Parser)]
#[clap(
    name = "wash",
    about,
    version,
    arg_required_else_help = true,
    color = clap::ColorChoice::Auto
)]
struct Cli {
    #[clap(
        short = 'o',
        long = "output",
        default_value = "text",
        help = "Specify output format (text or json)",
        global = true
    )]
    pub(crate) output: OutputKind,

    #[clap(
        long = "help-markdown",
        conflicts_with = "help",
        help = "Print help in markdown format (conflicts with --help and --output json)",
        hide = true,
        global = true
    )]
    help_markdown: bool,

    #[clap(
        short = 'l',
        long = "log-level",
        default_value = "info",
        help = "Set the log level (trace, debug, info, warn, error)",
        global = true
    )]
    log_level: Level,

    #[clap(long = "verbose", help = "Enable verbose output", global = true)]
    verbose: bool,

    #[clap(subcommand)]
    command: Option<WashCliCommand>,
}

/// The main CLI commands for wash
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Subcommand)]
enum WashCliCommand {
    /// Build a Wasm component
    #[clap(name = "build")]
    Build(wash::cli::component_build::ComponentBuildCommand),
    /// View configuration for wash
    #[clap(name = "config", subcommand)]
    Config(wash::cli::config::ConfigCommand),
    /// Start a development server for a Wasm component
    #[clap(name = "dev")]
    Dev(wash::cli::dev::DevCommand),
    /// Check the health of your wash installation and environment
    #[clap(name = "doctor")]
    Doctor(wash::cli::doctor::DoctorCommand),
    /// Inspect a Wasm component's embedded WIT
    #[clap(name = "inspect", hide = true)]
    Inspect(wash::cli::inspect::InspectCommand),
    /// Create a new project from a template or git repository
    #[clap(name = "new")]
    New(wash::cli::new::NewCommand),
    /// Push or pull Wasm components to/from an OCI registry
    #[clap(name = "oci", alias = "docker", subcommand)]
    Oci(wash::cli::oci::OciCommand),
    /// Manage wash plugins
    #[clap(name = "plugin", subcommand)]
    Plugin(wash::cli::plugin::PluginCommand),
    /// Update wash to the latest version
    #[clap(name = "update", alias = "upgrade")]
    Update(wash::cli::update::UpdateCommand),
}

impl CliCommand for WashCliCommand {
    /// Handle the wash command
    #[instrument(level = "debug", skip_all, name = "wash")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            WashCliCommand::Build(command) => command.handle(ctx).await,
            WashCliCommand::Config(command) => command.handle(ctx).await,
            WashCliCommand::Dev(command) => command.handle(ctx).await,
            WashCliCommand::Doctor(command) => command.handle(ctx).await,
            WashCliCommand::Inspect(command) => command.handle(ctx).await,
            WashCliCommand::New(command) => command.handle(ctx).await,
            WashCliCommand::Oci(command) => command.handle(ctx).await,
            WashCliCommand::Plugin(command) => command.handle(ctx).await,
            WashCliCommand::Update(command) => command.handle(ctx).await,
        }
    }
    fn enable_pre_hook(
        &self,
    ) -> Option<wash::runtime::bindings::plugin_guest::exports::wasmcloud::wash::plugin::HookType>
    {
        match self {
            WashCliCommand::Build(cmd) => cmd.enable_pre_hook(),
            WashCliCommand::Config(cmd) => cmd.enable_pre_hook(),
            WashCliCommand::Dev(cmd) => cmd.enable_pre_hook(),
            WashCliCommand::Doctor(cmd) => cmd.enable_pre_hook(),
            WashCliCommand::Inspect(cmd) => cmd.enable_pre_hook(),
            WashCliCommand::New(cmd) => cmd.enable_pre_hook(),
            WashCliCommand::Oci(cmd) => cmd.enable_pre_hook(),
            WashCliCommand::Plugin(cmd) => cmd.enable_pre_hook(),
            WashCliCommand::Update(cmd) => cmd.enable_pre_hook(),
        }
    }
    fn enable_post_hook(
        &self,
    ) -> Option<wash::runtime::bindings::plugin_guest::exports::wasmcloud::wash::plugin::HookType>
    {
        match self {
            WashCliCommand::Build(cmd) => cmd.enable_post_hook(),
            WashCliCommand::Config(cmd) => cmd.enable_post_hook(),
            WashCliCommand::Dev(cmd) => cmd.enable_post_hook(),
            WashCliCommand::Doctor(cmd) => cmd.enable_post_hook(),
            WashCliCommand::Inspect(cmd) => cmd.enable_post_hook(),
            WashCliCommand::New(cmd) => cmd.enable_post_hook(),
            WashCliCommand::Oci(cmd) => cmd.enable_post_hook(),
            WashCliCommand::Plugin(cmd) => cmd.enable_post_hook(),
            WashCliCommand::Update(cmd) => cmd.enable_post_hook(),
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    trace!(cli = ?cli, "parsed CLI");

    // Implements clap_markdown for markdown generation of command line documentation. Most straightforward way to invoke is probably `wash app get --help-markdown > help.md`
    if cli.help_markdown {
        clap_markdown::print_help_markdown::<Cli>();
        std::process::exit(0);
    };

    // Initialize tracing with the specified log level
    let (stdout, _stderr) = initialize_tracing(cli.log_level, cli.verbose);
    // Use a buffered writer to prevent broken pipe errors
    let mut stdout_buf = BufWriter::new(stdout);

    // Create global context with output kind and directory paths
    let ctx = match CliContext::new().await {
        Ok(ctx) => ctx,
        Err(e) => {
            error!(error = ?e, "failed to infer global context");
            exit_with_output(
                &mut stdout_buf,
                CommandOutput::error(e, None).with_output_kind(cli.output),
            );
        }
    };
    trace!(ctx = ?ctx, "inferred global context");

    // Recommend a new version of wash if available
    if ctx
        .check_new_version()
        .await
        .is_ok_and(|new_version| new_version)
        // Don't show the update message if the user is updating
        && !matches!(cli.command, Some(WashCliCommand::Update(_)))
    {
        info!(
            "a new version of wash is available! Update to the latest version with `wash update`"
        );
    }

    // Since some interactive commands may hide the cursor, we need to ensure it is shown again on exit
    if let Err(e) = ctrlc::set_handler(move || {
        let term = dialoguer::console::Term::stdout();
        let _ = term.show_cursor();
    }) {
        warn!(err = ?e, "failed to set ctrl_c handler, interactive prompts may not restore cursor visibility");
    }

    let Some(command) = cli.command else {
        exit_with_output(
            &mut stdout_buf,
            CommandOutput::error(
                "No command provided. Use `wash --help` to see available commands.",
                None,
            )
            .with_output_kind(cli.output),
        );
    };

    trace!(command = ?command, "running command pre-hook");
    if let Err(e) = command.pre_hook(&ctx).await {
        error!(error = ?e, "failed to run pre-hook for command");
        exit_with_output(
            &mut stdout_buf,
            CommandOutput::error(e, None).with_output_kind(cli.output),
        );
    }

    trace!(command = ?command, "handling command");
    let command_output = command.handle(&ctx).await;

    trace!(command = ?command, "running command post-hook");
    if let Err(e) = command.post_hook(&ctx).await {
        error!(error = ?e, "failed to run post-hook for command");
        exit_with_output(
            &mut stdout_buf,
            CommandOutput::error(e, None).with_output_kind(cli.output),
        );
    }

    match command_output {
        Ok(output) => {
            exit_with_output(&mut stdout_buf, output.with_output_kind(cli.output));
        }
        Err(e) => {
            exit_with_output(
                &mut stdout_buf,
                CommandOutput::error(format!("{e:?}"), None).with_output_kind(cli.output),
            );
        }
    }
}

/// Initialize tracing with a custom format
///
/// Returns a tuple of stdout and stderr writers for consistency with the previous API.
fn initialize_tracing(
    log_level: Level,
    verbose: bool,
) -> (Box<dyn std::io::Write>, Box<dyn std::io::Write>) {
    // Display logs in a compact, CLI-friendly format
    if verbose {
        // Enable dynamic filtering from `RUST_LOG`, fallback to "info"
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(log_level.as_str()));

        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_level(true)
            .with_file(true)
            .with_line_number(true)
            .with_ansi(true); // Color output for TTY

        // Register all layers with the subscriber
        Registry::default().with(env_filter).with(fmt_layer).init();

        (Box::new(std::io::stdout()), Box::new(std::io::stderr()))
    } else {
        // Enable dynamic filtering from `RUST_LOG`, fallback to "info", but always set wasm_pkg_client=error
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(log_level.as_str()))
            // wasm_pkg_client/core are a little verbose so we set them to error level in non-verbose mode
            .add_directive(
                "wasm_pkg_client=error"
                    .parse()
                    .expect("failed to parse wasm_pkg_client directive"),
            )
            .add_directive(
                "wasm_pkg_core=error"
                    .parse()
                    .expect("failed to parse wasm_pkg_core directive"),
            );

        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_level(true)
            .with_file(false)
            .with_line_number(false)
            .with_ansi(true);

        // Register all layers with the subscriber
        Registry::default().with(env_filter).with(fmt_layer).init();

        (Box::new(std::io::stdout()), Box::new(std::io::stderr()))
    }
}

/// Helper function to ensure that we're exiting the program consistently and with the correct output format.
fn exit_with_output(stdout: &mut impl std::io::Write, output: CommandOutput) -> ! {
    let (message, success) = output.render();
    writeln!(stdout, "{message}").expect("failed to write output to stdout");
    stdout.flush().expect("failed to flush stdout");
    if success {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

//     /// Manage declarative applications and deployments (wadm)
//     #[clap(name = "app", subcommand)]
//     App(AppCliCommand),
//     /// Build (and sign) a wasmCloud component or capability provider
//     #[clap(name = "build")]
//     Build(BuildCommand),
//     /// Invoke a simple function on a component running in a wasmCloud host
//     #[clap(name = "call")]
//     Call(CallCli),
//     /// Capture and debug cluster invocations and state
//     #[clap(name = "capture")]
//     Capture(CaptureCommand),
//     /// Generate shell completions for wash
//     #[clap(name = "completions")]
//     Completions(CompletionOpts),
//     /// Generate and manage JWTs for wasmCloud components and capability providers
//     #[clap(name = "claims", subcommand)]
//     Claims(ClaimsCliCommand),
//     /// Create configuration for components, capability providers and links
//     #[clap(name = "config", subcommand)]
//     Config(ConfigCliCommand),
//     /// Manage wasmCloud host configuration contexts
//     #[clap(name = "ctx", alias = "context", alias = "contexts", subcommand)]
//     Ctx(CtxCommand),
//     /// Start a developer loop to hot-reload a local wasmCloud component
//     #[clap(name = "dev")]
//     Dev(DevCommand),
//     /// Tear down a local wasmCloud environment (launched with wash up)
//     #[clap(name = "down")]
//     Down(DownCommand),
//     /// Manage contents of local wasmCloud caches
//     #[clap(name = "drain", subcommand)]
//     Drain(DrainSelection),
//     /// Get information about different running wasmCloud resources
//     #[clap(name = "get", subcommand)]
//     Get(GetCommand),
//     /// Inspect a Wasm component or capability provider for signing information and interfaces
//     #[clap(name = "inspect")]
//     Inspect(InspectCliCommand),
//     /// Generate and manage signing keys
//     #[clap(name = "keys", alias = "key", subcommand)]
//     Keys(KeysCliCommand),
//     /// Link one component to another on a set of interfaces
//     #[clap(name = "link", alias = "links", subcommand)]
//     Link(LinkCommand),

//     /// Create, inspect, and modify capability provider archive files
//     #[clap(name = "par", subcommand)]
//     Par(ParCliCommand),
//     /// Manage wash plugins
//     #[clap(name = "plugin", subcommand)]
//     Plugin(PluginCommand),
//     /// Push an artifact to an OCI compliant registry
//     #[clap(name = "push")]
//     RegPush(RegistryPushCommand),
//     /// Pull an artifact from an OCI compliant registry
//     #[clap(name = "pull")]
//     RegPull(RegistryPullCommand),
//     /// Create secret references for components, capability providers and links
//     #[clap(name = "secrets", alias = "secret", subcommand)]
//     Secrets(SecretsCliCommand),
//     /// Spy on all invocations a component sends and receives
//     #[clap(name = "spy")]
//     Spy(SpyCommand),
//     /// Scale a component running in a host to a certain level of concurrency
//     #[clap(name = "scale", subcommand)]
//     Scale(ScaleCommand),
//     /// Start a component or capability provider
//     #[clap(name = "start", subcommand)]
//     Start(StartCommand),
//     /// Stop a component, capability provider, or host
//     #[clap(name = "stop", subcommand)]
//     Stop(StopCommand),
//     /// Label (or un-label) a host with a key=value label pair
//     #[clap(name = "label", alias = "tag")]
//     Label(LabelHostCommand),
//     /// Update a component running in a host to newer image reference
//     #[clap(name = "update", subcommand)]
//     Update(UpdateCommand),
//     /// Bootstrap a local wasmCloud environment
//     #[clap(name = "up")]
//     Up(UpCommand),
//     /// Serve a web UI for wasmCloud
//     #[clap(name = "ui")]
//     Ui(UiCommand),
//     /// Create wit packages and fetch wit dependencies for a component
//     #[clap(name = "wit", subcommand)]
//     Wit(WitCommand),
// }

// #[tokio::main]
// async fn main() {
//     use clap::CommandFactory;
//     tracing_subscriber::fmt()
//         .with_writer(std::io::stderr)
//         .with_env_filter(EnvFilter::from_default_env())
//         .init();

//     let mut command = Cli::command();

//     // Load plugins if they are not disabled
//     let mut resolved_plugins = None;
//     if std::env::var("WASH_DISABLE_PLUGINS").is_err() {
//         if let Some((plugins, dir)) = load_plugins().await {
//             let mut plugin_paths = HashMap::new();
//             for plugin in plugins.all_metadata().into_iter().cloned() {
//                 if command.find_subcommand(&plugin.id).is_some() {
//                     tracing::error!(
//                         id = %plugin.id,
//                         "Plugin ID matches an existing subcommand, skipping",
//                     );
//                     continue;
//                 }
//                 let (flag_args, path_ids): (Vec<_>, Vec<_>) = plugin
//                     .flags
//                     .into_iter()
//                     .map(|(flag, arg)| {
//                         let trimmed = flag.trim_start_matches('-').to_owned();
//                         // Return a list of args with an Option containing the ID of the flag if it was a path
//                         (
//                             Arg::new(trimmed.clone())
//                                 .long(trimmed.clone())
//                                 .help(arg.description)
//                                 .required(arg.required),
//                             arg.is_path.then_some(trimmed),
//                         )
//                     })
//                     .unzip();
//                 let (positional_args, positional_ids): (Vec<_>, Vec<_>) = plugin
//                     .arguments
//                     .into_iter()
//                     .map(|(name, arg)| {
//                         let trimmed = name.trim().to_owned();
//                         // Return a list of args with an Option containing the ID of the argument if it was a path
//                         (
//                             Arg::new(trimmed.clone())
//                                 .help(arg.description)
//                                 .required(arg.required),
//                             arg.is_path.then_some(trimmed),
//                         )
//                     })
//                     .unzip();
//                 let subcmd = Command::new(plugin.id.clone())
//                     .about(plugin.description)
//                     .author(plugin.author)
//                     .version(plugin.version)
//                     .display_name(plugin.name)
//                     .args(flag_args.into_iter().chain(positional_args));
//                 command = command.subcommand(subcmd);
//                 plugin_paths.insert(
//                     plugin.id,
//                     path_ids
//                         .into_iter()
//                         .chain(positional_ids)
//                         .flatten()
//                         .collect::<Vec<_>>(),
//                 );
//             }
//             resolved_plugins = Some((plugins, dir, plugin_paths));
//         }
//     }

//     command.build();
//     let mut matches = command.get_matches_mut();

//     let cli = match (Cli::from_arg_matches(&matches), resolved_plugins) {
//         // Received a valid CLI command with no parsed known subcommand, but with a matched subcommand
//         // (this is usually a plugin call)
//         (Ok(Cli { command: None, .. }), Some((mut plugins, plugin_dir, plugin_paths)))
//             if matches.subcommand().is_some() =>
//         {
//             run_plugin(&mut matches, None, &mut plugins, &plugin_dir, plugin_paths).await
//         }
//         // Received a valid CLI command
//         (Ok(cli), _) => cli,
//         // Failed to parse the CLI command, which *may* be a plugin execution
//         (Err(mut e), Some((mut plugins, plugin_dir, plugin_paths))) => {
//             run_plugin(
//                 &mut matches,
//                 Some(&mut e),
//                 &mut plugins,
//                 &plugin_dir,
//                 plugin_paths,
//             )
//             .await
//         }
//         // Failed to parse the CLI command, with no possible plugins to call
//         (Err(e), None) => {
//             e.exit();
//         }
//     };

//     // print info on new wash version if available
//     if cli.experimental {
//         if let Err(e) = inform_new_wash_version().await {
//             eprintln!("Error while checking for new wash version: {e}");
//             std::process::exit(2);
//         }
//     }

//     let output_kind = cli.output;

//     let cli_command = cli.command.unwrap_or_else(|| {
//         eprintln!("{}", command.render_help());
//         std::process::exit(2);
//     });

//     // Whether or not to append `success: true` to the output JSON. For now, we only omit it for `wash config get`.
//     let append_json_success = !matches!(
//         cli_command,
//         CliCommand::Config(ConfigCliCommand::GetCommand { .. }),
//     );
//     let res: anyhow::Result<CommandOutput> = match cli_command {
//         CliCommand::App(app_cli) => app::handle_command(app_cli, output_kind).await,
//         CliCommand::Build(build_cli) => build::handle_command(build_cli).await,
//         CliCommand::Call(call_cli) => call::handle_command(call_cli.command()).await,
//         CliCommand::Capture(capture_cli) => {
//             if !cli.experimental {
//                 experimental_error_message("capture")
//             } else if let Some(CaptureSubcommand::Replay(cmd)) = capture_cli.replay {
//                 wash::lib::cli::capture::handle_replay_command(cmd).await
//             } else {
//                 wash::lib::cli::capture::handle_command(capture_cli).await
//             }
//         }
//         CliCommand::Claims(claims_cli) => {
//             wash::lib::cli::claims::handle_command(claims_cli, output_kind).await
//         }
//         CliCommand::Completions(completions_cli) => {
//             completions::handle_command(completions_cli, Cli::command())
//         }
//         CliCommand::Config(config_cli) => config::handle_command(config_cli, output_kind).await,
//         CliCommand::Ctx(ctx_cli) => ctx::handle_command(ctx_cli).await,
//         CliCommand::Dev(dev_cli) => dev::handle_command(dev_cli, output_kind).await,
//         CliCommand::Down(down_cli) => down::handle_command(down_cli, output_kind).await,
//         CliCommand::Drain(drain_cli) => drain::handle_command(drain_cli),
//         CliCommand::Get(get_cli) => common::get_cmd::handle_command(get_cli, output_kind).await,
//         CliCommand::Inspect(inspect_cli) => {
//             wash::lib::cli::inspect::handle_command(inspect_cli, output_kind).await
//         }
//         CliCommand::Keys(keys_cli) => keys::handle_command(keys_cli),
//         CliCommand::Link(link_cli) => link::invoke(link_cli, output_kind).await,
//         CliCommand::New(new_cli) => generate::handle_command(new_cli).await,
//         CliCommand::Par(par_cli) => par::handle_command(par_cli, output_kind).await,
//         CliCommand::Plugin(plugin_cli) => plugin::handle_command(plugin_cli, output_kind).await,
//         CliCommand::RegPush(reg_push_cli) => {
//             common::registry_cmd::registry_push(reg_push_cli, output_kind).await
//         }
//         CliCommand::RegPull(reg_pull_cli) => {
//             common::registry_cmd::registry_pull(reg_pull_cli, output_kind).await
//         }
//         CliCommand::Spy(spy_cli) => {
//             if !cli.experimental {
//                 experimental_error_message("spy")
//             } else {
//                 wash::lib::cli::spy::handle_command(spy_cli).await
//             }
//         }
//         CliCommand::Scale(scale_cli) => {
//             common::scale_cmd::handle_command(scale_cli, output_kind).await
//         }
//         CliCommand::Secrets(secrets_cli) => secrets::handle_command(secrets_cli, output_kind).await,
//         CliCommand::Start(start_cli) => {
//             common::start_cmd::handle_command(start_cli, output_kind).await
//         }
//         CliCommand::Stop(stop_cli) => common::stop_cmd::handle_command(stop_cli, output_kind).await,
//         CliCommand::Label(label_cli) => {
//             common::label_cmd::handle_command(label_cli, output_kind).await
//         }
//         CliCommand::Update(update_cli) => {
//             common::update_cmd::handle_command(update_cli, output_kind).await
//         }
//         CliCommand::Up(up_cli) => up::handle_command(up_cli, output_kind).await,
//         CliCommand::Ui(ui_cli) => ui::handle_command(ui_cli, output_kind).await,
//         CliCommand::Wit(wit_cli) => wit::handle_command(wit_cli).await,
//     };

//     // Use buffered writes to stdout preventing a broken pipe error in case this program has been
//     // piped to another program (e.g. 'wash dev | jq') and CTRL^C has been pressed.
//     let mut stdout_buf = BufWriter::new(stdout().lock());

//     let exit_code: i32 = match res {
//         Ok(out) => {
//             match output_kind {
//                 OutputKind::Json => {
//                     let mut map = out.map;
//                     // When we fetch configuration, we don't want to arbitrarily insert a key into the map.
//                     // There may be other commands we do this in the future, but for now the special check is fine.
//                     if append_json_success {
//                         map.insert("success".to_string(), json!(true));
//                     }
//                     let _ = writeln!(
//                         stdout_buf,
//                         "\n{}",
//                         serde_json::to_string_pretty(&map).unwrap()
//                     );
//                     0
//                 }
//                 OutputKind::Text => {
//                     let _ = writeln!(stdout_buf, "\n{}", out.text);
//                     // on the first non-error, non-json use of wash, print info about shell completions
//                     match completions::first_run_suggestion() {
//                         Ok(Some(suggestion)) => {
//                             let _ = writeln!(stdout_buf, "\n{suggestion}");
//                             0
//                         }
//                         Ok(None) => {
//                             // >1st run,  no message
//                             0
//                         }
//                         Err(e) => {
//                             // error creating first-run token file
//                             eprintln!("\nError: {e}");
//                             1
//                         }
//                     }
//                 }
//             }
//         }
//         Err(e) => {
//             match output_kind {
//                 OutputKind::Json => {
//                     let mut map = HashMap::new();
//                     map.insert("success".to_string(), json!(false));
//                     map.insert("error".to_string(), json!(e.to_string()));

//                     let error_chain = e
//                         .chain()
//                         .skip(1)
//                         .map(|e| format!("{e}"))
//                         .collect::<Vec<String>>();

//                     if !error_chain.is_empty() {
//                         map.insert("error_chain".to_string(), json!(error_chain));
//                     }

//                     let backtrace = e.backtrace().to_string();

//                     if !backtrace.is_empty() && backtrace != "disabled backtrace" {
//                         map.insert("backtrace".to_string(), json!(backtrace));
//                     }

//                     eprintln!("\n{}", serde_json::to_string_pretty(&map).unwrap());
//                 }
//                 OutputKind::Text => {
//                     eprintln!("\n{e:?}");
//                 }
//             }
//             1
//         }
//     };

//     let _ = stdout_buf.flush();
//     std::process::exit(exit_code);
// }

// /// Run a plugin
// async fn run_plugin(
//     matches: &mut ArgMatches,
//     maybe_error: Option<&mut clap::Error>,
//     plugins: &mut SubcommandRunner,
//     plugin_dir: &PathBuf,
//     plugin_paths: HashMap<String, Vec<String>>,
// ) -> ! {
//     let (id, sub_matches) = match matches.subcommand() {
//         Some(data) => data,
//         None => {
//             if let Some(e) = maybe_error {
//                 e.exit();
//             } else {
//                 clap::Error::raw(
//                     clap::error::ErrorKind::InvalidSubcommand,
//                     "failed to find named plugin",
//                 )
//                 .exit();
//             }
//         }
//     };

//     let dir = match ensure_plugin_scratch_dir_exists(plugin_dir, id).await {
//         Ok(dir) => dir,
//         Err(e) => {
//             eprintln!("Error creating plugin scratch directory: {}", e);
//             std::process::exit(1);
//         }
//     };

//     let mut plugin_dirs = Vec::new();

//     // Try fetching all path matches from args marked as paths
//     plugin_dirs.extend(
//         plugin_paths
//             .get(id)
//             .map(ToOwned::to_owned)
//             .unwrap_or_default()
//             .into_iter()
//             .filter_map(|id| {
//                 sub_matches.get_one::<String>(&id).map(|path| DirMapping {
//                     host_path: path.into(),
//                     component_path: None,
//                 })
//             }),
//     );

//     if plugins.metadata(id).is_none() {
//         if let Some(e) = maybe_error {
//             e.insert(
//                 clap::error::ContextKind::InvalidSubcommand,
//                 clap::error::ContextValue::String("No plugin found for subcommand".to_string()),
//             );
//             e.exit();
//         } else {
//             clap::Error::raw(
//                 clap::error::ErrorKind::InvalidSubcommand,
//                 "No plugin found for subcommand",
//             )
//             .exit();
//         }
//     }

//     // NOTE(thomastaylor312): This is a hack to get the raw args to pass to the plugin. I
//     // don't really love this, but we can't add nice help text and structured arguments to
//     // the CLI if we just get the raw args with `allow_external_subcommands(true)`. We can
//     // revisit this later with something if we need to. I did do some basic testing that
//     // even if you wrap wash in a shell script, it still works.
//     let args: Vec<String> = std::env::args().skip(1).collect();
//     if let Err(e) = plugins.run(id, dir, plugin_dirs, args).await {
//         eprintln!("Error running plugin: {e}");
//         std::process::exit(1);
//     } else {
//         std::process::exit(0);
//     }
// }

// fn experimental_error_message(command: &str) -> anyhow::Result<CommandOutput> {
//     bail!("The `wash {command}` command is experimental and may change in future releases. Set the `WASH_EXPERIMENTAL` environment variable or `--experimental` flag to `true` to use this command.")
// }

// /// Helper for loading plugins. If plugins fail to load, we log the error and return `None` so
// /// execution can continue
// async fn load_plugins() -> Option<(SubcommandRunner, PathBuf)> {
//     // We need to use env vars here because the plugin loading needs to be initialized before
//     // the CLI is parsed
//     let plugin_dir = match ensure_plugin_dir(std::env::var("WASH_PLUGIN_DIR").ok()).await {
//         Ok(dir) => dir,
//         Err(e) => {
//             tracing::error!(err = ?e, "Could not load wash plugin directory");
//             return None;
//         }
//     };

//     let plugins = match wash::cli::util::load_plugins(&plugin_dir).await {
//         Ok(plugins) => plugins,
//         Err(e) => {
//             tracing::error!(err = ?e, "Could not load wash plugins");
//             return None;
//         }
//     };

//     Some((plugins, plugin_dir))
// }

// async fn ensure_plugin_scratch_dir_exists(
//     plugin_dir: impl AsRef<Path>,
//     id: &str,
// ) -> anyhow::Result<PathBuf> {
//     let dir = plugin_dir.as_ref().join("scratch").join(id);
//     if !tokio::fs::try_exists(&dir).await.unwrap_or(false) {
//         if let Err(e) = tokio::fs::create_dir_all(&dir).await {
//             bail!("Could not create plugin scratch directory: {e:?}");
//         }
//     }
//     Ok(dir)
// }

// async fn inform_new_wash_version() -> anyhow::Result<()> {
//     let wash_version = Version::parse(clap::crate_version!())
//         .context("failed to parse version from current crate")?;
//     let releases = get_wash_versions_newer_than(&wash_version)
//         .await
//         .context("failed to retrieve newer wash releases")?;
//     if let Some(latest_release) = releases
//         .first()
//         .map(|x| x.get_x_y_z_version())
//         .transpose()?
//     {
//         eprintln!(
//             "{} Consider upgrading to newest wash version: v{latest_release}",
//             emoji::INFO_SQUARE,
//         );
//     }
//     Ok(())
// }
