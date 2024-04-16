// Uncomment this block to pass the first stage

use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4221").await?;

    loop {
        let (_stream, _) = listener.accept().await?;
        println!("accepted new connection");
    }
}
