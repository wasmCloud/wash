use std::io::BufWriter;

use clap::{Parser, Subcommand};
use tracing::{error, info, instrument, trace, warn, Level};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

use wash::cli::{
    plugin::ComponentPluginCommand, CliCommand, CliCommandExt, CliContext, CommandOutput,
    OutputKind,
};

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
    /// A command registered and implemented by a Wash component plugin
    #[clap(external_subcommand)]
    ComponentPlugin(Vec<String>),
}

impl CliCommand for WashCliCommand {
    /// Handle the wash command
    #[instrument(level = "debug", skip_all, name = "wash")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            WashCliCommand::Build(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Config(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Dev(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Doctor(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Inspect(cmd) => cmd.handle(ctx).await,
            WashCliCommand::New(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Oci(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Plugin(cmd) => cmd.handle(ctx).await,
            WashCliCommand::Update(cmd) => cmd.handle(ctx).await,
            WashCliCommand::ComponentPlugin(args) => {
                let cmd: ComponentPluginCommand = args.into();
                cmd.handle(&ctx).await
            }
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
            WashCliCommand::ComponentPlugin(args) => {
                let cmd: ComponentPluginCommand = args.into();
                cmd.enable_pre_hook()
            }
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
            WashCliCommand::ComponentPlugin(args) => {
                let cmd: ComponentPluginCommand = args.into();
                cmd.enable_post_hook()
            }
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
        // TODO: If the runtime is executing a component here, we need to stop it.
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
