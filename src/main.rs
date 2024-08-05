use clap::Parser;

#[tokio::main]
async fn main() -> tokio::io::Result<()> {
    zpl::run(zpl::Args::parse()).await
}
