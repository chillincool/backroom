//! `backroom`: a command-line spike that exercises backroom-core against soju.

mod loopback;
mod token_file;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use backroom_core::auth::jwt_peek::peek_claims;
use backroom_core::auth::{
    CredentialProvider, OidcClient, OidcConfig, OidcCredentials, StaticCredentials, TokenSet,
    TokenStore,
};
use backroom_core::bouncer::{Network, find_network};
use backroom_core::proto::Message;
use backroom_core::proto::formatting::strip_formatting;
use backroom_core::sasl::SaslCredentials;
use backroom_core::session::RegistrationError;
use backroom_core::transport::{TlsOptions, connector_for_url};
use backroom_core::{Client, ClientError, ConnectOptions};
use chrono::{DateTime, Local};
use clap::{Parser, Subcommand, ValueEnum};
use secrecy::{ExposeSecret, SecretString};

use crate::loopback::Loopback;
use crate::token_file::TokenFile;

/// Exit code when SASL authentication fails, so scripts can tell it apart.
const EXIT_SASL_FAILED: u8 = 2;

#[derive(Parser)]
#[command(name = "backroom", version, about = "Backroom CLI spike for soju")]
struct Cli {
    /// soju endpoint: wss:// or ws:// (WebSocket), ircs:// (TLS) or
    /// irc+insecure:// (plain text).
    #[arg(
        long,
        env = "BACKROOM_URL",
        default_value = "wss://irc.chillincool.net/socket",
        global = true
    )]
    url: String,

    /// Extra CA certificate(s) to trust, PEM (e.g. a self-hosted soju's private CA).
    #[arg(long, env = "BACKROOM_CA_FILE", global = true)]
    ca_file: Option<std::path::PathBuf>,

    /// OIDC issuer (Keycloak realm URL).
    #[arg(
        long,
        env = "BACKROOM_ISSUER",
        default_value = "https://login.chillincool.net/realms/chillincool",
        global = true
    )]
    issuer: String,

    /// Keycloak client ID (public client, PKCE).
    #[arg(
        long,
        env = "BACKROOM_CLIENT_ID",
        default_value = "backroom",
        global = true
    )]
    client_id: String,

    /// Loopback port for the redirect URI http://127.0.0.1:<port>/callback.
    #[arg(
        long,
        env = "BACKROOM_REDIRECT_PORT",
        default_value_t = 47615,
        global = true
    )]
    redirect_port: u16,

    /// Extra OAuth scopes (openid is always requested).
    #[arg(long = "scope", default_values_t = ["soju_roles".to_owned()], global = true)]
    scopes: Vec<String>,

    /// SASL mechanism for connecting to soju.
    #[arg(long, value_enum, default_value_t = Mech::Oauthbearer, global = true)]
    sasl: Mech,

    /// soju username (required for --sasl plain; password from
    /// BACKROOM_PASSWORD or an interactive prompt).
    #[arg(long, env = "BACKROOM_USER", global = true)]
    user: Option<String>,

    /// Nickname to register with (defaults to the account name).
    #[arg(long, global = true)]
    nick: Option<String>,

    /// Print raw IRC traffic to stderr (AUTHENTICATE payloads are redacted).
    #[arg(long, global = true)]
    raw: bool,

    /// More logging (-v debug).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Mech {
    Oauthbearer,
    Plain,
}

