//! Shared CLI command parsing and dispatch for Coral clients.

#![allow(
    unused_crate_dependencies,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI intentionally renders user-facing output and the package includes test-only dependencies."
)]

mod bootstrap;
mod branding;
mod browser;
#[cfg(feature = "embedded-ui")]
mod embedded_ui;
pub mod env;
mod onboard;
mod query_error;
mod source_ops;

#[cfg(any(feature = "server", feature = "embedded-ui"))]
use std::net::SocketAddr;
use std::path::PathBuf;
#[cfg(feature = "embedded-ui")]
use std::sync::Arc;

use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use coral_api::v1::ExecuteSqlRequest;
#[cfg(feature = "embedded-ui")]
use coral_app::StaticAssetsProvider;
use coral_client::{
    AppClient, decode_execute_sql_response, default_workspace, format_batches_json,
    format_batches_table,
};
use dialoguer::console::measure_text_width;
use tonic::Request;

#[cfg(test)]
use tempfile as _;

/// Default loopback address used by `coral server` and `coral ui` to expose a
/// browser-facing gRPC-Web surface.
#[cfg(any(feature = "server", feature = "embedded-ui"))]
const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:1457";

#[derive(Debug, Parser)]
#[command(name = "coral", version, arg_required_else_help = true)]
/// A local-first SQL interface for APIs, files, and other data sources.
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Execute a SQL query
    Sql(SqlArgs),
    /// Manage data sources
    Source(SourceArgs),
    /// Interactive wizard to set up Coral and explore use cases
    Onboard,
    /// Start the MCP server over stdio
    McpStdio,
    #[cfg(feature = "server")]
    /// Start the local gRPC-Web server (use with the UI dev server)
    Server(ServerArgs),
    #[cfg(feature = "embedded-ui")]
    /// Start the local gRPC-Web server with the embedded Coral UI
    Ui(ServerArgs),
    /// Generate shell completion scripts
    Completion(CompletionArgs),
}

/// Runtime a command needs before it can execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredRuntime {
    AppClient,
    None,
}

#[cfg(any(feature = "server", feature = "embedded-ui"))]
#[derive(Debug, Clone, Copy, Args)]
/// Local browser-facing server options
struct ServerArgs {
    /// Address to bind for the local gRPC-Web server
    #[arg(long = "addr", value_name = "ADDR", default_value = DEFAULT_SERVER_ADDR)]
    bind_addr: SocketAddr,
}

#[derive(Debug, Args)]
/// Generate shell completion scripts
struct CompletionArgs {
    /// Shell to generate completions for
    shell: Shell,
}

#[derive(Debug, Args)]
/// Execute a SQL query
struct SqlArgs {
    /// Output format for query results
    #[arg(long, value_enum, default_value = "table")]
    format: OutputFormat,
    /// SQL query to execute
    sql: String,
}

#[derive(Debug, Args)]
/// Manage data sources
struct SourceArgs {
    #[command(subcommand)]
    command: SourceCommand,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("source_input")
        .args(["name", "file"])
        .required(true)
        .multiple(false)
))]
struct SourceAddArgs {
    /// Name for the new source
    name: Option<String>,

    /// Path to a file
    #[arg(long)]
    file: Option<PathBuf>,

    /// Prompt for input values interactively. When unset, values are read from
    /// environment variables matching each input key.
    #[arg(long)]
    interactive: bool,
}

#[derive(Debug, Subcommand)]
enum SourceCommand {
    /// Discover available sources
    Discover,
    /// List configured sources
    List,
    /// Show metadata for a source
    Info {
        /// Name of the source to show info for
        name: String,
        /// Show additional details such as input hints
        #[arg(short, long)]
        verbose: bool,
    },
    /// Add a new source
    Add(SourceAddArgs),
    /// Lint manifest file
    Lint { file: PathBuf },
    /// Test connectivity for a source
    Test {
        /// Name of the source to test
        name: String,
    },
    /// Remove a source
    Remove {
        /// Name of the source to remove
        name: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
}

/// Typed CLI error whose stderr rendering and exit code are owned by the binary.
#[derive(Debug, thiserror::Error)]
#[error("cli command failed")]
pub struct CliExitError {
    rendered_stderr: String,
}

impl CliExitError {
    #[must_use]
    /// Builds a CLI error with pre-rendered stderr output.
    pub fn new(rendered_stderr: String) -> Self {
        Self { rendered_stderr }
    }

