use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(version, about, author, long_about = None)]
pub struct CmdArgs {
    #[arg(short, long, default_value = "./config")]
    pub config: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmd_args() {
        use clap::CommandFactory;
        CmdArgs::command().debug_assert();
    }
}