#[derive(Subcommand)]
enum Command {
    /// Log in with Keycloak in the browser (OIDC + PKCE) and store tokens.
    Login,
    /// Delete stored tokens.
    Logout,
    /// Show the stored access token's claims (decoded locally, not verified).
    Token,
    /// List upstream networks (soju.im/bouncer-networks).
    Networks,
    /// Print recent history for a channel or nick (draft/chathistory).
    History {
        /// Network ID or name (see `backroom networks`).
        #[arg(long, short = 'N')]
        network: String,
        /// Channel (quote it in the shell: '#chan') or nick.
        target: String,
        /// Number of messages.
        #[arg(long, short = 'n', default_value_t = 50)]
        limit: usize,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(&cli);
    match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(cli: &Cli) {
    let mut filter = match cli.verbose {
        0 => "warn".to_owned(),
        1 => "warn,backroom=debug,backroom_core=debug".to_owned(),
        _ => "debug".to_owned(),
    };
    if cli.raw {
        filter.push_str(",backroom_core::raw=trace");
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BACKROOM_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .init();
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let store = Arc::new(TokenFile::default_location()?);
    match &cli.command {
        Command::Login => login(&cli, store).await?,
        Command::Logout => {
            store.clear()?;
            println!("Removed {}", store.path().display());
        }
        Command::Token => show_token(&store)?,
        Command::Networks => {
            let auth = Auth::new(&cli, store).await?;
            let mut client = match connect(&cli, &auth, None).await? {
                Ok(c) => c,
                Err(code) => return Ok(code),
            };
            let networks = client.list_networks().await?;
            client.quit().await.ok();
            print_networks(&networks);
        }
        Command::History {
            network,
            target,
            limit,
        } => {
            let auth = Auth::new(&cli, store).await?;
            // An unbound connection can list networks but has no history;
            // history needs a second connection bound with BOUNCER BIND.
            let mut control = match connect(&cli, &auth, None).await? {
                Ok(c) => c,
                Err(code) => return Ok(code),
            };
            let networks = control.list_networks().await?;
            control.quit().await.ok();
            let Some(net) = find_network(&networks, network) else {
                print_networks(&networks);
                bail!("no network with ID or name {network:?}");
            };

            let mut bound = match connect(&cli, &auth, Some(net.id.clone())).await? {
                Ok(c) => c,
                Err(code) => return Ok(code),
            };
            let messages = bound.chathistory_latest(target, *limit).await?;
            bound.quit().await.ok();

            if messages.is_empty() {
                println!("(no history for {target} on {})", net.display_name());
            }
            for msg in &messages {
                println!("{}", format_history_line(msg));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn login(cli: &Cli, store: Arc<TokenFile>) -> Result<()> {
    let oidc = OidcClient::discover(oidc_config(cli)).await?;
    let loopback = Loopback::bind(cli.redirect_port).await?;
    let pending = oidc.begin(&loopback.redirect_uri())?;

    println!(
        "Opening the browser to log in. If it doesn't open, visit:\n\n  {}\n",
        pending.auth_url
    );
    if let Err(e) = open::that_detached(pending.auth_url.as_str()) {
        tracing::warn!("could not open browser: {e}");
    }

    let callback = tokio::time::timeout(Duration::from_secs(300), loopback.wait_for_callback())
        .await
        .context("timed out waiting for the browser login (5 minutes)")??;
    let tokens = oidc.complete(pending, &callback).await?;
    store.save(&tokens)?;

    println!(
        "Logged in. Tokens saved to {} (dev only).",
        store.path().display()
    );
    print_token_summary(&tokens);

    // Keycloak drops scopes not assigned to the client, but also omits
    // assigned ones whose "Include in token scope" is off, so only hint.
    if let Some(granted) = &tokens.scope {
        let granted: Vec<&str> = granted.split(' ').collect();
        for scope in cli.scopes.iter().filter(|s| !granted.contains(&s.as_str())) {
            eprintln!(
                "note: scope {scope:?} is not listed as granted. Either it isn't assigned to client \
                 {:?}, or its \"Include in token scope\" setting is off.",
                cli.client_id
            );
        }
    }
    Ok(())
}

fn show_token(store: &TokenFile) -> Result<()> {
    let Some(tokens) = store.load()? else {
        bail!("not logged in (run `backroom login`)");
    };
    print_token_summary(&tokens);
    Ok(())
}

fn print_token_summary(tokens: &TokenSet) {
    println!(
        "Refresh token: {}",
        if tokens.refresh_token.is_some() {
            "present"
        } else {
            "none"
        }
    );
    if let Some(scope) = &tokens.scope {
        println!("Granted scope: {scope}");
    }
    println!("Access token claims (decoded locally, NOT verified):");
    match peek_claims(tokens.access_token.expose_secret()) {
        Ok(claims) => println!("{claims}"),
        Err(e) => println!("  (cannot decode: {e})"),
    }
}

fn oidc_config(cli: &Cli) -> OidcConfig {
    OidcConfig {
        issuer: cli.issuer.clone(),
        client_id: cli.client_id.clone(),
        scopes: cli.scopes.clone(),
    }
}

/// The credential source chosen by `--sasl`.
enum Auth {
    Oidc(Box<OidcCredentials>),
    Plain(StaticCredentials, String),
}

impl Auth {
    async fn new(cli: &Cli, store: Arc<TokenFile>) -> Result<Self> {
        match cli.sasl {
            Mech::Oauthbearer => {
                if store.load()?.is_none() {
                    bail!("not logged in (run `backroom login`, or use --sasl plain)");
                }
                let oidc = OidcClient::discover(oidc_config(cli)).await?;
                Ok(Auth::Oidc(Box::new(OidcCredentials::new(oidc, store))))
            }
            Mech::Plain => {
                let Some(user) = cli.user.clone() else {
                    bail!("--sasl plain needs --user (or BACKROOM_USER)");
                };
                let password = match std::env::var("BACKROOM_PASSWORD") {
                    Ok(p) => SecretString::from(p),
                    Err(_) => SecretString::from(
                        rpassword::prompt_password(format!("soju password for {user}: "))
                            .context("reading password")?,
                    ),
                };
                let creds = SaslCredentials::Plain {
                    username: user.clone(),
                    password,
                };
                Ok(Auth::Plain(StaticCredentials(creds), user))
            }
        }
    }

    fn provider(&self) -> &dyn CredentialProvider {
        match self {
            Auth::Oidc(p) => p.as_ref(),
            Auth::Plain(p, _) => p,
        }
    }

    /// Nick to register with: account name if we can tell, else "backroom".
    async fn default_nick(&self) -> String {
        match self {
            Auth::Plain(_, user) => user.clone(),
            Auth::Oidc(p) => p
                .fresh_tokens()
                .await
                .ok()
                .and_then(|t| peek_claims(t.access_token.expose_secret()).ok())
                .and_then(|c| c.preferred_username)
                .unwrap_or_else(|| "backroom".into()),
        }
    }
}

/// Connects; on SASL failure prints diagnostics and returns the exit code.
async fn connect(
    cli: &Cli,
    auth: &Auth,
    bind_network: Option<String>,
) -> Result<std::result::Result<Client, ExitCode>> {
    let mut tls = TlsOptions::default();
    if let Some(path) = &cli.ca_file {
        tls = tls.with_ca_file(path)?;
    }
    let connector = connector_for_url(&cli.url, tls)?;
    let nick = match &cli.nick {
        Some(n) => n.clone(),
        None => auth.default_nick().await,
    };
    let mut opts = ConnectOptions::new(nick);
    opts.bind_network = bind_network;

    match Client::connect(connector.as_ref(), auth.provider(), opts).await {
        Ok(client) => Ok(Ok(client)),
        Err(ClientError::Registration(RegistrationError::SaslFailed {
            mechanism,
            line,
            challenge,
            server_mechanisms,
        })) => {
            eprintln!("SASL {mechanism} authentication FAILED.");
            eprintln!("  soju replied: {line}");
            if let Some(c) = challenge {
                eprintln!("  RFC 7628 error challenge: {c}");
            }
            if let Some(m) = server_mechanisms {
                eprintln!("  server mechanisms: {m}");
            }
            if let Auth::Oidc(p) = auth {
                if let Ok(tokens) = p.fresh_tokens().await {
                    eprintln!("Token that was sent (claims decoded locally, NOT verified):");
                    match peek_claims(tokens.access_token.expose_secret()) {
                        Ok(claims) => eprintln!("{claims}"),
                        Err(e) => eprintln!("  (cannot decode: {e})"),
                    }
                }
                if line.ends_with(":Invalid credentials") {
                    // soju v0.11 sends this only when introspection says `active: false`.
                    eprintln!(
                        "\n\"Invalid credentials\" means Keycloak's introspection returned active=false.\n\
                         Since Keycloak 26.6.2 that happens unless soju's introspection client (the user \
                         part of its `auth oauth2` URL) is in the token's `aud` claim."
                    );
                }
                eprintln!(
                    "\nFor other failures the reason (introspection status, missing username, …) is \
                     only in soju's log, e.g.:\n  kubectl -n util logs deploy/soju | grep -i sasl"
                );
            }
            Ok(Err(ExitCode::from(EXIT_SASL_FAILED)))
        }
        Err(e) => Err(e.into()),
    }
}

fn print_networks(networks: &[Network]) {
    if networks.is_empty() {
        println!("(no networks)");
        return;
    }
    println!("{:<6} {:<24} {:<13} ERROR", "ID", "NAME", "STATE");
    for n in networks {
        println!(
            "{:<6} {:<24} {:<13} {}",
            n.id,
            sanitize(n.display_name()),
            n.state().unwrap_or("?"),
            sanitize(n.attr("error").unwrap_or(""))
        );
    }
}

fn format_history_line(msg: &Message) -> String {
    let time = msg
        .tag("time")
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| {
            t.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "????-??-?? ??:??:??".into());
    let nick = sanitize(msg.source_nick().unwrap_or("*"));
    let clean = |t: &str| sanitize(&strip_formatting(t));

    let body = match (msg.command.as_str(), msg.param(1)) {
        ("PRIVMSG", Some(t)) => match t.strip_prefix("\u{1}ACTION ") {
            Some(action) => format!("* {nick} {}", clean(action.trim_end_matches('\u{1}'))),
            None => format!("<{nick}> {}", clean(t)),
        },
        ("NOTICE", Some(t)) => format!("-{nick}- {}", clean(t)),
        (cmd, _) => format!("-- {nick} {cmd} {}", sanitize(&msg.params.join(" "))),
    };
    format!("{time}  {body}")
}

/// Replaces control characters so server text can't drive the terminal.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}