    #[must_use]
    /// Returns the stderr block the binary should render before exiting.
    pub fn rendered_stderr(&self) -> &str {
        &self.rendered_stderr
    }
}

impl Command {
    fn required_runtime(&self) -> RequiredRuntime {
        match self {
            Command::Sql(_) | Command::Source(_) | Command::Onboard | Command::McpStdio => {
                RequiredRuntime::AppClient
            }
            Command::Completion(_) => RequiredRuntime::None,
            #[cfg(feature = "server")]
            Command::Server(_) => RequiredRuntime::None,
            #[cfg(feature = "embedded-ui")]
            Command::Ui(_) => RequiredRuntime::None,
        }
    }
}

/// Parses CLI arguments, starts the runtime required by the selected command,
/// and runs the command.
///
/// # Errors
///
/// Returns an error if runtime startup, command execution, or output
/// formatting fails.
pub async fn run_from_env() -> Result<(), anyhow::Error> {
    let Cli { command } = Cli::parse();
    match command.required_runtime() {
        RequiredRuntime::AppClient => {
            let bootstrap::Bootstrap { app, _server } = bootstrap::bootstrap().await?;
            run_app_command(app, command).await
        }
        RequiredRuntime::None => run_no_runtime_command(command).await,
    }
}

/// Returns the embedded Coral UI assets for the local server to serve.
#[cfg(feature = "embedded-ui")]
#[must_use]
pub fn embedded_ui_assets() -> Arc<dyn StaticAssetsProvider> {
    Arc::new(embedded_ui::EmbeddedUi)
}

/// Opens the given URL in the user's default browser.
///
/// # Errors
///
/// Returns an error if the platform browser opener fails.
#[cfg(any(feature = "server", feature = "embedded-ui"))]
pub fn open_url(url: &str) -> Result<(), std::io::Error> {
    browser::open_url(url)
}

#[cfg(feature = "server")]
async fn run_dev_server(bind_addr: SocketAddr) -> Result<(), anyhow::Error> {
    let server = bootstrap::start_dev_server(bind_addr).await?;
    let endpoint = server.endpoint_uri().to_string();

    println!("Coral gRPC-Web server listening on {endpoint}");
    println!("Run the UI dev server (e.g. `npm run dev` in ui/) and proxy gRPC-Web requests here.");
    println!("Press Ctrl-C to stop the server.");

    let signal = tokio::signal::ctrl_c().await;
    let shutdown = server.shutdown().await;
    signal?;
    shutdown?;
    Ok(())
}

#[cfg(feature = "embedded-ui")]
async fn run_ui(bind_addr: SocketAddr) -> Result<(), anyhow::Error> {
    let server = bootstrap::start_ui_server(bind_addr).await?;
    let endpoint = server.endpoint_uri().to_string();

    println!("Coral UI listening on {endpoint}");
    match open_url(&endpoint) {
        Ok(()) => println!("Opened {endpoint}"),
        Err(error) => {
            eprintln!("Could not open browser: {error}");
            eprintln!("Open {endpoint} manually.");
        }
    }
    println!("Press Ctrl-C to stop the UI.");

    let signal = tokio::signal::ctrl_c().await;
    let shutdown = server.shutdown().await;
    signal?;
    shutdown?;
    Ok(())
}

/// Parses CLI arguments and runs the shared Coral CLI.
///
/// # Errors
///
/// Returns an error if argument parsing, command execution, or output
/// formatting fails.
pub async fn run(app: AppClient) -> Result<(), anyhow::Error> {
    let Cli { command } = Cli::parse();
    match command.required_runtime() {
        RequiredRuntime::AppClient => run_app_command(app, command).await,
        RequiredRuntime::None => run_no_runtime_command(command).await,
    }
}

async fn run_no_runtime_command(command: Command) -> Result<(), anyhow::Error> {
    match command {
        Command::Completion(args) => {
            run_completion(&args);
            Ok(())
        }
        #[cfg(feature = "server")]
        Command::Server(args) => run_dev_server(args.bind_addr).await,
        #[cfg(feature = "embedded-ui")]
        Command::Ui(args) => run_ui(args.bind_addr).await,
        Command::Sql(_) | Command::Source(_) | Command::Onboard | Command::McpStdio => {
            unreachable!("app client commands are routed through app runtime startup")
        }
    }
}

async fn run_app_command(app: AppClient, command: Command) -> Result<(), anyhow::Error> {
    match command {
        Command::Sql(args) => run_sql(&app, args).await?,
        Command::Source(args) => run_source(&app, args).await?,
        Command::Onboard => {
            onboard::run(&app).await?;
        }
        Command::McpStdio => {
            coral_mcp::run_stdio_with_client(app).await?;
        }
        Command::Completion(args) => {
            run_completion(&args);
        }
        #[cfg(feature = "server")]
        Command::Server(_) => {
            unreachable!("no-runtime commands are routed without an app client")
        }
        #[cfg(feature = "embedded-ui")]
        Command::Ui(_) => {
            unreachable!("no-runtime commands are routed without an app client")
        }
    }

    Ok(())
}

fn run_completion(args: &CompletionArgs) {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    generate(args.shell, &mut cmd, bin_name, &mut std::io::stdout());
}

async fn run_sql(app: &AppClient, args: SqlArgs) -> Result<(), anyhow::Error> {
    let response = match app
        .query_client()
        .execute_sql(Request::new(ExecuteSqlRequest {
            workspace: Some(default_workspace()),
            sql: args.sql,
        }))
        .await
    {
        Ok(response) => response.into_inner(),
        Err(status) => {
            return Err(CliExitError::new(query_error::render_query_error(&status)).into());
        }
    };
    let result = decode_execute_sql_response(&response)?;
    print_batches(result.batches(), args.format)?;
    Ok(())
}

async fn run_source(app: &AppClient, args: SourceArgs) -> Result<(), anyhow::Error> {
    match args.command {
        SourceCommand::Discover => {
            let sources = source_ops::discover_sources(app).await?;
            if sources.is_empty() {
                println!("No bundled sources available.");
            } else {
                let rows = sources.into_iter().map(|source| {
                    let status = if source.installed {
                        "installed".to_string()
                    } else {
                        "available".to_string()
                    };
                    [source.name, source.version, status]
                });
                print_text_table(["Source", "Version", "Status"], rows);
            }
        }
        SourceCommand::List => {
            let sources = source_ops::list_sources(app).await?;
            if sources.is_empty() {
                println!("No sources configured.");
            } else {
                let rows = sources.into_iter().map(|source| {
                    [
                        source.name,
                        source.version,
                        source_ops::source_origin_label(source.origin).to_string(),
                    ]
                });
                print_text_table(["Source", "Version", "Origin"], rows);
            }
        }
        SourceCommand::Info { name, verbose } => {
            source_ops::print_source_info(app, &name, verbose).await?;
        }
        SourceCommand::Add(args) => run_source_add(app, args).await?,
        SourceCommand::Lint { file } => {
            source_ops::load_validated_manifest_file(&file)?;
            println!("Manifest is valid");
        }
        SourceCommand::Test { name } => {
            source_ops::test_and_print(
                app,
                &name,
                source_ops::TableDisplayLimit::All,
                source_ops::ValidationSeverityMode::Strict,
            )
            .await?;
        }
        SourceCommand::Remove { name } => {
            source_ops::delete_source(app, &name).await?;
            println!("Removed source {name}");
        }
    }
    Ok(())
}

fn print_batches(
    batches: &[arrow::record_batch::RecordBatch],
    format: OutputFormat,
) -> Result<(), anyhow::Error> {
    let output = match format {
        OutputFormat::Table => format_batches_table(batches)?,
        OutputFormat::Json => format_batches_json(batches)?,
    };
    println!("{output}");
    Ok(())
}

fn print_text_table<const COLUMNS: usize>(
    headers: [&str; COLUMNS],
    rows: impl IntoIterator<Item = [String; COLUMNS]>,
) {
    let rows = rows.into_iter().collect::<Vec<_>>();
    let widths = compute_column_widths(headers, &rows);

    println!("{}", format_table_row(headers, &widths));
    println!("{}", format_separator_row(&widths));
    for row in rows {
        println!("{}", format_table_row(row.each_ref(), &widths));
    }
}

fn compute_column_widths<const COLUMNS: usize>(
    headers: [&str; COLUMNS],
    rows: &[[String; COLUMNS]],
) -> [usize; COLUMNS] {
    std::array::from_fn(|idx| {
        let header_width = measure_text_width(headers[idx]);
        let row_width = rows
            .iter()
            .map(|row| measure_text_width(&row[idx]))
            .max()
            .unwrap_or(0);
        header_width.max(row_width)
    })
}

fn format_table_row<const COLUMNS: usize, T>(
    cells: [T; COLUMNS],
    widths: &[usize; COLUMNS],
) -> String
where
    T: AsRef<str>,
{
    cells
        .into_iter()
        .enumerate()
        .map(|(idx, cell)| pad_cell(cell.as_ref(), widths[idx], idx + 1 < COLUMNS))
        .collect::<Vec<_>>()
        .join("  ")
}

fn format_separator_row<const COLUMNS: usize>(widths: &[usize; COLUMNS]) -> String {
    widths
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join("  ")
}

fn pad_cell(value: &str, width: usize, pad: bool) -> String {
    if !pad {
        return value.to_string();
    }

    let padding = width.saturating_sub(measure_text_width(value));
    format!("{value}{}", " ".repeat(padding))
}

async fn run_source_add(app: &AppClient, args: SourceAddArgs) -> Result<(), anyhow::Error> {
    let SourceAddArgs {
        name,
        file,
        interactive,
    } = args;
    if interactive {
        source_ops::require_interactive()?;
    }
    let collect = |inputs: &[coral_spec::ManifestInputSpec]| {
        if interactive {
            source_ops::prompt_for_inputs(inputs)
        } else {
            source_ops::collect_inputs_from_env(inputs)
        }
    };
    let response = match (name, file) {
        (Some(name), None) => {
            let bundled_name = source_ops::source_name_arg(Some(&name))?;
            let discover = source_ops::discover_sources(app).await?;
            let available = discover
                .into_iter()
                .find(|source| source.name == bundled_name)
                .ok_or_else(|| anyhow::anyhow!("unknown bundled source '{bundled_name}'"))?;
            let inputs = available
                .inputs
                .iter()
                .map(source_ops::manifest_input_from_proto)
                .collect::<Result<Vec<_>, _>>()?;
            let (variables, secrets) = collect(&inputs)?;
            source_ops::add_bundled_source(app, &available.name, variables, secrets).await?
        }
        (None, Some(file)) => {
            let (manifest_yaml, manifest) = source_ops::load_validated_manifest_file(&file)?;
            let (variables, secrets) = collect(manifest.declared_inputs())?;
            source_ops::import_source(app, manifest_yaml, variables, secrets).await?
        }
        _ => unreachable!("clap enforces exactly one of name or file"),
    };
    println!("Added source {}", response.name);
    source_ops::validate_and_warn(app, &response.name, source_ops::TableDisplayLimit::DEFAULT).await
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, RequiredRuntime};
    use clap::Parser;

