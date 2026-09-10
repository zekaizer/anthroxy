use clap::Args;

use super::{Cli, is_wsl};
use crate::config::client_env::{self, EnvFormat};

#[derive(Debug, Clone, Args)]
pub struct EnvArgs {
    /// Host name or address clients use to reach the router
    /// [default: derived from server.listen; "localhost" when it binds every interface]
    #[arg(long, value_name = "HOST")]
    pub host: Option<String>,
    /// Output syntax
    #[arg(long, value_enum, default_value_t = EnvFormat::Sh)]
    pub format: EnvFormat,
}

pub fn run(cli: &Cli, args: &EnvArgs) -> anyhow::Result<()> {
    let config = cli.load_config()?;
    let host = client_env::host(&config, args.host.as_deref());
    let vars = client_env::vars(&config, &client_env::base_url(&config, &host));
    let wsl_localhost = is_wsl() && host == "localhost";
    print!("{}", client_env::render(&vars, args.format, wsl_localhost));
    Ok(())
}