    #[cfg(feature = "server")]
    #[test]
    fn server_command_uses_custom_bind_addr_without_required_runtime() {
        let cli = Cli::try_parse_from(["coral", "server", "--addr", "127.0.0.1:1458"])
            .expect("server args should parse");

        assert_eq!(cli.command.required_runtime(), RequiredRuntime::None);
        let Command::Server(args) = cli.command else {
            panic!("expected server command");
        };
        assert_eq!(
            args.bind_addr,
            "127.0.0.1:1458".parse().expect("socket addr")
        );
    }

    #[cfg(feature = "embedded-ui")]
    #[test]
    fn ui_command_uses_custom_bind_addr_without_required_runtime() {
        let cli = Cli::try_parse_from(["coral", "ui", "--addr", "127.0.0.1:1459"])
            .expect("ui args should parse");

        assert_eq!(cli.command.required_runtime(), RequiredRuntime::None);
        let Command::Ui(args) = cli.command else {
            panic!("expected ui command");
        };
        assert_eq!(
            args.bind_addr,
            "127.0.0.1:1459".parse().expect("socket addr")
        );
    }

    #[test]
    fn completion_requires_no_runtime() {
        let cli = Cli::try_parse_from(["coral", "completion", "bash"])
            .expect("completion args should parse");

        assert_eq!(cli.command.required_runtime(), RequiredRuntime::None);
    }

    #[test]
    fn regular_commands_use_normal_app_bootstrap() {
        let cli = Cli::try_parse_from(["coral", "source", "list"]).expect("source list parses");

        assert_eq!(cli.command.required_runtime(), RequiredRuntime::AppClient);
    }
}
